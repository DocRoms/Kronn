"""Unit tests for the kronn-internal MCP bridge helpers.

Run from the repo root:
    python3 -m unittest backend.scripts.test_disc_introspection_mcp
or via the Makefile:
    make test-python

These tests exercise the three pieces of the 0.8.5 auto-inheritance
plumbing whose only validation before this file was live-by-eyeball:

  * `_current_disc_meta()` — cache behaviour, env-var handling, error
    paths (backend unreachable / missing KRONN_DISCUSSION_ID).
  * `_current_project_id()` — derives from the meta cache, no separate
    HTTP call.
  * `call_disc_create()` — auto-fills `project_id` + `source_agent`
    from the parent disc, does NOT touch `source_session_id` (the
    idempotency-collision guard documented at
    `disc-introspection-mcp.py:527`).

We mock at two boundaries:
  - `urllib.request.urlopen`  for `_current_disc_meta()`'s direct HTTP.
  - the module-level `_http`  for `call_disc_create()` — easier than
    threading a urlopen mock through both the meta lookup and the
    create POST, and `_http` already has its own integration coverage
    via the live-run smoke test the user runs by hand.

Stdlib `unittest` + `unittest.mock` only — zero extra dev deps,
matches the script's own "no third-party packages" discipline.
"""

import io
import importlib.util
import os
import sys
import unittest
from pathlib import Path
from unittest import mock

_SCRIPT = Path(__file__).resolve().parent / "disc-introspection-mcp.py"


def _load_module():
    """Load disc-introspection-mcp.py despite the kebab-case filename.

    Standard `import` can't handle the hyphens, so we use importlib's
    file-loader API. Re-loaded fresh in `setUp` so per-process caches
    (`_CURRENT_DISC_META_CACHE`) don't leak between tests.
    """
    spec = importlib.util.spec_from_file_location("kronn_mcp", _SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class CurrentDiscMetaCacheTests(unittest.TestCase):
    """Behaviour of `_current_disc_meta()` and its cache."""

    def setUp(self):
        self.mod = _load_module()
        # Each test starts with the env we control and a fresh cache.
        self.env_patch = mock.patch.dict(
            os.environ,
            {
                "KRONN_DISCUSSION_ID": "disc-abc",
                "KRONN_BACKEND_URL": "http://127.0.0.1:3140",
            },
            clear=False,
        )
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)

    def _fake_response(self, payload):
        """Build a context-manager that mimics `urlopen()`'s usage."""
        body = (b'{"success": true, "data": ' + payload.encode() + b'}')
        cm = mock.MagicMock()
        cm.__enter__.return_value.read.return_value = body
        cm.__exit__.return_value = False
        return cm

    def test_cache_miss_fetches_from_backend_and_returns_struct(self):
        """First call to `_current_disc_meta` hits HTTP and returns the
        triple `{id, project_id, agent}`."""
        payload = (
            '{"project_id": "proj-front-eu", "agent": "ClaudeCode", '
            '"message_count": 12}'
        )
        with mock.patch("urllib.request.urlopen", return_value=self._fake_response(payload)) as urlopen:
            meta = self.mod._current_disc_meta()
        self.assertIsNotNone(meta)
        self.assertEqual(meta["id"], "disc-abc")
        self.assertEqual(meta["project_id"], "proj-front-eu")
        self.assertEqual(meta["agent"], "ClaudeCode")
        urlopen.assert_called_once()

    def test_cache_hit_does_not_refetch(self):
        """Second call returns the cached value with zero HTTP traffic
        — guarantees the auto-inherit helpers don't multiply backend
        load per MCP tool invocation."""
        payload = '{"project_id": "proj-a", "agent": "ClaudeCode"}'
        with mock.patch("urllib.request.urlopen", return_value=self._fake_response(payload)) as urlopen:
            self.mod._current_disc_meta()
            self.mod._current_disc_meta()
            self.mod._current_disc_meta()
        self.assertEqual(urlopen.call_count, 1, "cache must short-circuit follow-up calls")

    def test_missing_kronn_discussion_id_returns_none_silently(self):
        """If the launcher didn't set `KRONN_DISCUSSION_ID`, the helper
        must NOT raise — it has to return `None` so callers fall back
        to pre-0.8.5 behaviour (no inheritance). The cache still gets
        marked `checked` to avoid repeat env probes."""
        with mock.patch.dict(os.environ, {}, clear=True), \
             mock.patch("urllib.request.urlopen") as urlopen:
            meta = self.mod._current_disc_meta()
        self.assertIsNone(meta)
        urlopen.assert_not_called()
        # Calling again should also be a no-op (cache marked checked).
        with mock.patch("urllib.request.urlopen") as urlopen2:
            self.mod._current_disc_meta()
        urlopen2.assert_not_called()

    def test_backend_unreachable_returns_none_and_logs_to_stderr(self):
        """If the backend is down (urlopen raises), the helper must
        swallow the error, return `None`, and surface a stderr line so
        the user can investigate. It must NOT crash the MCP server —
        every tool call would then 500 and the agent loop dies."""
        import urllib.error
        err = urllib.error.URLError("Connection refused")
        with mock.patch("urllib.request.urlopen", side_effect=err), \
             mock.patch("sys.stderr", new_callable=io.StringIO) as stderr:
            meta = self.mod._current_disc_meta()
        self.assertIsNone(meta)
        self.assertIn("failed to resolve current disc's meta", stderr.getvalue())

    def test_current_project_id_derives_from_meta(self):
        """`_current_project_id` is a thin accessor over the same
        cache — no separate HTTP."""
        payload = '{"project_id": "proj-xyz", "agent": "Codex"}'
        with mock.patch("urllib.request.urlopen", return_value=self._fake_response(payload)) as urlopen:
            pid = self.mod._current_project_id()
            # Second call must hit the cache, not the network.
            self.mod._current_project_id()
        self.assertEqual(pid, "proj-xyz")
        self.assertEqual(urlopen.call_count, 1)


class CallDiscCreateAutoInheritTests(unittest.TestCase):
    """The auto-fill contract on `call_disc_create`.

    We mock `_http` directly so we can inspect the body the helper
    sends to `/api/disc/create`. The cache is pre-seeded so each test
    runs against a known `{id, project_id, agent}` parent.
    """

    def setUp(self):
        self.mod = _load_module()
        self.mod._CURRENT_DISC_META_CACHE.update({
            "checked": True,
            "value": {
                "id": "disc-parent",
                "project_id": "proj-front-eu",
                "agent": "ClaudeCode",
            },
        })
        # _http normally returns the parsed envelope. We mimic the
        # success-shape so `_unwrap()` extracts our payload.
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {"disc_id": "disc-new", "created": True},
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_auto_fills_project_id_and_source_agent_when_omitted(self):
        """The killer path — agent calls `disc_create` with only the
        2 required args. Validator must inject the inherited project
        binding + source_agent so the disc lands in the right project
        AND renders the sidebar 📥 ClaudeCode badge."""
        self.mod.call_disc_create({"title": "New chat", "agent": "Codex"})
        method, path, body = self.fake_http.call_args.args
        self.assertEqual(method, "POST")
        self.assertEqual(path, "/api/disc/create")
        self.assertEqual(body["project_id"], "proj-front-eu")
        self.assertEqual(body["source_agent"], "ClaudeCode")
        # The user-supplied agent (the child's CLI) is NOT the source_agent
        # — source_agent identifies the AGENT THAT TRIGGERED THE CREATE,
        # which is the parent disc's runtime.
        self.assertEqual(body["agent"], "Codex")

    def test_does_not_auto_fill_source_session_id(self):
        """SAFETY-CRITICAL: `(source_agent, source_session_id)` is the
        idempotency key in `api/disc_source.rs::disc_create`. Auto-
        filling both with `(parent.agent, parent.id)` would collapse
        every sibling MCP-created disc back to the first one.

        This test pins that we leave session-id to the agent. If you
        change this, also update the idempotency contract docs."""
        self.mod.call_disc_create({"title": "Child", "agent": "Codex"})
        _, _, body = self.fake_http.call_args.args
        self.assertNotIn("source_session_id", body)

    def test_explicit_values_are_not_overridden(self):
        """An agent passing `project_id` / `source_agent` explicitly
        must win — auto-fill is the fallback, not a steamroller."""
        self.mod.call_disc_create({
            "title": "Cross-project share",
            "agent": "Codex",
            "project_id": "proj-different",
            "source_agent": "ManualImport",
            "source_session_id": "ext-sess-42",
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["project_id"], "proj-different")
        self.assertEqual(body["source_agent"], "ManualImport")
        self.assertEqual(body["source_session_id"], "ext-sess-42")

    def test_missing_required_fields_raise_runtime_error_with_clear_message(self):
        """Pre-0.8.5 the script's own assertions had to stay sharp —
        the wizard isn't on the path here, so a malformed agent payload
        would 500 the MCP if we didn't guard. The error messages must
        name the missing field so the agent can self-correct."""
        with self.assertRaises(RuntimeError) as ctx:
            self.mod.call_disc_create({"agent": "Codex"})
        self.assertIn("title", str(ctx.exception))

        with self.assertRaises(RuntimeError) as ctx:
            self.mod.call_disc_create({"title": "x"})
        self.assertIn("agent", str(ctx.exception))

    def test_no_parent_meta_skips_inheritance_silently(self):
        """Legacy / dev launchers might not set KRONN_DISCUSSION_ID.
        The helper must still create the disc, just without
        inherited fields — same as pre-0.8.5 behaviour."""
        self.mod._CURRENT_DISC_META_CACHE.update({"checked": True, "value": None})
        self.mod.call_disc_create({"title": "Standalone", "agent": "Codex"})
        _, _, body = self.fake_http.call_args.args
        self.assertNotIn("project_id", body)
        self.assertNotIn("source_agent", body)
        self.assertNotIn("source_session_id", body)


if __name__ == "__main__":
    unittest.main()
