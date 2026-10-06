#!/usr/bin/env python3
"""Fail when an axum body limit exceeds the nginx cap in front of it.

Each `DefaultBodyLimit::max(..)` attached to a route in backend/src/lib.rs is
compared with the `client_max_body_size` of the nginx location serving that
path in .docker/nginx.conf: an exact `location = <path>` when there is one,
the server-level value otherwise. A route whose limit nginx cannot carry
gets a 413 from the gateway in Docker, before the backend sees the request.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
LIB_RS = ROOT / "backend" / "src" / "lib.rs"
NGINX_CONF = ROOT / ".docker" / "nginx.conf"

_ROUTE_RE = re.compile(r'\.route\(\s*"([^"]+)"(.*?)(?=\.route\(|\Z)', re.S)
_LIMIT_RE = re.compile(r"DefaultBodyLimit::max\(([^)]*)\)")
_SIZE_RE = re.compile(r"client_max_body_size\s+(\d+)([kKmMgG]?)\s*;")
_UNITS = {"": 1, "k": 1024, "m": 1024**2, "g": 1024**3}


def _eval_product(expr: str) -> int:
    value = 1
    for factor in expr.replace("_", "").split("*"):
        value *= int(factor.strip())
    return value


def axum_limits(source: str) -> dict[str, int]:
    """Route path -> body limit in bytes, for routes that set one."""
    limits: dict[str, int] = {}
    for path, body in _ROUTE_RE.findall(source):
        match = _LIMIT_RE.search(body)
        if match:
            limits[path] = _eval_product(match.group(1))
    return limits


def _size(match: re.Match[str]) -> int:
    return int(match.group(1)) * _UNITS[match.group(2).lower()]


def nginx_limits(conf: str) -> tuple[int, dict[str, int]]:
    """(server-level cap, exact location path -> cap), in bytes."""
    exact: dict[str, int] = {}
    conf = re.sub(r"#[^\n]*", "", conf)
    for match in re.finditer(r"location\s*=\s*(\S+)\s*\{([^}]*)\}", conf):
        size = _SIZE_RE.search(match.group(2))
        if size:
            exact[match.group(1)] = _size(size)
    # Drop every location block, so only the server-level directive remains.
    stripped = re.sub(r"location[^{]*\{[^}]*\}", "", conf)
    server = _SIZE_RE.search(stripped)
    # nginx's built-in default when nothing is set.
    return (_size(server) if server else 1024**2), exact


def violations(source: str, conf: str) -> list[str]:
    server, exact = nginx_limits(conf)
    found = []
    for path, limit in sorted(axum_limits(source).items()):
        cap = exact.get(path, server)
        if limit > cap:
            found.append(
                f"{path}: axum accepts {limit} bytes, nginx caps at {cap} bytes"
            )
    return found


def main() -> int:
    source = LIB_RS.read_text(encoding="utf-8")
    conf = NGINX_CONF.read_text(encoding="utf-8")
    if not axum_limits(source):
        print("no DefaultBodyLimit route found in lib.rs; parser out of date")
        return 1
    found = violations(source, conf)
    for line in found:
        print(line)
    if found:
        print("raise the nginx location cap (.docker/nginx.conf) for these routes")
        return 1
    print(f"nginx body caps cover {len(axum_limits(source))} axum route limits")
    return 0


if __name__ == "__main__":
    sys.exit(main())
