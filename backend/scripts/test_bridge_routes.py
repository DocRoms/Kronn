"""KT-1006 — the bridge token's positive list matches what the bridge calls.

`backend/src/core/bridge_token.rs` lists the (method, route) pairs a scoped
bridge token may call. This test reads `disc-introspection-mcp.py` with the
`ast` module, collects every API path it sends, and fails when a call is
missing from the list (rooms would go mute on that tool) or when the list
grants a route the bridge never calls (needless surface).
"""

import ast
import re
import unittest
from pathlib import Path

_HERE = Path(__file__).resolve().parent
_SCRIPT = _HERE / "disc-introspection-mcp.py"
_RUST_LIST = _HERE.parent / "src" / "core" / "bridge_token.rs"

_HTTP_HELPERS = {"_http", "_http_transport_retry", "_http_text"}


def _flatten(node):
    """A string constant or f-string, with `{}` for each interpolation."""
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.JoinedStr):
        return "".join(
            part.value if isinstance(part, ast.Constant) else "{}"
            for part in node.values
        )
    return None


def _normalize(path):
    """`/api/x/{}/y?q` -> `/api/x/{}/y`; a placeholder glued to the end of a
    segment is a query suffix (`/tasks{suffix}`), not a path segment."""
    if path.startswith("{}/api/"):
        path = path[2:]
    path = path.split("?", 1)[0]
    while path.endswith("{}") and not path.endswith("/{}"):
        path = path[:-2]
    return path


def _docstring_nodes(tree):
    nodes = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            body = node.body
            if body and isinstance(body[0], ast.Expr) and isinstance(body[0].value, ast.Constant):
                nodes.add(id(body[0].value))
    return nodes


def _agent_library_paths(tree):
    """`_AGENT_LIB[kind]["path"]` values: the skill/profile/directive routes."""
    paths = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == "_AGENT_LIB" for target in node.targets
        ):
            for spec in node.value.values:
                for key, value in zip(spec.keys, spec.values):
                    if _flatten(key) == "path":
                        paths.append(_flatten(value))
    return paths


def bridge_calls():
    """Every (method, path) the bridge sends. Method `*` = sent through a
    helper whose method is a variable; the path still has to be listed."""
    tree = ast.parse(_SCRIPT.read_text(encoding="utf-8"))
    library = _agent_library_paths(tree)
    calls = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        func = node.func
        name = func.id if isinstance(func, ast.Name) else getattr(func, "attr", None)
        if name not in _HTTP_HELPERS or len(node.args) < 2:
            continue
        method = _flatten(node.args[0])
        if method is None:
            continue
        target = node.args[1]
        path = _flatten(target)
        if path is None and isinstance(target, ast.Subscript) and _flatten(target.slice) == "path":
            calls.update((method, _normalize(lib)) for lib in library)
        elif path is not None and path.startswith("{}/") and not path.startswith("{}/api/"):
            # f"{spec['path']}/{iid}": an item of the agent library.
            calls.update((method, _normalize(lib + path[2:])) for lib in library)
        elif path is not None and _normalize(path).startswith("/api/"):
            calls.add((method, _normalize(path)))
    known_paths = {path for _, path in calls}
    docstrings = _docstring_nodes(tree)
    for node in ast.walk(tree):
        if id(node) in docstrings or not isinstance(node, (ast.Constant, ast.JoinedStr)):
            continue
        text = _flatten(node)
        if not text:
            continue
        path = _normalize(text)
        # Error-message fragments and prefix tests end with "/"; prose has spaces.
        if not path.startswith("/api/") or path.endswith("/") or any(c in path for c in " `<>"):
            continue
        if path not in known_paths:
            calls.add(("*", path))
    return calls


def positive_list():
    """The (method, path) pairs `BRIDGE_ROUTES` grants, placeholders normalized."""
    source = _RUST_LIST.read_text(encoding="utf-8")
    start = source.index("pub const BRIDGE_ROUTES")
    end = source.index("];", start)
    pairs = re.findall(
        r'r\(\s*"(GET|POST|PUT|PATCH|DELETE)",\s*"(/api/[^"]+)"', source[start:end]
    )
    return {(method, re.sub(r"\{[a-z_]+\}", "{}", path)) for method, path in pairs}


class BridgeRouteListTests(unittest.TestCase):
    def setUp(self):
        self.calls = bridge_calls()
        self.granted = positive_list()

    def test_the_extractor_sees_the_bridge_s_calls(self):
        # Guards the parser itself: a regression here would make both checks
        # below vacuous.
        self.assertGreater(len(self.calls), 80)
        self.assertIn(("POST", "/api/disc/append"), self.calls)
        self.assertIn(("PUT", "/api/skills/{}"), self.calls)
        self.assertIn(("*", "/api/orchestration/tool/launch"), self.calls)
        self.assertIn(("GET", "/api/planning/tasks"), self.calls)

    def test_every_bridge_call_is_granted(self):
        granted_paths = {path for _, path in self.granted}
        missing = sorted(
            (method, path)
            for method, path in self.calls
            if (method == "*" and path not in granted_paths)
            or (method != "*" and (method, path) not in self.granted)
        )
        self.assertEqual(
            missing,
            [],
            "the bridge calls routes a bridge token cannot reach: add them to "
            "BRIDGE_ROUTES in backend/src/core/bridge_token.rs",
        )

    def test_the_list_grants_nothing_the_bridge_never_calls(self):
        called = {(method, path) for method, path in self.calls}
        dynamic = {path for method, path in self.calls if method == "*"}
        extra = sorted(
            (method, path)
            for method, path in self.granted
            if (method, path) not in called and path not in dynamic
        )
        self.assertEqual(
            extra,
            [],
            "BRIDGE_ROUTES grants routes the bridge never calls: remove them",
        )


if __name__ == "__main__":
    unittest.main()
