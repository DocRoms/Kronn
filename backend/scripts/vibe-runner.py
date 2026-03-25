#!/usr/bin/env python3
"""Kronn Vibe Runner — Real Vibe agent without MCP.

Uses Vibe's programmatic API (run_programmatic) which gives the full agent
with local tools (bash, read/write/edit file, grep, glob, web_search, etc.)
but disables MCP servers that cause the CLI to hang.

Falls back to direct Mistral API streaming if the vibe package is not
installed (e.g. in Docker without vibe).

Usage:
    python3 vibe-runner.py "your prompt here"
    python3 vibe-runner.py --model devstral-small-latest "prompt"

Requires: MISTRAL_API_KEY environment variable or ~/.vibe/.env.
"""
import argparse
import json
import os
import sys


def maybe_reexec_with_vibe_python():
    """Re-exec with vibe's own Python if we can't import vibe."""
    try:
        import vibe  # noqa: F401
        return  # Already using the right Python
    except ImportError:
        pass
    # Find vibe's Python from the shebang of the vibe binary
    import shutil, subprocess
    vibe_bin = shutil.which("vibe")
    if not vibe_bin:
        return  # vibe not installed, will use API fallback
    try:
        with open(vibe_bin) as f:
            shebang = f.readline().strip().lstrip("#!")
        if shebang and os.path.exists(shebang):
            os.execv(shebang, [shebang] + sys.argv)
    except Exception:
        pass  # Fall through to API fallback


# Bootstrap: ensure we use vibe's Python if available
maybe_reexec_with_vibe_python()


def parse_args():
    parser = argparse.ArgumentParser(description="Kronn Vibe Runner")
    parser.add_argument("prompt", help="Prompt to send")
    parser.add_argument("--model", default=None, help="Mistral model name")
    parser.add_argument("--max-tokens", type=int, default=16384, help="Max output tokens")
    parser.add_argument("--max-turns", type=int, default=None, help="Max agent turns")
    return parser.parse_args()


def load_vibe_env_key():
    """Read MISTRAL_API_KEY from ~/.vibe/.env (same source as Vibe CLI)."""
    env_path = os.path.join(os.path.expanduser("~"), ".vibe", ".env")
    try:
        with open(env_path) as f:
            for line in f:
                line = line.strip()
                if line.startswith("MISTRAL_API_KEY="):
                    val = line.split("=", 1)[1].strip().strip("'\"")
                    if val:
                        return val
    except OSError:
        pass
    return None


def ensure_api_key():
    """Ensure MISTRAL_API_KEY is available."""
    key = os.environ.get("MISTRAL_API_KEY", "") or load_vibe_env_key()
    if not key:
        print("Error: MISTRAL_API_KEY not set (checked env and ~/.vibe/.env)", file=sys.stderr)
        sys.exit(1)
    # Set in env so vibe SDK picks it up too
    os.environ["MISTRAL_API_KEY"] = key
    return key


def run_with_vibe_sdk(args):
    """Run using the real Vibe agent engine (local tools, no MCP)."""
    from vibe.cli.entrypoint import init_harness_files_manager
    from vibe.cli.cli import load_dotenv_values, bootstrap_config_files, load_config_or_exit
    from vibe.core.programmatic import run_programmatic

    init_harness_files_manager("user", "project")
    load_dotenv_values()
    bootstrap_config_files()
    config = load_config_or_exit()

    if args.model:
        config.model = args.model

    result = run_programmatic(
        config=config,
        prompt=args.prompt,
        max_turns=args.max_turns,
    )

    if result:
        print(result)


def run_with_api_fallback(args, api_key):
    """Fallback: direct Mistral API streaming (no tools, no agent)."""
    import urllib.request
    import urllib.error

    model = args.model or "mistral-vibe-cli-latest"

    payload = json.dumps({
        "model": model,
        "messages": [{"role": "user", "content": args.prompt}],
        "max_tokens": args.max_tokens,
        "stream": True,
    }).encode()

    req = urllib.request.Request(
        "https://api.mistral.ai/v1/chat/completions",
        data=payload,
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
        },
    )

    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            for line in resp:
                line = line.decode().strip()
                if not line.startswith("data: "):
                    continue
                data = line[6:]
                if data == "[DONE]":
                    break
                try:
                    chunk = json.loads(data)
                    delta = chunk.get("choices", [{}])[0].get("delta", {})
                    content = delta.get("content", "")
                    if content:
                        sys.stdout.write(content)
                        sys.stdout.flush()
                except json.JSONDecodeError:
                    continue

        sys.stdout.write("\n")
        sys.stdout.flush()

    except urllib.error.HTTPError as e:
        body = e.read().decode()
        print(f"Mistral API error {e.code}: {body}", file=sys.stderr)
        sys.exit(1)


def main():
    args = parse_args()
    api_key = ensure_api_key()

    try:
        import vibe  # noqa: F401
        run_with_vibe_sdk(args)
    except ImportError:
        # Vibe not installed (e.g. Docker) — use API fallback
        run_with_api_fallback(args, api_key)
    except Exception as e:
        print(f"Vibe SDK error: {e}", file=sys.stderr)
        print("Falling back to direct API...", file=sys.stderr)
        run_with_api_fallback(args, api_key)


if __name__ == "__main__":
    main()
