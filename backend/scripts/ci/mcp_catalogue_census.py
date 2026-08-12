#!/usr/bin/env python3
"""MCP catalogue census, per server and per tool — KT-192 DoD 0.

`mcp_surface_budget.py` already ratchets ONE catalogue: Kronn's own bridge, at
92 444 B. But an agent session receives the union of every configured server, and
nothing was measuring that union. Kronn stores no tool catalogue of its own — a
server's declarations only exist at runtime, in its `tools/list` reply — so the
only honest way to size the real surface is to ask each server.

That is what this does: MCP handshake (`initialize` → `notifications/initialized`
→ `tools/list`), then bytes per tool and per server, exactly as a client receives
them.

TWO RULES IT KEEPS.

A server that could not be probed is `unmeasured`, NEVER 0. A crashed or
credential-less server contributes an unknown amount, and reporting it as free
would understate the very number this exists to expose — the same rule the CLI
telemetry follows after 4.3 billion tokens were once stored as zero.

And it only ever calls `initialize` and `tools/list`. Both are read-only by
design: no tool is invoked and nothing is written. A server receives the env its
own declaration asks for — exactly what a client passes it, and nowhere else. A
census that could act would be a census nobody dares run.

Usage:
    python3 backend/scripts/ci/mcp_catalogue_census.py [--json] [--timeout N]
    python3 backend/scripts/ci/mcp_catalogue_census.py --only kronn-internal
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import subprocess
import sys

BYTES_PER_TOKEN = 3.7
DEFAULT_TIMEOUT = 25

# Where a client looks for server declarations, project scope first.
PROJECT_CONFIGS = [pathlib.Path(".mcp.json"), pathlib.Path(".claude/mcp.json")]

# User scope, taken as a PARAMETER rather than read from the environment. A
# measurement function that always reads the developer's home directory cannot be
# verified — a test asking about an isolated tree would still receive that
# machine's servers, which is how a census starts reporting someone else's
# surface.
DEFAULT_USER_CONFIG = pathlib.Path(os.path.expanduser("~/.claude.json"))


def load_servers(
    repo_root: pathlib.Path,
    user_config: pathlib.Path | None = None,
) -> dict[str, dict]:
    """Every declared stdio server, project scope winning on a name clash.

    A name declared in both scopes is ONE server for the client, so counting it
    twice would inflate the total — the opposite of the error this script exists
    to catch, and just as wrong.
    """
    found: dict[str, dict] = {}
    candidates = [repo_root / name for name in PROJECT_CONFIGS]
    if user_config is not None:
        candidates.append(user_config)
    for path in candidates:
        if not path.is_file():
            continue
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue

        blocks = [data.get("mcpServers") or {}]
        # `~/.claude.json` nests per-project blocks; take the one for this repo.
        for project, config in (data.get("projects") or {}).items():
            if pathlib.Path(project).resolve() == repo_root.resolve():
                blocks.append(config.get("mcpServers") or {})
        for block in blocks:
            for name, spec in block.items():
                found.setdefault(name, spec)
    return found


def probe(name: str, spec: dict, timeout: int) -> dict:
    """Ask one server for its catalogue.

    Returns a row with `tools` on success, or `unmeasured` with the reason. The
    reason is kept because "we could not size this server" is only actionable
    once you know why.
    """
    command = spec.get("command")
    if not command:
        return {"server": name, "unmeasured": "no command (http/sse transport)"}

    argv = [command, *spec.get("args", [])]
    env = {**os.environ, **(spec.get("env") or {})}

    requests = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "kronn-census", "version": "0"},
            },
        },
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
    ]
    payload = "".join(json.dumps(r) + "\n" for r in requests)

    try:
        done = subprocess.run(
            argv,
            input=payload,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
            # The server's OWN declared env, exactly as a client passes it. Omitting
            # it made a measurable server report "no API key" and land in the
            # unmeasured column — a surface counted as unknown when it was simply
            # not being given what its declaration asks for.
            env=env,
        )
    except FileNotFoundError:
        return {"server": name, "unmeasured": f"{command} not on PATH"}
    except subprocess.TimeoutExpired:
        return {"server": name, "unmeasured": f"no reply within {timeout}s"}
    except OSError as error:
        return {"server": name, "unmeasured": f"spawn failed: {error}"}

    for line in done.stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("id") != 2:
            continue
        if "error" in message:
            return {"server": name, "unmeasured": f"tools/list error: {message['error']}"}
        tools = (message.get("result") or {}).get("tools")
        if tools is None:
            continue
        rows = [
            {"name": tool.get("name", "?"), "bytes": len(json.dumps(tool).encode())}
            for tool in tools
        ]
        return {"server": name, "tools": rows}

    tail = (done.stderr or "").strip().splitlines()
    return {
        "server": name,
        "unmeasured": "no tools/list reply" + (f" — {tail[-1][:120]}" if tail else ""),
    }


def census(
    repo_root: pathlib.Path,
    timeout: int,
    only: list[str],
    user_config: pathlib.Path | None = None,
) -> list[dict]:
    servers = load_servers(repo_root, user_config)
    if only:
        servers = {k: v for k, v in servers.items() if k in only}
    return [probe(name, spec, timeout) for name, spec in sorted(servers.items())]


def report(rows: list[dict]) -> int:
    measured = [r for r in rows if "tools" in r]
    unmeasured = [r for r in rows if "tools" not in r]

    print(f"{'server':<26}{'tools':>7}{'bytes':>12}{'~tokens':>10}")
    print("-" * 55)
    total_bytes = 0
    total_tools = 0
    for row in sorted(measured, key=lambda r: -sum(t["bytes"] for t in r["tools"])):
        size = sum(tool["bytes"] for tool in row["tools"])
        total_bytes += size
        total_tools += len(row["tools"])
        print(f"{row['server']:<26}{len(row['tools']):>7}{size:>12,}"
              f"{int(size / BYTES_PER_TOKEN):>10,}")
    print("-" * 55)
    print(f"{'MEASURED TOTAL':<26}{total_tools:>7}{total_bytes:>12,}"
          f"{int(total_bytes / BYTES_PER_TOKEN):>10,}")

    if unmeasured:
        # Never folded into the total as zero: their cost is unknown, and the
        # total below is a FLOOR.
        print(f"\nUNMEASURED ({len(unmeasured)}) — the total above is a floor, not a total:")
        for row in unmeasured:
            print(f"  {row['server']:<26}{row['unmeasured']}")

    if measured:
        heaviest = sorted(
            ((t["bytes"], t["name"], r["server"]) for r in measured for t in r["tools"]),
            reverse=True,
        )[:10]
        print("\nheaviest declarations:")
        for size, name, server in heaviest:
            print(f"  {size:>7,}  {server} · {name}")

    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", default=".")
    parser.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT)
    parser.add_argument("--only", default="", help="comma-separated server names")
    parser.add_argument("--project-only", action="store_true",
                        help="ignore the user-scope config")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    only = [s for s in args.only.split(",") if s]
    user_config = None if args.project_only else DEFAULT_USER_CONFIG
    rows = census(pathlib.Path(args.repo_root), args.timeout, only, user_config)
    if args.json:
        print(json.dumps(rows, indent=2))
        return 0
    return report(rows)


if __name__ == "__main__":
    sys.exit(main())
