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


class CallDiscGetMessageTests(unittest.TestCase):
    """Selector and bounded-window contract for the cheap message reader."""

    def setUp(self):
        self.mod = _load_module()
        self.env_patch = mock.patch.dict(
            os.environ,
            {"KRONN_DISCUSSION_ID": "disc-abc"},
            clear=False,
        )
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {"idx": 4, "id": "12345678-full", "content": "hello"},
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_idx_remains_backward_compatible(self):
        self.mod.call_disc_get_message({"idx": -1})
        method, path = self.fake_http.call_args.args
        self.assertEqual(method, "GET")
        self.assertEqual(
            path,
            "/api/discussions/disc-abc/message/-1?before=0&after=0",
        )

    def test_catalog_exposes_either_selector_and_bounded_window(self):
        tool = next(item for item in self.mod.TOOLS if item["name"] == "disc_get_message")
        schema = tool["inputSchema"]
        self.assertIn("oneOf", schema)
        self.assertEqual(schema["properties"]["before"]["maximum"], 10)
        self.assertEqual(schema["properties"]["after"]["maximum"], 10)
        self.assertIn("message_id", schema["properties"])

    def test_short_reference_can_request_a_context_window(self):
        self.mod.call_disc_get_message({
            "message_id": "MSG-12345678",
            "before": 2,
            "after": 3,
        })
        method, path = self.fake_http.call_args.args
        self.assertEqual(method, "GET")
        self.assertEqual(
            path,
            "/api/discussions/disc-abc/message/MSG-12345678?before=2&after=3",
        )

    def test_requires_exactly_one_selector(self):
        with self.assertRaisesRegex(RuntimeError, "exactly one"):
            self.mod.call_disc_get_message({})
        with self.assertRaisesRegex(RuntimeError, "exactly one"):
            self.mod.call_disc_get_message({"idx": 1, "message_id": "MSG-12345678"})

    def test_rejects_oversized_or_non_integer_windows_before_http(self):
        with self.assertRaisesRegex(RuntimeError, "before"):
            self.mod.call_disc_get_message({"idx": 1, "before": 11})
        with self.assertRaisesRegex(RuntimeError, "after"):
            self.mod.call_disc_get_message({"idx": 1, "after": True})
        self.fake_http.assert_not_called()


class McpListEnrichedOutputTests(unittest.TestCase):
    """0.8.6 — the `mcp_list` tool surfaces enough metadata for the
    agent to PICK an API natively (description, docs link, per-endpoint
    description, side_effect flag, custom-plugin marker).

    These tests pin the contract the [[project_agent_api_broker_0_8_6]]
    work relies on: if the agent can't see `docs_url` or `is_custom` in
    its tool-call response, the user has to spoon-feed each new API to
    every agent — exactly the friction the broker is removing.
    """

    def setUp(self):
        self.mod = _load_module()

    def test_mcp_list_surfaces_docs_url_description_and_custom_flag(self):
        fake_backend = {
            "success": True,
            "data": {
                "configs": [
                    {
                        "id": "cfg-1", "server_id": "api-didomi",
                        "is_global": False, "project_ids": ["proj-front-eu"],
                        "label": "Didomi prod",
                    },
                ],
                "servers": [
                    {
                        "id": "api-didomi",
                        "name": "Didomi",
                        "description": "GDPR/CCPA consent management",
                        "tags": ["consent", "privacy"],
                        "api_spec": {
                            "description": "",  # often empty on first-party plugins
                            "docs_url": "https://developers.didomi.io/api/",
                            "endpoints": [
                                {
                                    "path": "/widgets",
                                    "method": "GET",
                                    "description": "List configured widgets",
                                    "side_effect": False,
                                },
                                {
                                    "path": "/widgets",
                                    "method": "POST",
                                    "description": "Create a widget",
                                    "side_effect": True,
                                },
                            ],
                        },
                    },
                    {
                        # Real custom-plugin id pattern is `custom-{slug}-{nano}`
                        # (cf. backend/src/api/mcps.rs:82-86). The sentinel
                        # "api-custom" is used ONLY in the create payload.
                        "id": "custom-acme-internal-27c67bd7",
                        "name": "Acme Internal API",
                        "description": "Custom plugin for the internal Acme service",
                        "tags": [],
                        "api_spec": {
                            "description": "Acme internal endpoints",
                            "docs_url": "https://wiki.acme.example/api",
                            "endpoints": [
                                {"path": "/v1/ping", "method": "GET",
                                 "description": "Health check", "side_effect": False},
                            ],
                        },
                    },
                    # Server with NO api_spec must be filtered out (e.g. stdio-only MCPs).
                    {"id": "mcp-no-api", "name": "Stdio only", "description": "x", "api_spec": None},
                ],
            },
        }
        with mock.patch.object(self.mod, "_http", return_value=fake_backend):
            out = self.mod.call_mcp_list({})

        # Configs are forwarded with the existing minimal shape.
        self.assertEqual(len(out["configs"]), 1)
        self.assertEqual(out["configs"][0]["server_id"], "api-didomi")

        # Stdio-only MCPs (no api_spec) excluded.
        servers = out["servers_with_api"]
        self.assertEqual(len(servers), 2)

        # Didomi: description falls back to the server-level field when
        # api_spec.description is empty (very common on first-party plugins).
        didomi = next(s for s in servers if s["id"] == "api-didomi")
        self.assertEqual(didomi["docs_url"], "https://developers.didomi.io/api/")
        self.assertIn("GDPR", didomi["description"])
        self.assertFalse(didomi["is_custom"])
        self.assertEqual(didomi["tags"], ["consent", "privacy"])

        # Per-endpoint description + side_effect flag preserved.
        get_widgets = next(e for e in didomi["endpoints"] if e["method"] == "GET")
        post_widgets = next(e for e in didomi["endpoints"] if e["method"] == "POST")
        self.assertEqual(get_widgets["description"], "List configured widgets")
        self.assertFalse(get_widgets["side_effect"])
        self.assertTrue(post_widgets["side_effect"],
                        "side_effect=true MUST surface so the future broker can gate it")

        # Custom plugin marker so the agent knows it's user-defined
        # (not a registry plugin) — informs the answer to
        # "what custom APIs do I have access to?". REAL id pattern is
        # `custom-{slug}-{nano}`, NOT `api-custom-*` (sentinel only).
        acme = next(s for s in servers if s["id"].startswith("custom-acme-internal"))
        self.assertTrue(acme["is_custom"], "custom-* prefix MUST mark as custom")
        self.assertEqual(acme["docs_url"], "https://wiki.acme.example/api")

    def test_mcp_list_emits_machine_actionable_hint_per_plugin(self):
        """The agent shouldn't have to encode the 'endpoints empty →
        read docs' heuristic in its own system prompt — it would
        fragment across CLIs. `mcp_list` emits a `hint` field on every
        plugin with one of 3 prefixes: READY / NEEDS_RESEARCH / AMBIGUOUS.

        Locks the contract the workflow-architect skill and any future
        agent-api-broker rely on. If you change the prefix names, update
        the skill docs too (search for `NEEDS_RESEARCH` across repo)."""
        fake = {
            "success": True,
            "data": {
                "configs": [],
                "servers": [
                    {
                        "id": "api-ready", "name": "Ready",
                        "description": "x",
                        "api_spec": {
                            "docs_url": "https://docs.example.com/ready",
                            "endpoints": [{"path": "/x", "method": "GET"}],
                        },
                    },
                    {
                        # User's actual Didomi-shaped case (2026-05-19):
                        # custom plugin, docs_url set, endpoints not
                        # yet declared.
                        "id": "custom-needs-research-abc",
                        "name": "Needs Research",
                        "description": "y",
                        "api_spec": {
                            "docs_url": "https://docs.example.com/needs-research",
                            "endpoints": [],
                        },
                    },
                    {
                        # Worst case: plugin is wired but has nothing
                        # actionable. Agent must ask user before doing
                        # anything.
                        "id": "custom-ambiguous-def",
                        "name": "Ambiguous",
                        "description": "z",
                        "api_spec": {"endpoints": []},
                    },
                ],
            },
        }
        with mock.patch.object(self.mod, "_http", return_value=fake):
            out = self.mod.call_mcp_list({})
        by_id = {s["id"]: s for s in out["servers_with_api"]}
        self.assertTrue(by_id["api-ready"]["hint"].startswith("READY:"))
        # NEEDS_RESEARCH hint MUST embed the docs_url so the agent
        # can fetch it without a second tool call.
        nr = by_id["custom-needs-research-abc"]["hint"]
        self.assertTrue(nr.startswith("NEEDS_RESEARCH:"))
        self.assertIn("https://docs.example.com/needs-research", nr)
        self.assertTrue(by_id["custom-ambiguous-def"]["hint"].startswith("AMBIGUOUS:"))

    def test_mcp_list_surfaces_config_keys_with_auth_managed_flag(self):
        """0.8.6 — `config_keys[]` lists every env_key the plugin's
        config exposes, each tagged `auth_managed: bool`. The agent
        uses this to know which keys are credentials handled by Kronn
        (DO NOT reference) vs free-form identifiers (FREE to use
        as `${ENV.X}` in path/query/body).

        Critical for Didomi-shape plugins where an `ORGANIZATION_ID`
        identifier lives in the encrypted env alongside the auth
        creds. Without this flag the agent can't distinguish, can't
        avoid leaking the secret, can't reference the identifier."""
        fake = {
            "success": True,
            "data": {
                "configs": [],
                "servers": [{
                    "id": "custom-didomi-abc",
                    "name": "Didomi",
                    "description": "Consent management",
                    "api_spec": {
                        "auth": {
                            "TokenExchange": {
                                "endpoint": "/sessions",
                                "body_template": {
                                    "type": "api-key",
                                    "key": "${ENV.API_KEY}",
                                    "secret": "${ENV.API_SECRET}",
                                },
                                "creds_env_keys": [],
                            },
                        },
                        "config_keys": [
                            {"env_key": "API_KEY", "label": "API Key"},
                            {"env_key": "API_SECRET", "label": "API Secret"},
                            {"env_key": "ORGANIZATION_ID", "label": "Org ID"},
                        ],
                        "endpoints": [
                            {"path": "/widgets/notices", "method": "GET"},
                        ],
                    },
                }],
            },
        }
        with mock.patch.object(self.mod, "_http", return_value=fake):
            out = self.mod.call_mcp_list({})
        didomi = out["servers_with_api"][0]
        keys = {ck["env_key"]: ck for ck in didomi["config_keys"]}
        # The 2 env keys referenced in body_template via ${ENV.X} MUST be
        # tagged auth-managed even though `creds_env_keys` is empty —
        # the body_template scan is the safety net for users who don't
        # populate the defensive field.
        self.assertTrue(keys["API_KEY"]["auth_managed"],
            "API_KEY is referenced in body_template, must be auth_managed")
        self.assertTrue(keys["API_SECRET"]["auth_managed"],
            "API_SECRET is referenced in body_template, must be auth_managed")
        # The identifier NOT used in any auth slot must be free to template.
        self.assertFalse(keys["ORGANIZATION_ID"]["auth_managed"],
            "ORGANIZATION_ID is a non-secret identifier, agent should use ${ENV.X}")
        # Label preserved from config_keys (falls back to env_key if blank).
        self.assertEqual(keys["API_KEY"]["label"], "API Key")
        self.assertEqual(keys["ORGANIZATION_ID"]["label"], "Org ID")

    def test_mcp_list_marks_explicit_auth_env_keys_for_all_variants(self):
        """Auth variants where env_keys live in dedicated fields
        (Bearer.env_key, ApiKeyHeader.env_key, Basic.user_env, etc.)
        must also flip auth_managed=true. Pins the variant-aware
        introspection so future variants are tested."""
        for variant_name, variant_data, expected_managed in [
            ("Bearer", {"env_key": "TOKEN"}, {"TOKEN"}),
            ("ApiKeyHeader", {"header_name": "X-Key", "env_key": "API_KEY"}, {"API_KEY"}),
            ("ApiKeyQuery", {"param_name": "apikey", "env_key": "KEY"}, {"KEY"}),
            ("Basic", {"user_env": "USER", "password_env": "PASS"}, {"USER", "PASS"}),
            ("BasicApiKey", {"env_key": "API_KEY"}, {"API_KEY"}),
            ("OAuth2ClientCredentials", {
                "token_url": "https://x", "client_id_env": "CID",
                "client_secret_env": "CSECRET", "scope": "read",
            }, {"CID", "CSECRET"}),
        ]:
            fake = {
                "success": True,
                "data": {
                    "configs": [],
                    "servers": [{
                        "id": f"custom-{variant_name.lower()}-test",
                        "name": variant_name,
                        "description": "x",
                        "api_spec": {
                            "auth": {variant_name: variant_data},
                            "config_keys": [
                                {"env_key": k, "label": k}
                                for k in expected_managed.union({"FREE_ID"})
                            ],
                            "endpoints": [{"path": "/x", "method": "GET"}],
                        },
                    }],
                },
            }
            with mock.patch.object(self.mod, "_http", return_value=fake):
                out = self.mod.call_mcp_list({})
            keys = {ck["env_key"]: ck["auth_managed"] for ck in out["servers_with_api"][0]["config_keys"]}
            for k in expected_managed:
                self.assertTrue(keys.get(k), f"{variant_name}: {k} must be auth_managed")
            self.assertFalse(keys.get("FREE_ID"),
                f"{variant_name}: FREE_ID is unused in auth, should NOT be auth_managed")

    def test_mcp_list_marks_custom_plugins_correctly_for_both_id_forms(self):
        """Defence-in-depth on the prefix detection: both `custom-*`
        (the real persisted form) AND the bare `api-custom` sentinel
        (which would only ever surface if the materialization step
        didn't run — paranoid edge case) must be flagged as custom.
        Registry plugins like `api-jira` or `mcp-github` must NOT be
        flagged (the substring `custom` doesn't appear in them — but
        someone in 6 months might rename and we want a test that
        catches the regression)."""
        fake = {
            "success": True,
            "data": {
                "configs": [],
                "servers": [
                    {"id": "custom-foo-abc12345", "name": "Foo",
                     "description": "x", "api_spec": {"endpoints": []}},
                    {"id": "api-custom", "name": "Sentinel — should never persist",
                     "description": "y", "api_spec": {"endpoints": []}},
                    {"id": "api-jira", "name": "Jira",
                     "description": "z", "api_spec": {"endpoints": []}},
                    {"id": "mcp-github", "name": "GitHub",
                     "description": "w", "api_spec": {"endpoints": []}},
                ],
            },
        }
        with mock.patch.object(self.mod, "_http", return_value=fake):
            out = self.mod.call_mcp_list({})
        by_id = {s["id"]: s for s in out["servers_with_api"]}
        self.assertTrue(by_id["custom-foo-abc12345"]["is_custom"])
        self.assertTrue(by_id["api-custom"]["is_custom"])
        self.assertFalse(by_id["api-jira"]["is_custom"])
        self.assertFalse(by_id["mcp-github"]["is_custom"])

    def test_mcp_list_falls_back_to_none_for_empty_endpoint_description(self):
        """Endpoints with no description must emit `None` (not empty
        string) so the agent's JSON parser can branch cleanly on
        `description ?? "(undocumented)"` rather than truthy-checking
        an empty string."""
        fake = {
            "success": True,
            "data": {
                "configs": [],
                "servers": [{
                    "id": "api-test",
                    "name": "Test",
                    "description": "Test plugin",
                    "api_spec": {
                        "description": "",
                        "docs_url": None,
                        "endpoints": [
                            {"path": "/x", "method": "GET", "description": "", "side_effect": False},
                            {"path": "/y", "method": "GET"},  # description field absent
                        ],
                    },
                }],
            },
        }
        with mock.patch.object(self.mod, "_http", return_value=fake):
            out = self.mod.call_mcp_list({})
        eps = out["servers_with_api"][0]["endpoints"]
        self.assertIsNone(eps[0]["description"])
        self.assertIsNone(eps[1]["description"])
        # And missing `side_effect` defaults to False (safe default —
        # we'd rather refuse an unflagged endpoint than auto-allow it).
        self.assertFalse(eps[1]["side_effect"])


class ApiCallBrokerTests(unittest.TestCase):
    """0.8.6 — `api_call` MCP tool forwards to `POST /api/agent-api/call`.

    Pin the contract the agent relies on:
    - disc_id auto-injected from KRONN_DISCUSSION_ID (never trust the
      agent to pass it).
    - Either (api_plugin_slug + api_config_id) OR quick_api_id required.
    - Credentials never appear in the body (this is enforced by NOT
      offering a way to pass them — but the test confirms we don't
      accidentally allow `auth_*` / `api_key` / etc. pass-through).
    - Empty/missing endpoint_path is rejected before any HTTP call.
    """

    def setUp(self):
        self.mod = _load_module()
        # Pre-seed the disc-meta cache so the broker has a parent disc
        # to scope against — same shape as `setUp` in the other class
        # but a different parent.
        self.mod._CURRENT_DISC_META_CACHE.update({
            "checked": True,
            "value": {
                "id": "disc-front-eu",
                "project_id": "proj-front-eu",
                "agent": "ClaudeCode",
            },
        })
        # Make _disc_id() resolve without relying on env.
        self.env_patch = mock.patch.dict(
            os.environ, {"KRONN_DISCUSSION_ID": "disc-front-eu"}
        )
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)
        # Mock _http to capture the forwarded body.
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {
                "success": True,
                "duration_ms": 142,
                "data": {"items": [{"id": "EW-1"}]},
                "status": "OK",
                "summary": "GET /search → 1 result",
                "http_status": 200,
            },
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_forwards_disc_id_and_endpoint_payload_to_broker_route(self):
        """Happy path with plugin+config pair. The broker receives the
        disc_id auto-filled from env + the pass-through fields. No
        credentials appear in the body."""
        self.mod.call_api_call({
            "api_plugin_slug": "mcp-atlassian",
            "api_config_id": "cfg-jira",
            "endpoint_path": "/rest/api/3/search/jql",
            "method": "GET",
            "query": {"jql": "project = EW"},
        })
        method, path, body = self.fake_http.call_args.args
        self.assertEqual(method, "POST")
        self.assertEqual(path, "/api/agent-api/call")
        self.assertEqual(body["disc_id"], "disc-front-eu")
        self.assertEqual(body["api_plugin_slug"], "mcp-atlassian")
        self.assertEqual(body["api_config_id"], "cfg-jira")
        self.assertEqual(body["endpoint_path"], "/rest/api/3/search/jql")
        self.assertEqual(body["method"], "GET")
        self.assertEqual(body["query"], {"jql": "project = EW"})
        # CRITICAL: no credentials in the body. The broker must never
        # accept agent-supplied auth — credentials live in Kronn DB
        # only. We assert by NEGATIVE: none of the standard auth
        # field names leak through.
        for forbidden in ("auth", "api_key", "bearer", "authorization",
                          "secret", "password", "token"):
            self.assertNotIn(forbidden, body,
                f"agent-supplied `{forbidden}` MUST NOT pass through the broker")

    def test_supports_quick_api_id_path(self):
        """Alternative selector path: `quick_api_id` instead of
        slug+config. Same broker route, hydration happens server-side."""
        self.mod.call_api_call({
            "quick_api_id": "qa-jira-fetch",
            "endpoint_path": "/rest/api/3/issue/EW-7247",
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["quick_api_id"], "qa-jira-fetch")
        # No plugin slug forwarded when only QA ref is given.
        self.assertNotIn("api_plugin_slug", body)
        self.assertNotIn("api_config_id", body)

    def test_rejects_payload_with_neither_plugin_pair_nor_quick_api_id(self):
        """Without any plugin reference there's nothing to call. Must
        fail BEFORE the HTTP round-trip (free, no backend load) with
        a clear instruction pointing to `mcp_list`/`qa_list`."""
        with self.assertRaises(RuntimeError) as ctx:
            self.mod.call_api_call({"endpoint_path": "/whatever"})
        msg = str(ctx.exception)
        self.assertIn("mcp_list", msg)
        self.assertIn("qa_list", msg)
        # Backend was never hit.
        self.fake_http.assert_not_called()

    def test_rejects_payload_with_slug_but_no_config_id(self):
        """Half-filled plugin reference must be rejected — the backend
        executor needs BOTH the slug AND the config_id to decrypt the
        right env. (Same predicate as the Rust route's validator.)"""
        with self.assertRaises(RuntimeError):
            self.mod.call_api_call({
                "api_plugin_slug": "mcp-atlassian",
                # no api_config_id
                "endpoint_path": "/x",
            })
        self.fake_http.assert_not_called()

    def test_rejects_payload_without_endpoint_path(self):
        with self.assertRaises(RuntimeError) as ctx:
            self.mod.call_api_call({
                "api_plugin_slug": "mcp-atlassian",
                "api_config_id": "cfg-jira",
            })
        self.assertIn("endpoint_path", str(ctx.exception))
        self.fake_http.assert_not_called()

    def test_missing_disc_id_no_longer_blocks_host_cli_sessions(self):
        """0.8.6 relaxation: host-CLI sessions (no KRONN_DISCUSSION_ID
        in env) can still call the broker. Project scope is derived
        server-side from the chosen `api_config_id`'s project_ids
        (or from explicit `project_id` arg). Pre-fix the tool hard-
        rejected with `KRONN_DISCUSSION_ID env var not set` and locked
        out every host-CLI agent — caught live 2026-05-19 when a
        regular `claude` session couldn't call Didomi after the broker
        shipped."""
        with mock.patch.dict(os.environ, {}, clear=True):
            # Must NOT raise — the call proceeds, server resolves the project.
            self.mod.call_api_call({
                "api_plugin_slug": "mcp-atlassian",
                "api_config_id": "cfg-jira",
                "endpoint_path": "/x",
            })
        _, _, body = self.fake_http.call_args.args
        # disc_id is NOT in the body when missing — backend infers from config.
        self.assertNotIn("disc_id", body)
        self.assertEqual(body["api_config_id"], "cfg-jira")

    def test_explicit_project_id_passes_through(self):
        """Highest-priority scope override. Agent passes `project_id`
        explicitly when calling a global config and wanting attribution
        to a specific project, OR when overriding disc-derived scope."""
        self.mod.call_api_call({
            "api_plugin_slug": "mcp-atlassian",
            "api_config_id": "cfg-jira",
            "endpoint_path": "/x",
            "project_id": "proj-explicit-override",
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["project_id"], "proj-explicit-override")

    def test_optional_fields_propagated_only_when_non_none(self):
        """Passing only the required fields must NOT splatter `null`s
        into the body — the route uses `#[serde(default)]` everywhere
        so missing == default, but null-vs-missing would surface as a
        deserialization error on stricter setups. Keep the body lean."""
        self.mod.call_api_call({
            "api_plugin_slug": "mcp-atlassian",
            "api_config_id": "cfg",
            "endpoint_path": "/x",
        })
        _, _, body = self.fake_http.call_args.args
        # Only the 4 expected keys.
        self.assertEqual(
            set(body.keys()),
            {"disc_id", "api_plugin_slug", "api_config_id", "endpoint_path"},
        )

    def test_unwraps_broker_response_envelope(self):
        """The broker returns an ApiResponse wrapping the
        AgentApiCallResponse. The tool MUST unwrap and return the
        inner payload so the agent sees `data` / `status` / `summary`
        directly — not a nested `{success, data: {…}}`."""
        result = self.mod.call_api_call({
            "api_plugin_slug": "mcp-atlassian",
            "api_config_id": "cfg",
            "endpoint_path": "/x",
        })
        # _unwrap stripped the outer envelope.
        self.assertEqual(result["status"], "OK")
        self.assertEqual(result["http_status"], 200)
        self.assertEqual(result["data"], {"items": [{"id": "EW-1"}]})


class RuntimeDiscBindingTests(unittest.TestCase):
    """0.8.6 phase 2 — `_CURRENT_DISC_ID` mutable binding.

    Before phase 2 the bridge could only learn its disc via the
    `KRONN_DISCUSSION_ID` env at boot. Phase 2 adds a runtime setter
    so `disc_join({token})` can bind a host-launched CLI to a Kronn
    disc without restarting the process. These tests lock the new
    contract :

      * env present at boot → `_CURRENT_DISC_ID` reflects it
      * env absent → `_disc_id()` raises an actionable error mentioning
        BOTH the env-var path AND the `disc_join` path
      * `_set_current_disc_id(x)` mutates the binding live + invalidates
        the meta cache so the next read goes to the new disc
      * `_set_current_disc_id(None)` clears the binding (used by
        `disc_leave`)
    """

    def test_boot_with_env_sets_current_disc_id(self):
        with mock.patch.dict(os.environ, {"KRONN_DISCUSSION_ID": "disc-from-env"}):
            mod = _load_module()
        self.assertEqual(mod._CURRENT_DISC_ID, "disc-from-env")
        # And `_disc_id()` returns it.
        self.assertEqual(mod._disc_id(), "disc-from-env")

    def test_boot_without_env_leaves_current_disc_id_none(self):
        # Strip the env var explicitly even if the host shell has one set.
        env_clean = {k: v for k, v in os.environ.items() if k != "KRONN_DISCUSSION_ID"}
        with mock.patch.dict(os.environ, env_clean, clear=True):
            mod = _load_module()
        self.assertIsNone(mod._CURRENT_DISC_ID)

    def test_disc_id_raises_actionable_error_when_unbound(self):
        env_clean = {k: v for k, v in os.environ.items() if k != "KRONN_DISCUSSION_ID"}
        with mock.patch.dict(os.environ, env_clean, clear=True):
            mod = _load_module()
        with self.assertRaises(RuntimeError) as cm:
            mod._disc_id()
        msg = str(cm.exception)
        # The error MUST surface both ways to bind (env + disc_join), so the
        # caller (or the agent inspecting the error) knows the fix.
        self.assertIn("KRONN_DISCUSSION_ID", msg)
        self.assertIn("disc_join", msg)

    def test_set_current_disc_id_mutates_binding_at_runtime(self):
        env_clean = {k: v for k, v in os.environ.items() if k != "KRONN_DISCUSSION_ID"}
        with mock.patch.dict(os.environ, env_clean, clear=True):
            mod = _load_module()
        # Initially unbound.
        self.assertIsNone(mod._CURRENT_DISC_ID)
        # Runtime bind.
        mod._set_current_disc_id("disc-runtime-joined")
        self.assertEqual(mod._CURRENT_DISC_ID, "disc-runtime-joined")
        self.assertEqual(mod._disc_id(), "disc-runtime-joined")

    def test_set_current_disc_id_invalidates_meta_cache(self):
        with mock.patch.dict(os.environ, {"KRONN_DISCUSSION_ID": "disc-old"}):
            mod = _load_module()
        # Pre-populate the cache as if we already looked up disc-old.
        mod._CURRENT_DISC_META_CACHE["checked"] = True
        mod._CURRENT_DISC_META_CACHE["value"] = {
            "id": "disc-old",
            "project_id": "p-old",
            "agent": "ClaudeCode",
        }
        # Switching disc must clear the stale meta — otherwise the next
        # `_current_project_id()` would return p-old for the new disc.
        mod._set_current_disc_id("disc-new")
        self.assertFalse(mod._CURRENT_DISC_META_CACHE["checked"])
        self.assertIsNone(mod._CURRENT_DISC_META_CACHE["value"])

    def test_set_current_disc_id_none_clears_binding(self):
        with mock.patch.dict(os.environ, {"KRONN_DISCUSSION_ID": "disc-X"}):
            mod = _load_module()
        self.assertEqual(mod._CURRENT_DISC_ID, "disc-X")
        # disc_leave path (later wave) clears.
        mod._set_current_disc_id(None)
        self.assertIsNone(mod._CURRENT_DISC_ID)
        with self.assertRaises(RuntimeError):
            mod._disc_id()


class DiscJoinTests(unittest.TestCase):
    """0.8.6 phase 2 — `disc_join` MCP tool.

    The tool POSTs to `/api/discussions/peer-join`, validates the
    invite token, and (on success) mutates `_CURRENT_DISC_ID` so
    every subsequent `disc_*` tool resolves to the joined disc. We
    mock the `_http` boundary so the test is pure — no live backend
    required.

    These lock the contract :
      * happy path : `_CURRENT_DISC_ID` bound + response returned
      * missing token → typed RuntimeError before any HTTP call
      * backend error response → `_CURRENT_DISC_ID` left UNCHANGED
      * env override : KRONN_AGENT_TYPE / KRONN_SESSION_ID surface
        in the POST body
    """

    def setUp(self):
        # Start every test with no disc bound — covers the host-launched
        # CLI scenario (no env injection).
        env_clean = {
            k: v for k, v in os.environ.items()
            if k not in ("KRONN_DISCUSSION_ID", "KRONN_AGENT_TYPE", "KRONN_SESSION_ID")
        }
        self.env_patch = mock.patch.dict(os.environ, env_clean, clear=True)
        self.env_patch.start()
        self.mod = _load_module()
        # Sanity : the module starts unbound.
        self.assertIsNone(self.mod._CURRENT_DISC_ID)

    def tearDown(self):
        self.env_patch.stop()

    def test_happy_path_binds_disc_and_returns_payload(self):
        # Mock the parent-process cmdline lookup — the 2026-05-21
        # fallback would otherwise pick up the test runner's parent
        # (claude/pytest/etc) and inject a real agent_type, defeating
        # the "no useful detection → Unknown" intent of this test.
        with mock.patch.object(self.mod, "_parent_process_cmdline", return_value=None), \
             mock.patch.object(self.mod, "_http") as mock_http:
            mock_http.return_value = {
                "success": True,
                "data": {
                    "disc_id": "d-from-token",
                    "session_pk": 42,
                    "peer_count": 1,
                    "disc_title": "RGPD audit",
                    "recent_messages": [],
                },
            }
            result = self.mod.call_disc_join({"token": "kr-join-abc"})

        # Bound.
        self.assertEqual(self.mod._CURRENT_DISC_ID, "d-from-token")
        # Now `_disc_id()` returns the joined disc.
        self.assertEqual(self.mod._disc_id(), "d-from-token")
        # Returned payload matches the data field.
        self.assertEqual(result["disc_id"], "d-from-token")
        self.assertEqual(result["peer_count"], 1)
        # Backend was called with the token + synthesised agent_type / session_id.
        called_path = mock_http.call_args[0][1]
        called_body = mock_http.call_args[0][2]
        self.assertEqual(called_path, "/api/discussions/peer-join")
        self.assertEqual(called_body["token"], "kr-join-abc")
        self.assertEqual(called_body["agent_type"], "Unknown")
        self.assertTrue(called_body["session_id"].startswith("adhoc-"))

    def test_missing_token_raises_before_any_http_call(self):
        with mock.patch.object(self.mod, "_http") as mock_http:
            with self.assertRaises(RuntimeError) as cm:
                self.mod.call_disc_join({})
            mock_http.assert_not_called()
        msg = str(cm.exception)
        self.assertIn("token", msg)
        self.assertIn("kr-join", msg, "error should hint at the token format")
        # Still unbound.
        self.assertIsNone(self.mod._CURRENT_DISC_ID)

    def test_backend_rejection_leaves_current_disc_unbound(self):
        # _unwrap raises on `success=false`. The disc binding must NOT
        # change if the backend rejects the token.
        with mock.patch.object(self.mod, "_http") as mock_http:
            mock_http.return_value = {
                "success": False,
                "error": "invite token already used",
            }
            with self.assertRaises(RuntimeError):
                self.mod.call_disc_join({"token": "kr-join-stale"})
        self.assertIsNone(
            self.mod._CURRENT_DISC_ID,
            "rejected join must not leave a phantom binding",
        )

    def test_env_overrides_propagate_to_post_body(self):
        # When the agent supplies its own identity via env (Kronn-launched
        # case or a wrapper script setting these vars), the bridge MUST
        # forward them so the `discussion_sessions` row carries the right
        # agent_type / session_id.
        with mock.patch.dict(os.environ, {
            "KRONN_AGENT_TYPE": "Codex",
            "KRONN_SESSION_ID": "sess-codex-real",
        }):
            mod2 = _load_module()
            with mock.patch.object(mod2, "_http") as mock_http:
                mock_http.return_value = {
                    "success": True,
                    "data": {"disc_id": "d", "session_pk": 1, "peer_count": 1, "disc_title": "x", "recent_messages": []},
                }
                mod2.call_disc_join({"token": "kr-join-z"})
            body = mock_http.call_args[0][2]
            self.assertEqual(body["agent_type"], "Codex")
            self.assertEqual(body["session_id"], "sess-codex-real")


class DiscAppendSimpleModeTests(unittest.TestCase):
    """0.8.6 fix 2026-05-21 — ergonomic simple-mode for `disc_append`.

    Before this, the tool only accepted `messages: [{source_msg_id,
    role, content, agent_type}, …]` (heavy mode for transcript
    import). The simple `disc_append({content: "Hi"})` form that the
    multi-agent collab UX guides agents toward FAILED with "missing
    required 'messages'". These tests lock the new simple-mode :
      - content alone → bridge wraps it + auto-fills the rest
      - disc_id defaults to runtime-bound disc
      - agent_type derives from clientInfo
      - heavy mode (messages array) still works for back-compat
      - role override + agent_type override propagate
      - missing both content AND messages → clear error
    """

    def setUp(self):
        self.env_patch = mock.patch.dict(
            os.environ,
            {"KRONN_DISCUSSION_ID": "disc-chat"},
            clear=False,
        )
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)
        self.mod = _load_module()
        # Pretend Codex is the calling CLI via the captured clientInfo.
        self.mod._CLIENT_INFO["name"] = "codex-cli"

    def test_simple_mode_content_alone_succeeds(self):
        with mock.patch.object(self.mod, "_http") as mock_http:
            mock_http.return_value = {
                "success": True,
                "data": {"appended": 1, "skipped_as_duplicates": 0, "diverged": False},
            }
            result = self.mod.call_disc_append({"content": "ready to play"})
        body = mock_http.call_args[0][2]
        self.assertEqual(body["disc_id"], "disc-chat")
        self.assertEqual(len(body["messages"]), 1)
        msg = body["messages"][0]
        self.assertEqual(msg["content"], "ready to play")
        self.assertEqual(msg["role"], "Agent")
        self.assertEqual(msg["agent_type"], "Codex")
        # source_msg_id auto-generated with `live-` prefix for traceability.
        self.assertTrue(msg["source_msg_id"].startswith("live-"))
        self.assertEqual(result["appended"], 1)

    def test_simple_mode_role_and_agent_type_overrides_propagate(self):
        with mock.patch.object(self.mod, "_http") as mock_http:
            mock_http.return_value = {"success": True, "data": {}}
            self.mod.call_disc_append({
                "content": "hi",
                "role": "User",
                "agent_type": "ManualOverride",
            })
        msg = mock_http.call_args[0][2]["messages"][0]
        self.assertEqual(msg["role"], "User")
        self.assertEqual(msg["agent_type"], "ManualOverride")

    def test_heavy_mode_messages_array_still_works(self):
        # Back-compat : the 0.8.4 cross-agent-memory transcript import
        # path MUST keep working. Agents that pass an explicit messages
        # array bypass the auto-fill.
        with mock.patch.object(self.mod, "_http") as mock_http:
            mock_http.return_value = {"success": True, "data": {}}
            self.mod.call_disc_append({
                "disc_id": "disc-other",
                "messages": [
                    {"source_msg_id": "abc-1", "role": "Agent", "content": "first"},
                    {"source_msg_id": "abc-2", "role": "User", "content": "second"},
                ],
            })
        body = mock_http.call_args[0][2]
        self.assertEqual(body["disc_id"], "disc-other")
        self.assertEqual(len(body["messages"]), 2)
        self.assertEqual(body["messages"][0]["source_msg_id"], "abc-1")

    def test_neither_content_nor_messages_errors_with_clear_hint(self):
        with mock.patch.object(self.mod, "_http") as mock_http:
            with self.assertRaises(RuntimeError) as cm:
                self.mod.call_disc_append({})
            mock_http.assert_not_called()
        msg = str(cm.exception)
        # Error must mention BOTH modes so the agent knows how to fix.
        self.assertIn("content", msg)
        self.assertIn("messages", msg)


class DiscLeaveTests(unittest.TestCase):
    """0.8.6 phase 3 — `disc_leave` MCP tool."""

    def setUp(self):
        self.env_patch = mock.patch.dict(
            os.environ,
            {
                "KRONN_DISCUSSION_ID": "disc-bye",
                "KRONN_AGENT_TYPE": "Codex",
                "KRONN_SESSION_ID": "sess-bye",
            },
            clear=False,
        )
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)
        self.mod = _load_module()

    def test_clears_local_binding_and_posts_leave(self):
        with mock.patch.object(self.mod, "_http") as mock_http:
            mock_http.return_value = {
                "success": True,
                "data": {"left": True},
            }
            # Sanity : we start bound.
            self.assertEqual(self.mod._CURRENT_DISC_ID, "disc-bye")
            result = self.mod.call_disc_leave({})

        self.assertEqual(result, {"left": True})
        # Bridge cleared its local binding.
        self.assertIsNone(self.mod._CURRENT_DISC_ID)
        # Body forwarded the agent identity.
        called_body = mock_http.call_args[0][2]
        self.assertEqual(called_body["agent_type"], "Codex")
        self.assertEqual(called_body["session_id"], "sess-bye")

    def test_clears_local_binding_even_when_backend_unreachable(self):
        # If the backend is down or 500s, the bridge MUST still clear
        # its local `_CURRENT_DISC_ID` so the next `disc_*` tool isn't
        # stuck targeting a disc the user wanted to leave. The error
        # bubbles up so the caller knows the leave wasn't recorded
        # server-side.
        with mock.patch.object(self.mod, "_http", side_effect=RuntimeError("network down")):
            with self.assertRaises(RuntimeError):
                self.mod.call_disc_leave({})
        self.assertIsNone(self.mod._CURRENT_DISC_ID)


class ClientInfoAutoDetectTests(unittest.TestCase):
    """0.8.6 fix 2026-05-21 — auto-derive agent_type from MCP clientInfo.

    Without this, peers showed up as 'Unknown' in the header because
    host-launched CLIs don't naturally set `KRONN_AGENT_TYPE` env.
    The fix captures `clientInfo.name` from the initialize handshake
    and maps it to the canonical AgentType.
    """

    def setUp(self):
        env_clean = {
            k: v for k, v in os.environ.items()
            if k not in ("KRONN_AGENT_TYPE", "KRONN_CALLER_AGENT")
        }
        self.env_patch = mock.patch.dict(os.environ, env_clean, clear=True)
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)
        self.mod = _load_module()

    def test_infer_agent_type_from_known_clients(self):
        cases = {
            "claude-code": "ClaudeCode",
            "Claude Code": "ClaudeCode",
            "codex-cli": "Codex",
            "codex": "Codex",
            "gemini-cli": "GeminiCli",
            "Kiro": "Kiro",
            "kiro-cli": "Kiro",
            "copilot-cli": "CopilotCli",
            "vibe": "Vibe",
            "vibe-cli": "Vibe",
            "cursor": "Custom",
            "cline": "Custom",
            "totally-unknown-cli": "Unknown",
            "": "Unknown",
            None: "Unknown",
        }
        for client_name, expected in cases.items():
            got = self.mod._infer_agent_type_from_client_name(client_name)
            self.assertEqual(
                got, expected,
                f"clientInfo.name={client_name!r} → expected {expected}, got {got}",
            )

    def test_initialize_captures_client_info(self):
        # Replay the initialize request the way a real CLI would.
        resp = self.mod._handle({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": {"name": "claude-code", "version": "1.2.3"},
            },
        })
        self.assertEqual(resp["result"]["serverInfo"]["name"], "kronn-internal")
        # Server-level precedence guidance (reuse QA → construct → persist) so
        # the token-economy ordering doesn't depend on which tool desc the model
        # happens to read.
        instr = resp["result"]["instructions"]
        for needle in ("qa_list", "qa_run", "qa_create_draft"):
            self.assertIn(needle, instr)
        # And the side-effect : clientInfo stashed for downstream tools.
        self.assertEqual(self.mod._CLIENT_INFO["name"], "claude-code")
        self.assertEqual(self.mod._CLIENT_INFO["version"], "1.2.3")

    def test_api_call_description_links_the_qa_reuse_loop(self):
        # api_call is the hub: it must point BACKWARD (reuse a saved QA before
        # rebuilding) and FORWARD (persist a hand-built call as a QA), so agents
        # don't reconstruct the same payload every session.
        tool = next(t for t in self.mod.TOOLS if t["name"] == "api_call")
        desc = tool["description"]
        self.assertIn("qa_list", desc)          # reuse first
        self.assertIn("qa_run", desc)
        self.assertIn("qa_create_draft", desc)  # persist after

    def test_agent_type_for_session_explicit_env_wins_over_inferred(self):
        # When KRONN_AGENT_TYPE is set explicitly (wrapper script, test
        # harness, etc.), it overrides the auto-detection — useful for
        # advanced users who want to surface a Custom subtype.
        with mock.patch.dict(os.environ, {"KRONN_AGENT_TYPE": "OverriddenAgent"}):
            self.mod._CLIENT_INFO["name"] = "claude-code"
            self.assertEqual(self.mod._agent_type_for_session(), "OverriddenAgent")

    def test_agent_type_for_session_uses_client_info_when_no_env(self):
        # The headline fix : no env vars + clientInfo from handshake
        # → derives ClaudeCode (was returning Unknown before 2026-05-21).
        self.mod._CLIENT_INFO["name"] = "claude-code"
        self.assertEqual(self.mod._agent_type_for_session(), "ClaudeCode")

    def test_agent_type_for_session_falls_through_to_unknown(self):
        # No env, no clientInfo, no useful parent cmdline → Unknown.
        # The peer still joins (the backend doesn't reject Unknown),
        # the header just shows a generic chip. Better than crashing.
        # We mock _parent_process_cmdline because the test runner's
        # parent process (Claude Code Bash, pytest, etc.) might
        # contain a matchable name and accidentally pass.
        self.mod._CLIENT_INFO["name"] = None
        with mock.patch.object(
            self.mod, "_parent_process_cmdline",
            return_value=None,
        ):
            self.assertEqual(self.mod._agent_type_for_session(), "Unknown")

    def test_parent_cmdline_fallback_kicks_in_when_clientinfo_useless(self):
        # 2026-05-21 fix : Vibe's MCP client doesn't always send a
        # name we can match → fall back to /proc/<PPID>/cmdline. Mock
        # `_parent_process_cmdline` to simulate a Vibe parent process.
        self.mod._CLIENT_INFO["name"] = None  # clientInfo gave us nothing
        with mock.patch.object(
            self.mod, "_parent_process_cmdline",
            return_value="/usr/local/bin/vibe --some-flag",
        ):
            self.assertEqual(self.mod._agent_type_for_session(), "Vibe")

    def test_unknown_fallback_when_neither_clientinfo_nor_cmdline_helps(self):
        # Final guard : nothing we can do, return Unknown rather than
        # crashing. The session row still gets created server-side.
        self.mod._CLIENT_INFO["name"] = "totally-mystery-cli"
        with mock.patch.object(
            self.mod, "_parent_process_cmdline",
            return_value="/usr/bin/totally-mystery-cli --foo",
        ):
            self.assertEqual(self.mod._agent_type_for_session(), "Unknown")

    def test_disc_join_uses_inferred_agent_type_when_env_absent(self):
        # End-to-end : initialize handshake → clientInfo captured →
        # disc_join body carries the right agent_type without
        # KRONN_AGENT_TYPE being set.
        self.mod._handle({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {"name": "codex-cli", "version": "0.132.0"}},
        })
        with mock.patch.object(self.mod, "_http") as mock_http:
            mock_http.return_value = {
                "success": True,
                "data": {"disc_id": "d", "session_pk": 1, "peer_count": 1, "disc_title": "x", "recent_messages": []},
            }
            self.mod.call_disc_join({"token": "kr-join-test"})
        body = mock_http.call_args[0][2]
        self.assertEqual(
            body["agent_type"], "Codex",
            "disc_join should derive 'Codex' from clientInfo.name='codex-cli'",
        )


class StableSessionIdAcrossCallsTests(unittest.TestCase):
    """0.8.6 fix 2026-05-21 — `_session_id_for_caller` returns the
    same id for the entire bridge process lifetime.

    Before the fix, `disc_join` and `disc_leave` each generated a
    fresh `adhoc-<uuid>` so `find_active_session(agent_type,
    session_id)` missed and `disc_leave` returned `left: false`.
    Caught live on the 3-agent tennis match (Claude + Codex both
    surfaced `left: false` in their transcripts). These tests lock
    the stability contract.
    """

    def setUp(self):
        env_clean = {
            k: v for k, v in os.environ.items()
            if k not in ("KRONN_SESSION_ID", "KRONN_CALLER_SESSION_ID")
        }
        self.env_patch = mock.patch.dict(os.environ, env_clean, clear=True)
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)
        self.mod = _load_module()

    def test_session_id_is_identical_across_calls(self):
        # Hammer the helper a few times — same value every call.
        first = self.mod._session_id_for_caller()
        for _ in range(5):
            self.assertEqual(self.mod._session_id_for_caller(), first)

    def test_session_id_starts_with_adhoc_when_no_env(self):
        sid = self.mod._session_id_for_caller()
        self.assertTrue(
            sid.startswith("adhoc-"),
            f"host-launched bridge should fall back to adhoc- prefix, got {sid!r}",
        )

    # PR 118's "the adhoc id must survive a reload" premise is obsolete as of
    # 0.8.13 (and was environment-fragile: an unreadable parent start-token
    # takes the uuid fallback, so two loads legitimately differ). Reload
    # continuity is now the resume credential's job — covered by
    # ResumeBindingTests.test_reload_with_new_bridge_id_resumes_same_room.

    def test_adhoc_id_differs_for_a_different_parent_instance(self):
        # Two genuinely different CLIs must NOT collapse into one
        # participant. `_parent_start_token` is mocked non-None so the
        # assertion locks the ppid+token DERIVATION — an unreadable
        # parent would silently take the uuid fallback and pass anyway.
        with mock.patch.object(self.mod, "_parent_start_token", return_value="tok-A"):
            with mock.patch.object(self.mod.os, "getppid", return_value=1111):
                id_a = self.mod._resolve_bridge_session_id()
            with mock.patch.object(self.mod.os, "getppid", return_value=2222):
                id_b = self.mod._resolve_bridge_session_id()
        self.assertEqual(id_a, "adhoc-1111-tok-A", "deterministic ppid+token form")
        self.assertEqual(id_b, "adhoc-2222-tok-A")
        self.assertNotEqual(id_a, id_b, "different parent ⇒ different adhoc session id")

    def test_recycled_pid_with_a_new_start_token_is_a_new_identity(self):
        # Same pid after a reboot/pid-recycle: the start token is what
        # disambiguates — same ppid, different token ⇒ different id.
        with mock.patch.object(self.mod.os, "getppid", return_value=1111):
            with mock.patch.object(self.mod, "_parent_start_token", return_value="boot-1"):
                id_a = self.mod._resolve_bridge_session_id()
            with mock.patch.object(self.mod, "_parent_start_token", return_value="boot-2"):
                id_b = self.mod._resolve_bridge_session_id()
        self.assertNotEqual(id_a, id_b, "recycled pid ⇒ still a distinct identity")

    def test_adhoc_id_falls_back_to_uuid_when_parent_is_unreadable(self):
        with mock.patch.object(self.mod, "_parent_start_token", return_value=None):
            sid_a = self.mod._resolve_bridge_session_id()
            sid_b = self.mod._resolve_bridge_session_id()
        self.assertTrue(sid_a.startswith("adhoc-"))
        self.assertNotEqual(sid_a, sid_b, "uuid fallback stays random per resolution")

    def test_env_session_id_still_wins_over_the_parent_identity(self):
        # Priority contract unchanged: Kronn-launched agents keep their
        # injected KRONN_SESSION_ID regardless of parent-derived ids.
        with mock.patch.dict(os.environ, {"KRONN_SESSION_ID": "kronn-injected-9"}):
            self.assertEqual(self.mod._resolve_bridge_session_id(), "kronn-injected-9")

    def test_session_id_picks_up_env_when_set_at_module_load(self):
        with mock.patch.dict(os.environ, {"KRONN_SESSION_ID": "kronn-launched-123"}):
            mod2 = _load_module()
        self.assertEqual(mod2._session_id_for_caller(), "kronn-launched-123")

    def test_disc_join_and_disc_leave_send_the_same_session_id(self):
        # The headline regression : the two tool calls MUST forward
        # the same `session_id` so the backend's find_active_session
        # query hits on disc_leave.
        with mock.patch.dict(os.environ, {"KRONN_DISCUSSION_ID": "disc-stable"}):
            mod = _load_module()
            mod._CLIENT_INFO["name"] = "codex-cli"
            with mock.patch.object(mod, "_http") as mock_http:
                mock_http.return_value = {
                    "success": True,
                    "data": {
                        "disc_id": "d", "session_pk": 1, "peer_count": 1,
                        "disc_title": "x", "recent_messages": [],
                    },
                }
                mod.call_disc_join({"token": "kr-join-x"})
                join_sid = mock_http.call_args[0][2]["session_id"]

                mock_http.return_value = {"success": True, "data": {"left": True}}
                mod.call_disc_leave({})
                leave_sid = mock_http.call_args[0][2]["session_id"]

            self.assertEqual(
                join_sid, leave_sid,
                "disc_join and disc_leave must share the same session_id, "
                "otherwise find_active_session misses and left=false",
            )


class DiscWaitForPeerTests(unittest.TestCase):
    """0.8.6 phase 3 — `disc_wait_for_peer` MCP tool.

    Bridge-side mechanics : forwards `since_sort_order` + `timeout_secs`
    in the query string, derives `exclude_agent_type` from the
    `KRONN_AGENT_TYPE` (or `KRONN_CALLER_AGENT`) env so the agent
    doesn't wake itself, hits `_disc_id()` so an unbound bridge
    raises before any HTTP attempt.
    """

    def setUp(self):
        self.env_patch = mock.patch.dict(
            os.environ,
            {
                "KRONN_DISCUSSION_ID": "disc-poll",
                "KRONN_AGENT_TYPE": "Codex",
            },
            clear=False,
        )
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)
        self.mod = _load_module()

    def test_transport_cut_is_retried_with_the_same_request(self):
        # Passe stab-1 — a backend restart (cargo watch) mid-poll used to
        # surface as a tool error and drop the agent out of the room. The
        # bridge now retries transport failures, SAME query string (the
        # since_sort_order makes the resume idempotent).
        import urllib.error
        ok = {
            "success": True,
            "data": {"timed_out": True, "messages": [], "latest_sort_order": 12},
        }
        calls = []

        def flaky(method, path, body=None):
            calls.append(path)
            if len(calls) < 3:
                raise urllib.error.URLError(ConnectionRefusedError(61, "refused"))
            return ok

        with mock.patch.object(self.mod, "_http", side_effect=flaky), \
             mock.patch.object(self.mod.time, "sleep") as mock_sleep:
            result = self.mod.call_disc_wait_for_peer({"since_sort_order": 12})
        self.assertEqual(len(calls), 3, "two failures then success")
        self.assertTrue(all(p == calls[0] for p in calls), "identical request each attempt")
        self.assertTrue(result["timed_out"])
        self.assertEqual(mock_sleep.call_count, 2, "bounded backoff between attempts")

    def test_transport_retry_is_bounded_and_names_the_resume_contract(self):
        import urllib.error
        with mock.patch.object(
            self.mod, "_http",
            side_effect=urllib.error.URLError(ConnectionRefusedError(61, "refused")),
        ) as mock_http, mock.patch.object(self.mod.time, "sleep"):
            with self.assertRaises(RuntimeError) as ctx:
                self.mod.call_disc_wait_for_peer({"since_sort_order": 5})
        self.assertEqual(mock_http.call_count, 6, "bounded — never infinite")
        msg = str(ctx.exception)
        self.assertIn("since_sort_order", msg, "the error must teach the resume contract")
        self.assertIn("unreachable", msg)

    def test_http_application_errors_are_never_retried(self):
        # A 4xx/5xx is an app-level answer, not a transport cut — retrying
        # would hammer the backend and mask real errors.
        with mock.patch.object(
            self.mod, "_http",
            side_effect=RuntimeError("HTTP 404: nope"),
        ) as mock_http, mock.patch.object(self.mod.time, "sleep") as mock_sleep:
            with self.assertRaises(RuntimeError):
                self.mod.call_disc_wait_for_peer({"since_sort_order": 5})
        self.assertEqual(mock_http.call_count, 1, "no retry on HTTP errors")
        mock_sleep.assert_not_called()

    def test_forwards_since_and_timeout_in_query_string(self):
        with mock.patch.object(self.mod, "_http") as mock_http:
            mock_http.return_value = {
                "success": True,
                "data": {"timed_out": True, "messages": [], "latest_sort_order": 12},
            }
            self.mod.call_disc_wait_for_peer({
                "since_sort_order": 12,
                "timeout_secs": 30,
            })
        called_method, called_path = mock_http.call_args[0][:2]
        self.assertEqual(called_method, "GET")
        self.assertIn("/api/discussions/disc-poll/wait?", called_path)
        self.assertIn("since_sort_order=12", called_path)
        self.assertIn("timeout_secs=30", called_path)
        # Exclude propagated from KRONN_AGENT_TYPE so we don't wake self.
        self.assertIn("exclude_agent_type=Codex", called_path)

    def test_no_query_params_when_args_omitted(self):
        # When the agent calls disc_wait_for_peer() with no args, the
        # endpoint applies its own defaults (since=-1, timeout=60). The
        # tool MUST still forward `exclude_agent_type` though.
        with mock.patch.object(self.mod, "_http") as mock_http:
            mock_http.return_value = {
                "success": True,
                "data": {"timed_out": True, "messages": [], "latest_sort_order": -1},
            }
            self.mod.call_disc_wait_for_peer({})
        path = mock_http.call_args[0][1]
        self.assertNotIn("since_sort_order", path)
        self.assertNotIn("timeout_secs", path)
        self.assertIn("exclude_agent_type=Codex", path)

    def test_happy_path_returns_messages_envelope(self):
        # Verify the bridge correctly unwraps the backend envelope and
        # returns `{timed_out, messages, latest_sort_order}` to the agent.
        # Without this, an envelope-shape change server-side would slip
        # through silently.
        with mock.patch.object(self.mod, "_http") as mock_http:
            mock_http.return_value = {
                "success": True,
                "data": {
                    "timed_out": False,
                    "messages": [
                        {
                            "sort_order": 3,
                            "role": "Agent",
                            "agent_type": "ClaudeCode",
                            "content": "hello peer",
                            "timestamp": "2026-05-20T10:00:00Z",
                        }
                    ],
                    "latest_sort_order": 3,
                },
            }
            result = self.mod.call_disc_wait_for_peer({
                "since_sort_order": 0,
                "timeout_secs": 5,
            })
        self.assertEqual(result["timed_out"], False)
        self.assertEqual(result["latest_sort_order"], 3)
        self.assertEqual(len(result["messages"]), 1)
        self.assertEqual(result["messages"][0]["content"], "hello peer")
        self.assertEqual(result["messages"][0]["agent_type"], "ClaudeCode")

    def test_happy_path_timed_out_returns_empty_messages(self):
        # The other terminal state : the long-poll fired the timeout
        # without any new peer activity. The agent gets timed_out=true
        # and can either retry or surface "no activity" to the user.
        with mock.patch.object(self.mod, "_http") as mock_http:
            mock_http.return_value = {
                "success": True,
                "data": {
                    "timed_out": True,
                    "messages": [],
                    "latest_sort_order": 7,
                },
            }
            result = self.mod.call_disc_wait_for_peer({
                "since_sort_order": 7,
                "timeout_secs": 5,
            })
        self.assertTrue(result["timed_out"])
        self.assertEqual(result["messages"], [])
        # latest_sort_order echoes the input on timeout so the agent
        # can keep calling without losing its cursor.
        self.assertEqual(result["latest_sort_order"], 7)
        # A timeout now carries an explicit next-action hint so literal agents
        # (notably Codex) keep waiting instead of treating it as end-of-convo
        # and stopping after ~60s.
        self.assertIn("hint", result)
        self.assertIn("again", result["hint"].lower())
        self.assertIn("disc_wait_for_peer", result["hint"])

    def test_unbound_disc_raises_before_http(self):
        with mock.patch.dict(os.environ, {
            k: v for k, v in os.environ.items() if k != "KRONN_DISCUSSION_ID"
        }, clear=True):
            mod = _load_module()
            with mock.patch.object(mod, "_http") as mock_http:
                with self.assertRaises(RuntimeError):
                    mod.call_disc_wait_for_peer({})
                mock_http.assert_not_called()


class DiscInvitePeerTests(unittest.TestCase):
    """0.8.6 (#56) — `disc_invite_peer` mints an invite via the existing
    `/api/discussions/:id/invite-peer` route. The MCP tool just forwards;
    the bridge contract is:

      * disc_id auto-pulled from the runtime binding (no agent-supplied id)
      * single round trip, body empty, returns the route's payload as-is
      * unbound disc → raise BEFORE any HTTP call so the agent's error
        message points at `disc_join`/`disc_create_room` rather than at
        a 404 from the backend.
    """

    def setUp(self):
        self.mod = _load_module()
        self.env_patch = mock.patch.dict(
            os.environ, {"KRONN_DISCUSSION_ID": "disc-room-1"}
        )
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {
                "token": "kr-join-fixture-abc",
                "instruction_text": "Lance `disc_join(token=\"kr-join-fixture-abc\")`",
                "expires_at": "2026-05-21T10:00:00Z",
                "ttl_seconds": 600,
            },
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_forwards_to_invite_peer_route_for_current_disc(self):
        result = self.mod.call_disc_invite_peer({})
        method, path, body = self.fake_http.call_args.args
        self.assertEqual(method, "POST")
        self.assertEqual(path, "/api/discussions/disc-room-1/invite-peer")
        self.assertEqual(body, {})
        self.assertEqual(result["token"], "kr-join-fixture-abc")
        self.assertEqual(result["ttl_seconds"], 600)

    def test_unbound_disc_raises_before_http(self):
        with mock.patch.dict(os.environ, {
            k: v for k, v in os.environ.items() if k != "KRONN_DISCUSSION_ID"
        }, clear=True):
            mod = _load_module()
            with mock.patch.object(mod, "_http") as mock_http:
                with self.assertRaises(RuntimeError):
                    mod.call_disc_invite_peer({})
                mock_http.assert_not_called()


class DiscCreateRoomTests(unittest.TestCase):
    """0.8.6 (#56) — `disc_create_room` chains disc_create + invite-peer
    so an agent can bootstrap a multi-agent room in a single tool call.

    Locked guarantees :
      * missing `title` raises immediately (no HTTP, clear message)
      * happy path returns a flat payload exposing both disc_id and
        token + instruction_text from the second hop
      * uses `_agent_type_for_session()` for the `agent` field (so the
        Kronn UI shows the right CLI in the participants header from t0)
      * invite-peer is hit at the disc_id RETURNED by disc_create, not
        at any agent-passed id — closes a tampering surface.
    """

    def setUp(self):
        self.mod = _load_module()
        # Ensure a stable agent_type for the auto-fill.
        self.atype_patch = mock.patch.object(
            self.mod, "_agent_type_for_session", return_value="ClaudeCode"
        )
        self.atype_patch.start()
        self.addCleanup(self.atype_patch.stop)
        # Two-call sequence: disc_create then invite-peer.
        self.responses = [
            # disc_create → wrapper used inside call_disc_create.
            {
                "success": True,
                "data": {
                    "disc_id": "disc-newly-spawned",
                    "title": "Live multi-agent room",
                    "agent": "ClaudeCode",
                },
            },
            # invite-peer → second hop.
            {
                "success": True,
                "data": {
                    "token": "kr-join-spawn-xyz",
                    "instruction_text": "Lance `disc_join(token=\"kr-join-spawn-xyz\")`",
                    "expires_at": "2026-05-21T10:00:00Z",
                    "ttl_seconds": 600,
                },
            },
        ]
        self.fake_http = mock.MagicMock(side_effect=self.responses)
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_missing_title_raises_before_http(self):
        with self.assertRaises(RuntimeError) as ctx:
            self.mod.call_disc_create_room({})
        self.assertIn("title", str(ctx.exception))
        self.fake_http.assert_not_called()

    def test_happy_path_returns_flat_disc_id_token_payload(self):
        result = self.mod.call_disc_create_room({
            "title": "Live multi-agent room",
            "language": "en",
        })
        self.assertEqual(self.fake_http.call_count, 2)
        # Step 1 — disc_create route.
        create_call = self.fake_http.call_args_list[0]
        self.assertEqual(create_call.args[0], "POST")
        # Step 2 — invite-peer at the RETURNED disc_id (not user-supplied).
        invite_call = self.fake_http.call_args_list[1]
        self.assertEqual(invite_call.args[0], "POST")
        self.assertEqual(
            invite_call.args[1],
            "/api/discussions/disc-newly-spawned/invite-peer",
        )
        # Flat output shape: disc_id + token + instruction_text in one place.
        self.assertEqual(result["disc_id"], "disc-newly-spawned")
        self.assertEqual(result["title"], "Live multi-agent room")
        self.assertEqual(result["token"], "kr-join-spawn-xyz")
        self.assertIn("disc_join", result["instruction_text"])

    def test_auto_fills_agent_from_session(self):
        self.mod.call_disc_create_room({"title": "x"})
        create_body = self.fake_http.call_args_list[0].args[2]
        self.assertEqual(create_body["agent"], "ClaudeCode")
        self.assertEqual(create_body["title"], "x")

    def test_next_step_warns_when_bridge_already_bound(self):
        # 0.8.6 fix 2026-05-22 — when the caller is already bound to
        # a disc (typical Kronn-launched session via KRONN_DISCUSSION_ID
        # env var), the response MUST include a next_step that warns
        # the room is NOT auto-joined + tells the agent what to do.
        # Pre-fix, agents silently created the room and never told the
        # user → easy to lose context.
        self.mod._CURRENT_DISC_ID = "disc-original-context"
        result = self.mod.call_disc_create_room({"title": "New room"})
        self.assertIn("next_step", result)
        ns = result["next_step"]
        # Hint MUST mention the current binding is preserved (we
        # truncate the disc id to 8 chars in the hint to keep it
        # readable — "disc-original-context"[:8] = "disc-ori").
        self.assertIn("disc-ori", ns)
        # MUST tell the agent the room is NOT auto-joined.
        self.assertTrue("NOT" in ns or "not" in ns)
        # MUST suggest both paths (share token / explicit disc_join).
        self.assertIn("instruction_text", ns)
        self.assertIn("disc_join", ns)

    def test_next_step_invites_join_when_bridge_unbound(self):
        # Host-launched session : no current disc binding → safe to
        # encourage joining the new room directly.
        self.mod._CURRENT_DISC_ID = None
        result = self.mod.call_disc_create_room({"title": "New room"})
        self.assertIn("next_step", result)
        ns = result["next_step"]
        # No active binding → call out the unbound state.
        self.assertIn("no active disc binding", ns)
        # Direct path : recommend disc_join.
        self.assertIn("disc_join", ns)


# ─── 0.8.6 phase 4 — MCP Remote Control wrappers (workflow_trigger, ──────
# workflow_run_status, qp_run). These tools turn Kronn into a fully
# MCP-driveable backend ; a regression in the contract here breaks
# Claude Code mobile flows that depend on a stable JSON shape.

class WorkflowTriggerTests(unittest.TestCase):
    """`workflow_trigger` MCP wrapper forwards to `POST /api/mcp/workflow-trigger`.

    Contract:
      - missing `workflow_id` → RuntimeError, no HTTP call
      - `variables` coerced to {str: str} (defensive against LLM-typed ints)
      - response is the unwrapped backend `data` payload
    """

    def setUp(self):
        self.mod = _load_module()
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {
                "run_id": "run-abc",
                "workflow_id": "wf-1",
                "workflow_name": "Audit + AutoPilot",
                "status": "Pending",
                "started_at": "2026-05-22T14:30:00Z",
                "expected_duration_ms": 145_000,
                "samples": 8,
                "next_check": {
                    "wait_seconds": 30,
                    "reason": "sanity check — confirm the run actually started",
                    "confidence": "baseline",
                },
            },
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_missing_workflow_id_raises_before_http(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_workflow_trigger({})
        self.fake_http.assert_not_called()

    def test_forwards_workflow_id_to_mcp_trigger_route(self):
        out = self.mod.call_workflow_trigger({"workflow_id": "wf-1"})
        method, path, body = self.fake_http.call_args.args
        self.assertEqual(method, "POST")
        self.assertEqual(path, "/api/mcp/workflow-trigger")
        self.assertEqual(body, {"workflow_id": "wf-1"})
        # The wrapper unwraps the envelope ; agent reads `run_id` directly.
        self.assertEqual(out["run_id"], "run-abc")
        self.assertEqual(out["next_check"]["wait_seconds"], 30)

    def test_coerces_variables_to_str_str_map(self):
        # An LLM might emit `{count: 3}` (int) — the backend's
        # HashMap<String,String> would reject this. Coerce in the wrapper
        # so the agent doesn't have to remember the constraint.
        self.mod.call_workflow_trigger({
            "workflow_id": "wf-1",
            "variables": {"count": 3, "name": "PeerAlpha"},
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["variables"], {"count": "3", "name": "PeerAlpha"})

    def test_drops_non_dict_variables_silently(self):
        # A misbehaving LLM passing `variables: "foo"` shouldn't crash —
        # we just drop it. Matches defensive-input philosophy elsewhere.
        self.mod.call_workflow_trigger({
            "workflow_id": "wf-1",
            "variables": "not a dict",
        })
        _, _, body = self.fake_http.call_args.args
        self.assertNotIn("variables", body)


class WorkflowActiveRunsTests(unittest.TestCase):
    """`workflow_active_runs` — in-flight board over GET /api/workflows.

    Keeps only workflows whose latest run is still in flight
    (Running / WaitingApproval / Pending); finished + never-run drop out.
    """

    def setUp(self):
        self.mod = _load_module()
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": [
                {"id": "wf-run", "name": "Running one", "project_id": "p1",
                 "last_run": {"id": "r1", "status": "Running", "started_at": "2026-06-11T10:00:00Z"}},
                {"id": "wf-gate", "name": "Awaiting gate", "project_id": None,
                 "last_run": {"id": "r2", "status": "WaitingApproval", "started_at": "2026-06-11T09:00:00Z"}},
                {"id": "wf-done", "name": "Finished", "project_id": "p2",
                 "last_run": {"id": "r3", "status": "Success", "started_at": "2026-06-11T08:00:00Z"}},
                {"id": "wf-never", "name": "Never run", "project_id": None, "last_run": None},
            ],
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_lists_only_in_flight_runs(self):
        out = self.mod.call_workflow_active_runs({})
        ids = [r["workflow_id"] for r in out]
        self.assertEqual(ids, ["wf-run", "wf-gate"])

    def test_surfaces_run_id_status_started_at(self):
        out = self.mod.call_workflow_active_runs({})
        first = out[0]
        self.assertEqual(first["run_id"], "r1")
        self.assertEqual(first["status"], "Running")
        self.assertEqual(first["workflow_name"], "Running one")
        self.assertEqual(first["started_at"], "2026-06-11T10:00:00Z")

    def test_hits_workflows_list_endpoint_with_get(self):
        self.mod.call_workflow_active_runs({})
        method, path = self.fake_http.call_args.args[:2]
        self.assertEqual(method, "GET")
        self.assertEqual(path, "/api/workflows")


class WorkflowRunStatusTests(unittest.TestCase):
    """`workflow_run_status` MCP wrapper.

    Contract:
      - missing `run_id` → RuntimeError, no HTTP call
      - GET (not POST) so the route is cache-friendly + idempotent
      - response is the unwrapped backend payload, including
        `next_check: null` for terminal runs.
    """

    def setUp(self):
        self.mod = _load_module()
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {
                "run_id": "run-abc",
                "workflow_id": "wf-1",
                "status": "Running",
                "started_at": "2026-05-22T14:30:00Z",
                "elapsed_ms": 32_000,
                "current_step": "audit-tech-debt",
                "step_count": 3,
                "tokens_used": 1240,
                "steps": [],
                "expected_duration_ms": 145_000,
                "samples": 8,
                "next_check": {
                    "wait_seconds": 113,
                    "reason": "expected ~113s left + 15s buffer",
                    "confidence": "baseline",
                },
            },
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_missing_run_id_raises_before_http(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_workflow_run_status({})
        self.fake_http.assert_not_called()

    def test_forwards_get_request_with_run_id_in_path(self):
        self.mod.call_workflow_run_status({"run_id": "run-abc"})
        # `_http(method, path)` — no body positional for GET. The
        # default param defaults to None, which the wrapper omits
        # entirely to keep the call site clean.
        args = self.fake_http.call_args.args
        self.assertEqual(args[0], "GET")
        self.assertEqual(args[1], "/api/mcp/workflow-run-status/run-abc")
        # No body positional → 2-arg signature.
        self.assertEqual(len(args), 2)

    def test_returns_unwrapped_payload(self):
        out = self.mod.call_workflow_run_status({"run_id": "run-abc"})
        self.assertEqual(out["status"], "Running")
        self.assertEqual(out["next_check"]["wait_seconds"], 113)

    def test_terminal_run_response_passes_through_null_next_check(self):
        # Terminal runs have next_check=None. Make sure the wrapper
        # doesn't fabricate one — the agent uses null to stop polling.
        self.fake_http.return_value = {
            "success": True,
            "data": {
                "run_id": "run-done",
                "workflow_id": "wf-1",
                "status": "Success",
                "started_at": "2026-05-22T14:30:00Z",
                "finished_at": "2026-05-22T14:32:25Z",
                "elapsed_ms": 145_000,
                "step_count": 5,
                "tokens_used": 5840,
                "steps": [],
                "expected_duration_ms": 145_000,
                "samples": 8,
                "next_check": None,
            },
        }
        out = self.mod.call_workflow_run_status({"run_id": "run-done"})
        self.assertEqual(out["status"], "Success")
        self.assertIsNone(out["next_check"])


class QpRunTests(unittest.TestCase):
    """`qp_run` MCP wrapper forwards to `POST /api/mcp/qp-run`.

    Contract:
      - missing `qp_id` → RuntimeError, no HTTP call
      - `vars` coerced to {str: str} (same defensive coercion as
        workflow_trigger's `variables`)
      - `project_id` auto-inherited from current disc when absent
      - `agent` / `title` / explicit `project_id` pass through unchanged
    """

    def setUp(self):
        self.mod = _load_module()
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {
                "disc_id": "disc-new",
                "qp_id": "qp-audit",
                "qp_name": "Audit Quick Prompt",
                "agent": "ClaudeCode",
                "expected_duration_ms": 60_000,
                "samples": 4,
                "next_check": {
                    "wait_seconds": 30,
                    "reason": "sanity check — confirm started (expected ~60s)",
                    "confidence": "baseline",
                },
            },
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)
        # No current disc by default — sub-tests override when needed.
        self.mod._CURRENT_DISC_META_CACHE.update({
            "checked": True,
            "value": None,
        })

    def test_missing_qp_id_raises_before_http(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_qp_run({})
        self.fake_http.assert_not_called()

    def test_forwards_minimal_body_with_qp_id(self):
        out = self.mod.call_qp_run({"qp_id": "qp-audit"})
        method, path, body = self.fake_http.call_args.args
        self.assertEqual(method, "POST")
        self.assertEqual(path, "/api/mcp/qp-run")
        self.assertEqual(body["qp_id"], "qp-audit")
        self.assertEqual(out["disc_id"], "disc-new")

    def test_coerces_vars_values_to_strings(self):
        # Same defensive coercion as workflow_trigger.variables.
        self.mod.call_qp_run({
            "qp_id": "qp-audit",
            "vars": {"count": 5, "label": "TestUser"},
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["vars"], {"count": "5", "label": "TestUser"})

    def test_passes_through_agent_project_id_and_title(self):
        # All 3 optional override fields land in the body verbatim.
        self.mod.call_qp_run({
            "qp_id": "qp-audit",
            "agent": "Codex",
            "project_id": "proj-explicit",
            "title": "Custom title",
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["agent"], "Codex")
        self.assertEqual(body["project_id"], "proj-explicit")
        self.assertEqual(body["title"], "Custom title")

    def test_auto_inherits_project_id_from_current_disc_when_absent(self):
        # The current disc lives in project `proj-inherited` — the wrapper
        # should auto-fill that into the body so the new disc doesn't
        # accidentally land in "Général".
        self.mod._CURRENT_DISC_META_CACHE.update({
            "checked": True,
            "value": {"id": "disc-parent", "project_id": "proj-inherited"},
        })
        self.mod.call_qp_run({"qp_id": "qp-audit"})
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["project_id"], "proj-inherited")

    def test_explicit_project_id_wins_over_inheritance(self):
        # Explicit > inherited — matches disc_create / workflow_create_draft.
        self.mod._CURRENT_DISC_META_CACHE.update({
            "checked": True,
            "value": {"id": "disc-parent", "project_id": "proj-inherited"},
        })
        self.mod.call_qp_run({
            "qp_id": "qp-audit",
            "project_id": "proj-explicit",
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["project_id"], "proj-explicit")


class QaRunTests(unittest.TestCase):
    """`qa_run` MCP wrapper forwards to `POST /api/quick-apis/:id/run`.

    Contract :
      - missing `qa_id` → RuntimeError, no HTTP call
      - `vars` coerced to {str: str} (same defensive coercion pattern)
      - synchronous — returns the parsed envelope inline, NO next_check
        (QAs are fast — sub-second to a few seconds)
      - missing `vars` is legal (a QA with zero variables)
    """

    def setUp(self):
        self.mod = _load_module()
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {
                "success": True,
                "duration_ms": 142,
                "envelope": {
                    "data": {"items": [{"id": "EW-1", "key": "EW-1"}]},
                    "status": "OK",
                    "summary": "GET /search → 1 result",
                },
                "error": None,
            },
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_missing_qa_id_raises_before_http(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_qa_run({})
        self.fake_http.assert_not_called()

    def test_forwards_qa_id_to_run_route_with_empty_vars(self):
        # A QA with zero variables is legal — the wrapper must still
        # forward `variables: {}` so the backend's serde default kicks
        # in cleanly.
        self.mod.call_qa_run({"qa_id": "qa-jira-fetch"})
        method, path, body = self.fake_http.call_args.args
        self.assertEqual(method, "POST")
        self.assertEqual(path, "/api/quick-apis/qa-jira-fetch/run")
        self.assertEqual(body, {"variables": {}})

    def test_forwards_vars_renamed_to_variables_at_the_wire(self):
        # The MCP tool exposes `vars` (short, ergonomic) but the backend
        # accepts `variables` (matches the workflow shape). The wrapper
        # bridges the two without leaking the rename to the agent.
        self.mod.call_qa_run({
            "qa_id": "qa-jira-fetch",
            "vars": {"ticket_id": "EW-7247"},
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["variables"], {"ticket_id": "EW-7247"})
        # `vars` itself must NOT leak to the body — the backend would 422.
        self.assertNotIn("vars", body)

    def test_coerces_vars_values_to_strings(self):
        # Same defensive coercion as workflow_trigger / qp_run.
        self.mod.call_qa_run({
            "qa_id": "qa-jira-fetch",
            "vars": {"sprint_id": 42, "label": "blocker"},
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["variables"], {"sprint_id": "42", "label": "blocker"})

    def test_returns_unwrapped_envelope(self):
        # Caller reads `envelope.data` directly — agents shouldn't have
        # to walk the outer `{success, data}` ApiResponse shape.
        out = self.mod.call_qa_run({"qa_id": "qa-jira-fetch"})
        self.assertEqual(out["envelope"]["data"]["items"][0]["id"], "EW-1")
        self.assertEqual(out["duration_ms"], 142)

    def test_backend_error_propagates_via_envelope_field(self):
        # A QA that fails (HTTP error, extract failed, required var
        # missing) returns success=false at the inner level. The wrapper
        # passes that through ; only success=false on the OUTER envelope
        # raises (per `_unwrap`).
        self.fake_http.return_value = {
            "success": True,
            "data": {
                "success": False,
                "duration_ms": 12,
                "envelope": None,
                "error": "Variable obligatoire manquante : `ticket_id`",
            },
        }
        out = self.mod.call_qa_run({"qa_id": "qa-jira-fetch"})
        self.assertFalse(out["success"])
        self.assertIn("ticket_id", out["error"])
        self.assertIsNone(out["envelope"])


class QaListEnrichedOutputTests(unittest.TestCase):
    """`qa_list` was extended in phase 4 to expose the `variables[]`
    field so agents calling `qa_run` know what to pass without an
    extra `GET /api/quick-apis/<id>` round-trip."""

    def setUp(self):
        self.mod = _load_module()
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": [
                {
                    "id": "qa-jira-fetch",
                    "name": "Fetch Jira ticket",
                    "api_plugin_slug": "mcp-atlassian",
                    "api_endpoint_path": "/rest/api/3/issue/{ticket_id}",
                    "api_method": "GET",
                    "description": "Fetch a Jira ticket by key",
                    "project_id": None,
                    "variables": [
                        {
                            "name": "ticket_id",
                            "label": "Ticket ID",
                            "placeholder": "EW-7247",
                            "description": "JIRA issue key",
                            "required": True,
                        },
                        {
                            "name": "expand",
                            "label": "Expand",
                            "placeholder": "comments",
                            "description": None,
                            "required": False,
                        },
                    ],
                },
                {
                    # A QA with no variables — defensive : empty list,
                    # not missing key.
                    "id": "qa-jira-myself",
                    "name": "Who am I",
                    "api_plugin_slug": "mcp-atlassian",
                    "api_endpoint_path": "/rest/api/3/myself",
                    "api_method": "GET",
                    "description": "",
                    "project_id": None,
                    "variables": [],
                },
            ],
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_exposes_variables_per_entry(self):
        out = self.mod.call_qa_list({})
        first = next(q for q in out if q["id"] == "qa-jira-fetch")
        # The agent reads `variables[].name` directly to know what to
        # pass to `qa_run.vars`. Pin the exact shape.
        self.assertEqual(len(first["variables"]), 2)
        ticket_id_var = next(v for v in first["variables"] if v["name"] == "ticket_id")
        self.assertEqual(ticket_id_var["label"], "Ticket ID")
        self.assertTrue(ticket_id_var["required"])
        self.assertEqual(ticket_id_var["description"], "JIRA issue key")

    def test_normalises_empty_description_to_none(self):
        # When the underlying QA stores `description: ""`, the wrapper
        # emits `None` so agents can branch on truthiness without
        # special-casing empty strings.
        out = self.mod.call_qa_list({})
        first = next(q for q in out if q["id"] == "qa-jira-fetch")
        expand_var = next(v for v in first["variables"] if v["name"] == "expand")
        self.assertIsNone(expand_var["description"])

    def test_handles_qa_with_zero_variables(self):
        # Empty list — not missing, not None. Lets the agent infer
        # "this QA needs no input, call `qa_run({qa_id})` directly".
        out = self.mod.call_qa_list({})
        myself = next(q for q in out if q["id"] == "qa-jira-myself")
        self.assertEqual(myself["variables"], [])

    def test_preserves_legacy_fields_alongside_variables(self):
        # Defensive : the variables addition MUST NOT drop any
        # previously-exposed field — that would break agents that
        # consumed the old shape.
        out = self.mod.call_qa_list({})
        first = next(q for q in out if q["id"] == "qa-jira-fetch")
        for field in ("id", "name", "api_plugin_slug", "api_endpoint_path",
                      "api_method", "description", "project_id"):
            self.assertIn(field, first)


class QaCreateDraftTests(unittest.TestCase):
    """`qa_create_draft` MCP wrapper closes the symmetry gap with
    `workflow_create_draft` + `qp_create_draft`. Pin the contract :
      - required fields validated client-side (cleaner error than 422)
      - project_id auto-inherited from current disc when absent
      - explicit project_id wins over inheritance
      - name length cap mirrors qp_create_draft (200 chars)
      - POSTs to /api/quick-apis (the existing create route)
    """

    def setUp(self):
        self.mod = _load_module()
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {
                "id": "qa-new-123",
                "name": "Fetch active sprint",
                "api_plugin_slug": "mcp-atlassian",
                "api_config_id": "cfg-jira",
                "api_endpoint_path": "/rest/api/3/search/jql",
                "api_method": "GET",
                "variables": [],
            },
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)
        # No current disc by default — sub-tests override when needed.
        self.mod._CURRENT_DISC_META_CACHE.update({
            "checked": True,
            "value": None,
        })

    def test_rejects_missing_required_fields_before_http(self):
        # Each of the 4 required fields → RuntimeError before any HTTP call.
        for missing in ("name", "api_plugin_slug", "api_config_id", "api_endpoint_path"):
            args = {
                "name": "QA",
                "api_plugin_slug": "mcp-atlassian",
                "api_config_id": "cfg-jira",
                "api_endpoint_path": "/foo",
            }
            args.pop(missing)
            with self.assertRaises(RuntimeError) as cm:
                self.mod.call_qa_create_draft(args)
            self.assertIn(missing, str(cm.exception))
        self.fake_http.assert_not_called()

    def test_rejects_excessively_long_name(self):
        # Mirrors qp_create_draft's 200-char cap.
        with self.assertRaises(RuntimeError):
            self.mod.call_qa_create_draft({
                "name": "x" * 201,
                "api_plugin_slug": "mcp-atlassian",
                "api_config_id": "cfg-jira",
                "api_endpoint_path": "/foo",
            })

    def test_posts_to_quick_apis_create_route(self):
        out = self.mod.call_qa_create_draft({
            "name": "Fetch active sprint",
            "api_plugin_slug": "mcp-atlassian",
            "api_config_id": "cfg-jira",
            "api_endpoint_path": "/rest/api/3/search/jql",
            "api_method": "GET",
            "api_query": {"jql": "sprint in openSprints()"},
        })
        method, path, body = self.fake_http.call_args.args
        self.assertEqual(method, "POST")
        self.assertEqual(path, "/api/quick-apis")
        # Pass-through of the QA spec — same shape as the UI create form.
        self.assertEqual(body["name"], "Fetch active sprint")
        self.assertEqual(body["api_plugin_slug"], "mcp-atlassian")
        self.assertEqual(body["api_endpoint_path"], "/rest/api/3/search/jql")
        self.assertEqual(body["api_query"], {"jql": "sprint in openSprints()"})
        # Caller sees the unwrapped created QA, can echo back the id.
        self.assertEqual(out["id"], "qa-new-123")

    def test_auto_inherits_project_id_from_current_disc(self):
        # An agent operating inside a project's disc should NOT see its
        # QA created in "Général". Same UX rationale as qp_create_draft.
        self.mod._CURRENT_DISC_META_CACHE.update({
            "checked": True,
            "value": {"id": "disc-parent", "project_id": "proj-inherited"},
        })
        self.mod.call_qa_create_draft({
            "name": "QA",
            "api_plugin_slug": "mcp-atlassian",
            "api_config_id": "cfg-jira",
            "api_endpoint_path": "/foo",
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["project_id"], "proj-inherited")

    def test_explicit_project_id_wins_over_inheritance(self):
        # Explicit > inherited — matches the cluster (disc_create,
        # workflow_create_draft, qp_create_draft, qp_run).
        self.mod._CURRENT_DISC_META_CACHE.update({
            "checked": True,
            "value": {"id": "disc-parent", "project_id": "proj-inherited"},
        })
        self.mod.call_qa_create_draft({
            "name": "QA",
            "api_plugin_slug": "mcp-atlassian",
            "api_config_id": "cfg-jira",
            "api_endpoint_path": "/foo",
            "project_id": "proj-explicit",
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["project_id"], "proj-explicit")

    def test_listed_in_TOOLS_with_required_fields_pinned(self):
        # Discovery contract — agents enumerate TOOLS, must find
        # qa_create_draft alongside workflow_create_draft + qp_create_draft.
        tool = next(t for t in self.mod.TOOLS if t["name"] == "qa_create_draft")
        for required in ("name", "api_plugin_slug", "api_config_id", "api_endpoint_path"):
            self.assertIn(required, tool["inputSchema"]["required"])

    def test_dispatched_in_DISPATCH(self):
        # Tools listed in TOOLS must be wired in DISPATCH — else
        # tools/call returns 'unknown method'. Defensive pin.
        self.assertIn("qa_create_draft", self.mod.DISPATCH)


class QaUpdateTests(unittest.TestCase):
    """`qa_update` MCP wrapper — partial-update semantics on top of the
    bare PUT route. The wrapper MUST preserve every field the agent
    doesn't pass, otherwise the bare PUT would reset
    variables/profile_ids/directive_ids to empty.
    """

    def setUp(self):
        self.mod = _load_module()
        self.existing_qa = {
            "id": "qa-jira-fetch",
            "name": "Fetch Jira ticket",
            "icon": "🎫",
            "description": "Fetch a ticket by key",
            "project_id": "proj-jira",
            "api_plugin_slug": "mcp-atlassian",
            "api_config_id": "cfg-jira",
            "api_endpoint_path": "/rest/api/3/issue/{ticket_id}",
            "api_method": "GET",
            "api_query": {"expand": "renderedFields,changelog"},
            "api_path_params": None,
            "api_headers": None,
            "api_body": None,
            "api_extract": None,
            "api_pagination": None,
            "api_timeout_ms": 10_000,
            "api_max_retries": 2,
            "variables": [
                {
                    "name": "ticket_id",
                    "label": "Ticket ID",
                    "placeholder": "EW-7308",
                    "description": "JIRA issue key",
                    "required": True,
                },
            ],
            "profile_ids": ["profile-eng"],
            "directive_ids": [],
        }
        # _http : first call (GET /api/quick-apis) returns the list ;
        # subsequent calls (PUT) return the updated QA. Use side_effect.
        self.updated_qa_response = dict(self.existing_qa)
        self.fake_http = mock.MagicMock(side_effect=[
            {"success": True, "data": [self.existing_qa]},  # GET list
            {"success": True, "data": self.updated_qa_response},  # PUT
        ])
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_rejects_missing_qa_id(self):
        with self.assertRaises(RuntimeError) as cm:
            self.mod.call_qa_update({})
        self.assertIn("qa_id", str(cm.exception))
        # Crucially, NO HTTP call when validation fails.
        self.fake_http.assert_not_called()

    def test_rejects_unknown_qa_id_with_actionable_hint(self):
        # Existing list returns no matching QA → wrapper raises with a
        # hint that points the agent to qa_list. Better UX than a 404
        # from the PUT route mid-flow.
        self.fake_http.side_effect = [
            {"success": True, "data": [self.existing_qa]},
        ]
        with self.assertRaises(RuntimeError) as cm:
            self.mod.call_qa_update({"qa_id": "qa-does-not-exist"})
        self.assertIn("not found", str(cm.exception))
        self.assertIn("qa_list", str(cm.exception))

    def test_extract_only_patch_preserves_everything_else(self):
        # The flagship use case : agent realises payload is verbose,
        # patches ONLY api_extract. variables / profile_ids /
        # api_query / etc. MUST survive intact (the bare PUT would
        # wipe variables ; the wrapper merges).
        self.mod.call_qa_update({
            "qa_id": "qa-jira-fetch",
            "api_extract": {"path": "$.fields", "fail_on_empty": True},
        })
        # 1st _http call : GET list (already asserted via side_effect setup).
        # 2nd _http call : PUT with merged body.
        put_method, put_path, put_body = self.fake_http.call_args_list[1].args
        self.assertEqual(put_method, "PUT")
        self.assertEqual(put_path, "/api/quick-apis/qa-jira-fetch")
        # The patch landed :
        self.assertEqual(put_body["api_extract"], {"path": "$.fields", "fail_on_empty": True})
        # Everything else is intact :
        self.assertEqual(put_body["name"], "Fetch Jira ticket")
        self.assertEqual(put_body["api_endpoint_path"], "/rest/api/3/issue/{ticket_id}")
        self.assertEqual(put_body["api_query"], {"expand": "renderedFields,changelog"})
        self.assertEqual(put_body["api_method"], "GET")
        self.assertEqual(put_body["api_max_retries"], 2)
        # Critically : the bare PUT would wipe these to empty without merge.
        self.assertEqual(len(put_body["variables"]), 1)
        self.assertEqual(put_body["variables"][0]["name"], "ticket_id")
        self.assertEqual(put_body["profile_ids"], ["profile-eng"])

    def test_explicit_empty_list_clears_field(self):
        # Passing `variables: []` explicitly should clear them (agent
        # asked for it). Distinguish from "field absent" (preserve).
        self.mod.call_qa_update({
            "qa_id": "qa-jira-fetch",
            "variables": [],
        })
        _, _, put_body = self.fake_http.call_args_list[1].args
        self.assertEqual(put_body["variables"], [])
        # Other fields still preserved.
        self.assertEqual(put_body["profile_ids"], ["profile-eng"])

    def test_multi_field_patch_applies_all_overrides(self):
        # A more realistic patch : add api_extract, tighten api_query
        # (vendor-side filter), bump retries. All three apply, nothing
        # else moves.
        self.mod.call_qa_update({
            "qa_id": "qa-jira-fetch",
            "api_extract": {"path": "$.fields"},
            "api_query": {"fields": "summary,status,assignee"},
            "api_max_retries": 5,
        })
        _, _, put_body = self.fake_http.call_args_list[1].args
        self.assertEqual(put_body["api_extract"], {"path": "$.fields"})
        self.assertEqual(put_body["api_query"], {"fields": "summary,status,assignee"})
        self.assertEqual(put_body["api_max_retries"], 5)
        # Untouched fields still there.
        self.assertEqual(put_body["name"], "Fetch Jira ticket")
        self.assertEqual(len(put_body["variables"]), 1)

    def test_rejects_excessively_long_name_after_merge(self):
        # Defense : if an agent patches with name="x"*201, the merge
        # produces a too-long name. Reject BEFORE the PUT round-trip.
        with self.assertRaises(RuntimeError):
            self.mod.call_qa_update({
                "qa_id": "qa-jira-fetch",
                "name": "x" * 201,
            })

    def test_listed_in_TOOLS_with_only_qa_id_required(self):
        # Discovery contract : agents discover qa_update alongside
        # qa_create_draft + qa_run. ONLY qa_id is required — every
        # other field is optional (the merge fills the gaps).
        tool = next(t for t in self.mod.TOOLS if t["name"] == "qa_update")
        self.assertEqual(tool["inputSchema"]["required"], ["qa_id"])
        # Each patchable field exposed in properties.
        for f in ("api_extract", "api_query", "variables", "name"):
            self.assertIn(f, tool["inputSchema"]["properties"])

    def test_dispatched_in_DISPATCH(self):
        self.assertIn("qa_update", self.mod.DISPATCH)


class QaCreateDraftProbeFirstGuidanceTests(unittest.TestCase):
    """The `qa_create_draft` description was rewritten in PR 1.8 to push
    the PROBE-then-PERSIST workflow. Pin the key teaching points so a
    future description refactor doesn't accidentally drop them.
    """

    def setUp(self):
        self.mod = _load_module()
        self.tool = next(t for t in self.mod.TOOLS if t["name"] == "qa_create_draft")

    def test_description_recommends_probe_first(self):
        # The PROBE keyword + the recommendation to do an `api_call`
        # first MUST be present — without them the description reads
        # like "create + hope" and agents will repeat the 12k-token boo-boo.
        desc = self.tool["description"]
        self.assertIn("PROBE", desc)
        self.assertIn("api_call", desc)

    def test_description_mentions_vendor_payload_sizes(self):
        # Concrete numbers anchor the lesson — a future agent reading
        # the description sees that the "12k tokens" warning is real.
        desc = self.tool["description"]
        # Loose check : the word "tokens" + a "10-40k"-ish range must appear.
        self.assertIn("tokens", desc.lower())
        self.assertIn("10-40k", desc)

    def test_description_points_to_qa_update_for_iteration(self):
        # Agents who DID skip the probe should know they can recover
        # via qa_update without UI friction.
        desc = self.tool["description"]
        self.assertIn("qa_update", desc)


class CreateDraftClusterSymmetryTests(unittest.TestCase):
    """The 3 *_create_draft tools must form a coherent cluster — same
    discovery surface, same auto-inheritance, same project-id passthrough.
    A new draft tool that breaks one of these contracts in the future
    will be caught here."""

    def setUp(self):
        self.mod = _load_module()
        self.cluster = ("workflow_create_draft", "qp_create_draft", "qa_create_draft")

    def test_all_three_drafts_present_in_TOOLS(self):
        names = {t["name"] for t in self.mod.TOOLS}
        for required in self.cluster:
            self.assertIn(required, names,
                f"`{required}` missing — cluster symmetry broken")

    def test_all_three_drafts_dispatched(self):
        for required in self.cluster:
            self.assertIn(required, self.mod.DISPATCH,
                f"`{required}` listed but not dispatched")

    def test_all_three_drafts_accept_project_id_property(self):
        # Auto-inheritance from current disc requires the schema accept
        # project_id even when not in `required`. Defensive pin.
        for name in self.cluster:
            tool = next(t for t in self.mod.TOOLS if t["name"] == name)
            self.assertIn("project_id", tool["inputSchema"]["properties"],
                f"`{name}` schema missing project_id property")


class McpRemoteControlToolListingTests(unittest.TestCase):
    """The 3 new tools must be discoverable via the standard tools/list
    surface — without this, agents can't find them even if the routes
    are wired. Pin the contract here so a TOOLS list refactor doesn't
    silently drop them."""

    def setUp(self):
        self.mod = _load_module()
        self.tool_names = {t["name"] for t in self.mod.TOOLS}

    def test_workflow_trigger_listed_with_workflow_id_required(self):
        tool = next(t for t in self.mod.TOOLS if t["name"] == "workflow_trigger")
        self.assertIn("workflow_id", tool["inputSchema"]["required"])

    def test_workflow_run_status_listed_with_run_id_required(self):
        tool = next(t for t in self.mod.TOOLS if t["name"] == "workflow_run_status")
        self.assertIn("run_id", tool["inputSchema"]["required"])

    def test_qp_run_listed_with_qp_id_required(self):
        tool = next(t for t in self.mod.TOOLS if t["name"] == "qp_run")
        self.assertIn("qp_id", tool["inputSchema"]["required"])

    def test_qa_run_listed_with_qa_id_required(self):
        tool = next(t for t in self.mod.TOOLS if t["name"] == "qa_run")
        self.assertIn("qa_id", tool["inputSchema"]["required"])

    def test_all_four_tools_present(self):
        # Single assertion for the discovery contract — agents need
        # all four to drive the full launch+track flow + QA exec.
        for required in ("workflow_trigger", "workflow_run_status", "qp_run", "qa_run"):
            self.assertIn(required, self.tool_names,
                f"`{required}` missing from TOOLS — agents can't discover it")

    def test_async_tool_descriptions_mention_next_check_smart_polling(self):
        # The smart-polling hint is the whole point of the async tools ;
        # description MUST explain it so agents honour the wait_seconds.
        # `qa_run` is synchronous — explicitly excluded (it would be
        # actively misleading to mention next_check on a tool that
        # never returns one).
        for name in ("workflow_trigger", "workflow_run_status", "qp_run"):
            tool = next(t for t in self.mod.TOOLS if t["name"] == name)
            self.assertIn("next_check", tool["description"],
                f"`{name}` description must reference next_check")

    def test_qa_run_description_excludes_next_check_and_explains_sync(self):
        # `qa_run` is synchronous — calling out the absence of
        # next_check (NO `next_check`) prevents agents from waiting
        # for a hint that will never come.
        tool = next(t for t in self.mod.TOOLS if t["name"] == "qa_run")
        self.assertIn("synchronous", tool["description"].lower())
        # Must mention NO next_check so an agent migrating from qp_run
        # doesn't blindly look for the field.
        self.assertIn("NO `next_check`", tool["description"])

    def test_all_four_tools_dispatched(self):
        # Tools listed in TOOLS must be wired in DISPATCH — else
        # tools/call returns 'unknown method'. Defensive pin.
        for required in ("workflow_trigger", "workflow_run_status", "qp_run", "qa_run"):
            self.assertIn(required, self.mod.DISPATCH,
                f"`{required}` listed but not dispatched")


class QpBatchRunTests(unittest.TestCase):
    """`qp_batch_run` MCP wrapper forwards to `POST /api/mcp/qp-batch-run`.

    Contract:
      - missing `qp_id` → RuntimeError, no HTTP
      - missing / empty / non-list `items` → RuntimeError, no HTTP
      - non-dict item → RuntimeError
      - each item's `vars` coerced to {str: str}; `title` str-coerced; absent
        title/vars keys omitted from the item
      - `project_id` auto-inherited from current disc when absent
      - explicit `project_id` / `batch_name` pass through
    """

    def setUp(self):
        self.mod = _load_module()
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {
                "run_id": "batch-run-1",
                "qp_id": "qp-audit",
                "qp_name": "Audit Quick Prompt",
                "disc_ids": ["disc-1", "disc-2"],
                "batch_total": 2,
                "expected_duration_ms": 60_000,
                "samples": 4,
                "next_check": {"wait_seconds": 30, "reason": "…", "confidence": "baseline"},
            },
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)
        # No current disc by default — inheritance sub-test overrides.
        self.mod._CURRENT_DISC_META_CACHE.update({"checked": True, "value": None})

    def test_missing_qp_id_raises_before_http(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_qp_batch_run({"items": [{"vars": {}}]})
        self.fake_http.assert_not_called()

    def test_missing_items_raises_before_http(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_qp_batch_run({"qp_id": "qp-audit"})
        self.fake_http.assert_not_called()

    def test_empty_items_list_raises_before_http(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_qp_batch_run({"qp_id": "qp-audit", "items": []})
        self.fake_http.assert_not_called()

    def test_non_list_items_raises_before_http(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_qp_batch_run({"qp_id": "qp-audit", "items": "nope"})
        self.fake_http.assert_not_called()

    def test_non_dict_item_raises(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_qp_batch_run({"qp_id": "qp-audit", "items": ["nope"]})
        self.fake_http.assert_not_called()

    def test_forwards_items_with_coerced_vars_and_title(self):
        out = self.mod.call_qp_batch_run({
            "qp_id": "qp-audit",
            "items": [
                {"title": "First", "vars": {"count": 3, "name": "TestUser"}},
                {"vars": {"count": 4}},
            ],
        })
        method, path, body = self.fake_http.call_args.args
        self.assertEqual(method, "POST")
        self.assertEqual(path, "/api/mcp/qp-batch-run")
        self.assertEqual(body["qp_id"], "qp-audit")
        self.assertEqual(
            body["items"][0],
            {"title": "First", "vars": {"count": "3", "name": "TestUser"}},
        )
        # Second item had no title key — it stays absent, vars coerced.
        self.assertEqual(body["items"][1], {"vars": {"count": "4"}})
        self.assertEqual(out["run_id"], "batch-run-1")

    def test_auto_inherits_project_id_when_absent(self):
        self.mod._CURRENT_DISC_META_CACHE.update({
            "checked": True,
            "value": {"id": "disc-parent", "project_id": "proj-inherited"},
        })
        self.mod.call_qp_batch_run({"qp_id": "qp-audit", "items": [{"vars": {}}]})
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["project_id"], "proj-inherited")

    def test_explicit_project_id_and_batch_name_pass_through(self):
        self.mod.call_qp_batch_run({
            "qp_id": "qp-audit",
            "items": [{"vars": {}}],
            "project_id": "proj-explicit",
            "batch_name": "My Batch",
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["project_id"], "proj-explicit")
        self.assertEqual(body["batch_name"], "My Batch")


class WorkflowRunDiscussionsTests(unittest.TestCase):
    """`workflow_run_discussions` wrapper → GET /api/mcp/workflow-run-discussions/<run_id>."""

    def setUp(self):
        self.mod = _load_module()
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {
                "run_id": "batch-run-1",
                "disc_count": 1,
                "discussions": [
                    {
                        "disc_id": "disc-1", "title": "First", "agent": "ClaudeCode",
                        "message_count": 3, "archived": False,
                        "created_at": "2026-05-25T10:00:00Z",
                    },
                ],
            },
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_missing_run_id_raises_before_http(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_workflow_run_discussions({})
        self.fake_http.assert_not_called()

    def test_forwards_get_with_run_id_in_path(self):
        out = self.mod.call_workflow_run_discussions({"run_id": "batch-run-1"})
        args = self.fake_http.call_args.args
        self.assertEqual(args[0], "GET")
        self.assertEqual(args[1], "/api/mcp/workflow-run-discussions/batch-run-1")
        self.assertEqual(out["disc_count"], 1)
        self.assertEqual(out["discussions"][0]["disc_id"], "disc-1")


class WorkflowWaitForCompletionTests(unittest.TestCase):
    """`workflow_wait_for_completion` wrapper → POST /api/mcp/workflow-wait-for-completion."""

    def setUp(self):
        self.mod = _load_module()
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {
                "run_id": "run-abc",
                "workflow_id": "wf-1",
                "status": "Success",
                "finished_at": "2026-05-25T10:05:00Z",
                "elapsed_ms": 42_000,
                "tokens_used": 2200,
                "timed_out": False,
                "next_check": None,
            },
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def test_missing_run_id_raises_before_http(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_workflow_wait_for_completion({})
        self.fake_http.assert_not_called()

    def test_forwards_post_with_run_id(self):
        out = self.mod.call_workflow_wait_for_completion({"run_id": "run-abc"})
        method, path, body = self.fake_http.call_args.args
        self.assertEqual(method, "POST")
        self.assertEqual(path, "/api/mcp/workflow-wait-for-completion")
        self.assertEqual(body, {"run_id": "run-abc"})
        self.assertEqual(out["status"], "Success")

    def test_coerces_timeout_s_to_int(self):
        # An LLM may emit "30" (str) — coerce so the backend u64 parses.
        self.mod.call_workflow_wait_for_completion({"run_id": "run-abc", "timeout_s": "30"})
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["timeout_s"], 30)
        self.assertIsInstance(body["timeout_s"], int)

    def test_invalid_timeout_s_raises(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_workflow_wait_for_completion({"run_id": "run-abc", "timeout_s": "abc"})

    def test_omits_timeout_s_when_absent(self):
        self.mod.call_workflow_wait_for_completion({"run_id": "run-abc"})
        _, _, body = self.fake_http.call_args.args
        self.assertNotIn("timeout_s", body)


class McpRemoteControlPr2Pr3ToolListingTests(unittest.TestCase):
    """PR2/PR3 tools must be discoverable via tools/list AND dispatched."""

    def setUp(self):
        self.mod = _load_module()
        self.tool_names = {t["name"] for t in self.mod.TOOLS}

    def test_new_tools_present(self):
        for name in ("qp_batch_run", "workflow_run_discussions", "workflow_wait_for_completion"):
            self.assertIn(name, self.tool_names, f"`{name}` missing from TOOLS")

    def test_new_tools_dispatched(self):
        for name in ("qp_batch_run", "workflow_run_discussions", "workflow_wait_for_completion"):
            self.assertIn(name, self.mod.DISPATCH, f"`{name}` listed but not dispatched")

    def test_required_fields_pinned(self):
        qb = next(t for t in self.mod.TOOLS if t["name"] == "qp_batch_run")
        self.assertIn("qp_id", qb["inputSchema"]["required"])
        self.assertIn("items", qb["inputSchema"]["required"])
        rd = next(t for t in self.mod.TOOLS if t["name"] == "workflow_run_discussions")
        self.assertIn("run_id", rd["inputSchema"]["required"])
        ww = next(t for t in self.mod.TOOLS if t["name"] == "workflow_wait_for_completion")
        self.assertIn("run_id", ww["inputSchema"]["required"])

    def test_wait_description_mentions_timeout_and_non_terminal_gate(self):
        ww = next(t for t in self.mod.TOOLS if t["name"] == "workflow_wait_for_completion")
        self.assertIn("timeout_s", ww["description"])
        # WaitingApproval is explicitly NOT terminal — the description must
        # warn so an agent doesn't assume a Gate'd workflow will resolve here.
        self.assertIn("WaitingApproval", ww["description"])


class ConventionGetTests(unittest.TestCase):
    """0.8.7 — `convention_get` lets an agent pull a Kronn doc convention
    spec on demand (cheap if not called). Pins the allowlist (refuse
    unknown names — no arbitrary URL fetch surface) and the response
    shape (`{name, version, content_markdown}`)."""

    def setUp(self):
        self.mod = _load_module()

    def test_registered_in_tools_and_dispatch(self):
        names = [t["name"] for t in self.mod.TOOLS]
        self.assertIn("convention_get", names)
        self.assertIn("convention_get", self.mod.DISPATCH)

    def test_defaults_to_agents_md_format_v1_and_hits_text_endpoint(self):
        with mock.patch.object(self.mod, "_http_text", return_value="# spec body") as m:
            out = self.mod.call_convention_get({})
        m.assert_called_once_with("GET", "/api/conventions/agents-md-format-v1")
        self.assertEqual(out["name"], "agents-md-format")
        self.assertEqual(out["version"], "v1")
        self.assertEqual(out["content_markdown"], "# spec body")

    def test_explicit_args_passed_through(self):
        with mock.patch.object(self.mod, "_http_text", return_value="x") as m:
            self.mod.call_convention_get({"name": "agents-md-format", "version": "v1"})
        m.assert_called_once_with("GET", "/api/conventions/agents-md-format-v1")

    def test_unknown_convention_raises_without_issuing_http(self):
        # Allowlist guard — protects against an agent baiting the tool into
        # fetching an arbitrary backend path.
        with mock.patch.object(self.mod, "_http_text") as m:
            with self.assertRaises(RuntimeError) as ctx:
                self.mod.call_convention_get({"name": "../etc/passwd"})
        m.assert_not_called()
        self.assertIn("unknown convention", str(ctx.exception))

    def test_unknown_version_raises(self):
        with mock.patch.object(self.mod, "_http_text") as m:
            with self.assertRaises(RuntimeError):
                self.mod.call_convention_get({"name": "agents-md-format", "version": "v99"})
        m.assert_not_called()


class CallLearningProposeTests(unittest.TestCase):
    """0.9.0 — `learning_propose` client-side guards + auto-inheritance + the
    POST body it sends to `/api/learnings/propose`."""

    def setUp(self):
        self.mod = _load_module()
        self.mod._CURRENT_DISC_META_CACHE.update({
            "checked": True,
            "value": {"id": "disc-parent", "project_id": "proj-eu", "agent": "ClaudeCode"},
        })
        self.fake_http = mock.MagicMock(return_value={
            "success": True,
            "data": {"accepted": True, "warnings": [], "evidence_checks": []},
        })
        self.http_patch = mock.patch.object(self.mod, "_http", self.fake_http)
        self.http_patch.start()
        self.addCleanup(self.http_patch.stop)

    def _ev(self):
        return [{"kind": "file", "ref": "src/foo.rs:42", "quote": "fn foo() {}"}]

    def test_empty_evidence_raises_before_http(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_learning_propose({"claim": "x", "kind": "fact", "evidence": []})
        self.fake_http.assert_not_called()

    def test_blank_claim_raises(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_learning_propose({"claim": "   ", "kind": "fact", "evidence": self._ev()})
        self.fake_http.assert_not_called()

    def test_bad_kind_raises(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_learning_propose({"claim": "x", "kind": "guess", "evidence": self._ev()})
        self.fake_http.assert_not_called()

    def test_evidence_without_ref_raises(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_learning_propose(
                {"claim": "x", "kind": "fact", "evidence": [{"kind": "file", "ref": "  "}]}
            )
        self.fake_http.assert_not_called()

    def test_posts_to_endpoint_with_inherited_context(self):
        self.mod.call_learning_propose({"claim": "uses pnpm", "kind": "fact", "evidence": self._ev()})
        method, path, body = self.fake_http.call_args.args
        self.assertEqual(method, "POST")
        self.assertEqual(path, "/api/learnings/propose")
        self.assertEqual(body["claim"], "uses pnpm")
        self.assertEqual(body["kind"], "fact")
        self.assertEqual(len(body["evidence"]), 1)
        # auto-inherited from the parent disc
        self.assertEqual(body["discussion_id"], "disc-parent")
        self.assertEqual(body["project_id"], "proj-eu")
        self.assertEqual(body["source_agent"], "ClaudeCode")

    def test_explicit_context_overrides_inheritance(self):
        self.mod.call_learning_propose({
            "claim": "x", "kind": "preference", "evidence": self._ev(),
            "project_id": "proj-other", "source_agent": "Codex",
        })
        _, _, body = self.fake_http.call_args.args
        self.assertEqual(body["project_id"], "proj-other")
        self.assertEqual(body["source_agent"], "Codex")


class WorkflowQpCrudToolTests(unittest.TestCase):
    """0.8.8 (2026-06-23) — read · clone · update · enable wrappers for
    workflows + Quick Prompts. Thin wrappers over existing REST routes;
    we assert the right method/path/body, the Cron enable guard, and the
    PromptVariable label/placeholder normalisation."""

    def setUp(self):
        self.mod = _load_module()
        self.env_patch = mock.patch.dict(
            os.environ,
            {"KRONN_DISCUSSION_ID": "disc-abc", "KRONN_BACKEND_URL": "http://127.0.0.1:3140"},
            clear=False,
        )
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)

    @staticmethod
    def _env(data):
        """An `ApiResponse` envelope `_unwrap` will unwrap to `data`."""
        return {"success": True, "data": data}

    # ── _normalize_variables ─────────────────────────────────────────
    def test_normalize_variables_fills_label_and_placeholder(self):
        out = self.mod._normalize_variables([{"name": "ticket"}])
        self.assertEqual(out[0]["label"], "ticket")
        self.assertEqual(out[0]["placeholder"], "")

    def test_normalize_variables_preserves_existing(self):
        v = {"name": "x", "label": "L", "placeholder": "P"}
        self.assertEqual(self.mod._normalize_variables([v])[0], v)

    def test_normalize_variables_passes_through_non_list(self):
        self.assertIsNone(self.mod._normalize_variables(None))

    # ── _normalize_steps (tagged-enum wrapping) ──────────────────────
    def test_normalize_steps_wraps_bare_string_step_type(self):
        out = self.mod._normalize_steps([{"name": "s", "step_type": "Agent"}])
        self.assertEqual(out[0]["step_type"], {"type": "Agent"})

    def test_normalize_steps_wraps_output_format_and_mode(self):
        out = self.mod._normalize_steps([
            {"name": "s", "step_type": "Agent", "output_format": "Structured", "mode": "Normal"}
        ])
        self.assertEqual(out[0]["output_format"], {"type": "Structured"})
        self.assertEqual(out[0]["mode"], {"type": "Normal"})

    def test_normalize_steps_is_idempotent_on_tagged_objects(self):
        already = [{"name": "s", "step_type": {"type": "ApiCall"}}]
        self.assertEqual(self.mod._normalize_steps(already), already)

    def test_create_draft_normalizes_step_type_before_post(self):
        fake = mock.MagicMock(return_value=self._env({"id": "wf-1"}))
        with mock.patch.object(self.mod, "_http", fake), \
             mock.patch.object(self.mod, "_current_project_id", return_value=None):
            self.mod.call_workflow_create_draft({
                "name": "W", "trigger": {"type": "Manual"},
                "steps": [{"name": "s", "step_type": "Agent", "agent": "ClaudeCode",
                           "prompt_template": "x"}],
            })
        _, _, body = fake.call_args.args
        self.assertEqual(body["steps"][0]["step_type"], {"type": "Agent"},
                         "bare-string step_type must be wrapped before the POST")
        self.assertFalse(body["enabled"])

    def test_create_draft_defaults_concurrency_1_for_cron(self):
        fake = mock.MagicMock(return_value=self._env({"id": "wf-1"}))
        with mock.patch.object(self.mod, "_http", fake), \
             mock.patch.object(self.mod, "_current_project_id", return_value=None):
            self.mod.call_workflow_create_draft({
                "name": "Nightly", "trigger": {"type": "Cron", "schedule": "0 9 * * *"},
                "steps": [{"name": "s", "step_type": "ApiCall", "api_plugin_slug": "x",
                           "api_config_id": "y", "api_endpoint_path": "/z"}],
            })
        _, _, body = fake.call_args.args
        self.assertEqual(body["concurrency_limit"], 1,
                         "Cron with no concurrency_limit must default to 1 (no self-overlap)")

    def test_create_draft_respects_explicit_concurrency(self):
        fake = mock.MagicMock(return_value=self._env({"id": "wf-1"}))
        with mock.patch.object(self.mod, "_http", fake), \
             mock.patch.object(self.mod, "_current_project_id", return_value=None):
            self.mod.call_workflow_create_draft({
                "name": "Parallel", "trigger": {"type": "Cron", "schedule": "*/5 * * * *"},
                "concurrency_limit": 5,
                "steps": [{"name": "s", "step_type": "Notify", "notify_config": {}}],
            })
        _, _, body = fake.call_args.args
        self.assertEqual(body["concurrency_limit"], 5, "explicit concurrency_limit must win")

    def test_create_draft_no_concurrency_default_for_manual(self):
        fake = mock.MagicMock(return_value=self._env({"id": "wf-1"}))
        with mock.patch.object(self.mod, "_http", fake), \
             mock.patch.object(self.mod, "_current_project_id", return_value=None):
            self.mod.call_workflow_create_draft({
                "name": "OnDemand", "trigger": {"type": "Manual"},
                "steps": [{"name": "s", "step_type": "Notify", "notify_config": {}}],
            })
        _, _, body = fake.call_args.args
        self.assertIsNone(body.get("concurrency_limit"),
                          "Manual trigger must NOT force a concurrency_limit (user-initiated)")

    def test_update_normalizes_step_type(self):
        fake = mock.MagicMock(return_value=self._env({"id": "wf-1"}))
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_workflow_update({
                "workflow_id": "wf-1",
                "steps": [{"name": "s", "step_type": "Exec", "exec_command": "make"}],
            })
        _, _, body = fake.call_args.args
        self.assertEqual(body["steps"][0]["step_type"], {"type": "Exec"})

    # ── workflow_get ─────────────────────────────────────────────────
    def test_workflow_get_calls_full_route(self):
        fake = mock.MagicMock(return_value=self._env({"id": "wf-1", "steps": []}))
        with mock.patch.object(self.mod, "_http", fake):
            out = self.mod.call_workflow_get({"workflow_id": "wf-1"})
        method, path, *_ = fake.call_args.args
        self.assertEqual((method, path), ("GET", "/api/workflows/wf-1"))
        self.assertEqual(out["id"], "wf-1")

    def test_workflow_get_requires_id(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_workflow_get({})

    # ── workflow_update ──────────────────────────────────────────────
    def test_workflow_update_forwards_only_patchable_keys(self):
        fake = mock.MagicMock(return_value=self._env({"id": "wf-1"}))
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_workflow_update({"workflow_id": "wf-1", "name": "New", "bogus": 1})
        method, path, body = fake.call_args.args
        self.assertEqual((method, path), ("PUT", "/api/workflows/wf-1"))
        self.assertEqual(body, {"name": "New"})  # bogus stripped, id not in body

    def test_workflow_update_normalizes_variables(self):
        fake = mock.MagicMock(return_value=self._env({"id": "wf-1"}))
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_workflow_update({"workflow_id": "wf-1", "variables": [{"name": "v"}]})
        _, _, body = fake.call_args.args
        self.assertEqual(body["variables"][0]["label"], "v")

    def test_workflow_update_requires_a_patch_field(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_workflow_update({"workflow_id": "wf-1"})

    # ── workflow_clone ───────────────────────────────────────────────
    def test_workflow_clone_export_import_then_disable_and_default_name(self):
        fake_text = mock.MagicMock(return_value='{"kind":"x"}')
        fake_http = mock.MagicMock(side_effect=[
            self._env({"id": "wf-new", "name": "Src"}),               # import
            self._env({"id": "wf-new", "name": "Src (copie)"}),       # PUT
        ])
        with mock.patch.object(self.mod, "_http_text", fake_text), \
             mock.patch.object(self.mod, "_http", fake_http):
            self.mod.call_workflow_clone({"workflow_id": "wf-1", "project_id": "proj-1"})
        self.assertEqual(fake_text.call_args.args, ("GET", "/api/workflows/wf-1/export"))
        m1, p1, b1 = fake_http.call_args_list[0].args
        self.assertEqual((m1, p1), ("POST", "/api/workflows/import"))
        self.assertEqual(b1["content"], '{"kind":"x"}')
        self.assertEqual(b1["project_id"], "proj-1")
        m2, p2, b2 = fake_http.call_args_list[1].args
        self.assertEqual((m2, p2), ("PUT", "/api/workflows/wf-new"))
        self.assertFalse(b2["enabled"])                  # clone never auto-fires
        self.assertEqual(b2["name"], "Src (copie)")      # default distinct name

    def test_workflow_clone_respects_new_name(self):
        fake_text = mock.MagicMock(return_value="{}")
        fake_http = mock.MagicMock(side_effect=[
            self._env({"id": "wf-new", "name": "Src"}),
            self._env({"id": "wf-new"}),
        ])
        with mock.patch.object(self.mod, "_http_text", fake_text), \
             mock.patch.object(self.mod, "_http", fake_http):
            self.mod.call_workflow_clone({"workflow_id": "wf-1", "new_name": "Custom", "project_id": "p"})
        self.assertEqual(fake_http.call_args_list[1].args[2]["name"], "Custom")

    def test_workflow_clone_requires_id(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_workflow_clone({})

    # ── workflow_set_enabled ─────────────────────────────────────────
    def test_set_enabled_disable_puts_directly_without_get(self):
        fake = mock.MagicMock(return_value=self._env({"id": "wf-1", "enabled": False}))
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_workflow_set_enabled({"workflow_id": "wf-1", "enabled": False})
        self.assertEqual(fake.call_count, 1)             # no trigger GET when disabling
        method, path, body = fake.call_args.args
        self.assertEqual((method, path), ("PUT", "/api/workflows/wf-1"))
        self.assertFalse(body["enabled"])

    def test_set_enabled_manual_enables(self):
        fake = mock.MagicMock(side_effect=[
            self._env({"id": "wf-1", "trigger": {"type": "Manual"}}),   # GET
            self._env({"id": "wf-1", "enabled": True}),                  # PUT
        ])
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_workflow_set_enabled({"workflow_id": "wf-1", "enabled": True})
        self.assertEqual(fake.call_args_list[1].args[:2], ("PUT", "/api/workflows/wf-1"))

    def test_set_enabled_cron_refused_without_force(self):
        fake = mock.MagicMock(return_value=self._env(
            {"id": "wf-1", "trigger": {"type": "Cron", "schedule": "* * * * *"}}))
        with mock.patch.object(self.mod, "_http", fake):
            with self.assertRaises(RuntimeError) as ctx:
                self.mod.call_workflow_set_enabled({"workflow_id": "wf-1", "enabled": True})
        self.assertIn("Cron", str(ctx.exception))
        self.assertEqual(fake.call_count, 1)             # GET only, never PUT

    def test_set_enabled_cron_allowed_with_force_skips_guard(self):
        fake = mock.MagicMock(return_value=self._env({"id": "wf-1", "enabled": True}))
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_workflow_set_enabled({"workflow_id": "wf-1", "enabled": True, "force": True})
        self.assertEqual(fake.call_count, 1)             # force → straight PUT, no GET
        self.assertEqual(fake.call_args.args[:2], ("PUT", "/api/workflows/wf-1"))

    def test_set_enabled_requires_enabled(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_workflow_set_enabled({"workflow_id": "wf-1"})

    # ── qp_update ────────────────────────────────────────────────────
    def test_qp_update_merges_patch_over_existing(self):
        fake = mock.MagicMock(side_effect=[
            self._env([{"id": "qp-1", "name": "Old", "prompt_template": "T", "agent": "ClaudeCode"}]),
            self._env({"id": "qp-1", "name": "Old", "prompt_template": "NEW"}),
        ])
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_qp_update({"qp_id": "qp-1", "prompt_template": "NEW"})
        m, p, body = fake.call_args_list[1].args
        self.assertEqual((m, p), ("PUT", "/api/quick-prompts/qp-1"))
        self.assertEqual(body["prompt_template"], "NEW")  # patched
        self.assertEqual(body["name"], "Old")             # preserved from existing

    def test_qp_update_not_found_raises(self):
        fake = mock.MagicMock(return_value=self._env([{"id": "other"}]))
        with mock.patch.object(self.mod, "_http", fake):
            with self.assertRaises(RuntimeError):
                self.mod.call_qp_update({"qp_id": "qp-1", "name": "x"})

    def test_qp_update_requires_id(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_qp_update({})

    # ── qp_delete ────────────────────────────────────────────────────
    def test_qp_delete_calls_delete_route(self):
        fake = mock.MagicMock(return_value=self._env(None))
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_qp_delete({"qp_id": "qp-1"})
        self.assertEqual(fake.call_args.args[:2], ("DELETE", "/api/quick-prompts/qp-1"))

    def test_qp_delete_requires_id(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_qp_delete({})

    # ── qp_get (read the FULL QP incl prompt_template) ───────────────
    def test_qp_get_returns_full_qp_with_template(self):
        full = {"id": "qp-1", "name": "Triage", "prompt_template": "Analyse {{ticket}} …",
                "variables": [{"name": "ticket"}], "agent": "ClaudeCode", "skill_ids": ["s1"]}
        fake = mock.MagicMock(return_value=self._env([full, {"id": "other"}]))
        with mock.patch.object(self.mod, "_http", fake):
            out = self.mod.call_qp_get({"qp_id": "qp-1"})
        # the FULL body (which qp_list drops) is returned
        self.assertEqual(out["prompt_template"], "Analyse {{ticket}} …")
        self.assertEqual(out["agent"], "ClaudeCode")
        self.assertEqual(fake.call_args.args[:2], ("GET", "/api/quick-prompts"))

    def test_qp_get_not_found_raises(self):
        fake = mock.MagicMock(return_value=self._env([{"id": "other"}]))
        with mock.patch.object(self.mod, "_http", fake):
            with self.assertRaises(RuntimeError):
                self.mod.call_qp_get({"qp_id": "qp-1"})

    def test_qp_get_requires_id(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_qp_get({})

    # ── workflow_create_draft must document the FULL step_type taxonomy ──
    # (2026-06-24) An agent missed that SubWorkflow exists because it inferred
    # the available types from one workflow it opened. The create_draft tool
    # description must enumerate the whole closed set so no agent generalises
    # from a poor sample. If a new StepType ships, this fails → document it.
    def test_workflow_create_draft_documents_all_step_types(self):
        entry = next(t for t in self.mod.TOOLS if t["name"] == "workflow_create_draft")
        desc = entry["description"]
        for st in ["Agent", "ApiCall", "BatchApiCall", "BatchQuickPrompt",
                   "Exec", "Gate", "Notify", "JsonData", "SubWorkflow"]:
            self.assertIn(st, desc, f"step_type '{st}' must be documented in workflow_create_draft")

    # ── initialize `instructions` must ORIENT the agent (what Kronn is + a
    # map of the tool areas + how to navigate), so it doesn't reverse-engineer
    # capabilities from 40+ tools or generalise from one sample. (2026-06-24)
    def test_initialize_instructions_orient_the_agent(self):
        resp = self.mod._handle({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})
        instr = resp["result"]["instructions"]
        for needle in ["Kronn", "Discussions", "Workflows", "Quick Prompts",
                       "workflow_get", "qp_get", "workflow_create_draft", "Navigation"]:
            self.assertIn(needle, instr, f"MCP orientation must mention {needle}")


class StepSchemaAndBindingListTests(unittest.TestCase):
    """0.8.8 (2026-06-24) — `workflow_step_schema` (canonical schema as an
    untruncatable tool RESULT) + the Agent-step binding catalogs
    (`skills_list`/`profiles_list`/`directives_list`). Closes the gap where the
    only schema doc was a client-truncatable tool description and the binding
    ids couldn't be enumerated at all."""

    def setUp(self):
        self.mod = _load_module()

    @staticmethod
    def _env(data):
        return {"success": True, "data": data}

    # ── workflow_step_schema ─────────────────────────────────────────
    def test_step_schema_lists_the_closed_nine_set(self):
        out = self.mod.call_workflow_step_schema({})
        self.assertEqual(
            set(out["step_types_closed_set"]),
            {"Agent", "ApiCall", "BatchApiCall", "BatchQuickPrompt", "Exec",
             "Gate", "Notify", "JsonData", "SubWorkflow"},
        )
        # every type has a field spec
        for st in out["step_types_closed_set"]:
            self.assertIn(st, out["fields_by_type"])

    def test_step_schema_documents_foreach_runtime_contract(self):
        out = self.mod.call_workflow_step_schema({})
        contract = out["fields_by_type"]["SubWorkflow"]["FOREACH_RUNTIME_CONTRACT"]
        # the run-breaking fact: fixed target name, not derived from source
        self.assertIn(".kronn/current_task.json", contract)
        self.assertIn("FIXED", contract)

    def test_step_schema_is_a_tool_and_dispatchable(self):
        self.assertIn("workflow_step_schema", self.mod.DISPATCH)
        names = [t["name"] for t in self.mod.TOOLS]
        self.assertIn("workflow_step_schema", names)

    def test_step_schema_exec_documents_stdin(self):
        out = self.mod.call_workflow_step_schema({})
        opt = " ".join(out["fields_by_type"]["Exec"]["optional"])
        self.assertIn("exec_stdin", opt)

    def test_step_schema_warns_batchqp_output_is_metadata_only(self):
        out = self.mod.call_workflow_step_schema({})
        bqp = out["fields_by_type"]["BatchQuickPrompt"]["OUTPUT_IS_METADATA_ONLY"]
        # the trap: data is counters, the produced content is in discussions
        self.assertIn("metadata", bqp.lower())
        self.assertIn("discussions", bqp.lower())
        self.assertIn("results[]", bqp)

    def test_step_schema_agent_documents_typedschema_piping(self):
        out = self.mod.call_workflow_step_schema({})
        piping = out["fields_by_type"]["Agent"]["OUTPUT_PIPING"]
        # the accessor that DOES pipe an LLM result into a deterministic step
        self.assertIn("{{steps.<name>.data}}", piping)
        self.assertIn("TypedSchema", piping)

    def test_step_schema_documents_template_var_namespaces(self):
        out = self.mod.call_workflow_step_schema({})
        tv = out["template_vars"]
        ns = tv["namespaces"]
        # the namespaces an automating agent must know to wire a WF end-to-end
        for key in ["steps.<name>.data", "steps.<name>.data_json",
                    "previous_step.{output,data,data_json,summary,status}",
                    "current_task / current_task.<field>", "state.<key>",
                    "artifacts.<name>", "issue.{title,body,number,url,labels}"]:
            self.assertIn(key, ns)
        # api_body typed injection must be documented (the inline-array footgun)
        self.assertIn("typed JSON", ns["steps.<name>.data"])
        self.assertIn("alias", ns["steps.<name>.data_json"])

    # ── skills_list / profiles_list / directives_list ────────────────
    def test_skills_list_is_lean_and_drops_content(self):
        fake = self._env([
            {"id": "sk-1", "name": "Reviewer", "description": "d",
             "category": "code", "is_builtin": True, "token_estimate": 120,
             "content": "# huge markdown body", "icon": "x", "license": "MIT"},
        ])
        with mock.patch.object(self.mod, "_http", return_value=fake):
            out = self.mod.call_skills_list({})
        self.assertEqual(out[0]["id"], "sk-1")
        self.assertIn("name", out[0])
        self.assertNotIn("content", out[0], "skills_list must drop the markdown body")

    def test_profiles_list_drops_persona_prompt(self):
        fake = self._env([
            {"id": "pr-1", "name": "Archi", "role": "architect",
             "persona_name": "Ada", "category": "eng", "default_engine": "ClaudeCode",
             "is_builtin": True, "token_estimate": 80, "persona_prompt": "long..."},
        ])
        with mock.patch.object(self.mod, "_http", return_value=fake):
            out = self.mod.call_profiles_list({})
        self.assertEqual(out[0]["role"], "architect")
        self.assertNotIn("persona_prompt", out[0])

    def test_directives_list_keeps_conflicts_drops_content(self):
        fake = self._env([
            {"id": "di-1", "name": "Terse", "description": "be brief",
             "category": "style", "conflicts": ["di-2"], "is_builtin": True,
             "token_estimate": 30, "content": "long body", "icon": "x"},
        ])
        with mock.patch.object(self.mod, "_http", return_value=fake):
            out = self.mod.call_directives_list({})
        self.assertEqual(out[0]["conflicts"], ["di-2"])
        self.assertNotIn("content", out[0])

    def test_binding_list_tools_are_registered(self):
        names = [t["name"] for t in self.mod.TOOLS]
        for n in ["skills_list", "profiles_list", "directives_list"]:
            self.assertIn(n, names)
            self.assertIn(n, self.mod.DISPATCH)

    def test_orientation_mentions_step_schema_and_agent_library_reads(self):
        resp = self.mod._handle({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})
        instr = resp["result"]["instructions"]
        for needle in [
            "workflow_step_schema",
            "skills_list",
            "profiles_list",
            "directives_list",
            "skill_get",
            "profile_get",
            "directive_get",
        ]:
            self.assertIn(needle, instr)

    def test_step_schema_foreach_contract_documents_current_task_template_vars(self):
        out = self.mod.call_workflow_step_schema({})
        contract = out["fields_by_type"]["SubWorkflow"]["FOREACH_RUNTIME_CONTRACT"]
        # the templating accessor an agent kept guessing — now pinned
        self.assertIn("{{current_task.<field>}}", contract)
        self.assertIn("current_task.number", contract)
        # explicitly rules OUT the wrong guesses
        self.assertIn("item.*", contract)


class WorkflowRunHistoryTests(unittest.TestCase):
    """0.8.8 (2026-06-25) — run history (`workflow_runs`), per-run detail
    (`workflow_run_get`), and cancel (`workflow_cancel_run`). Closes the gap
    where only active/latest runs were reachable via MCP."""

    def setUp(self):
        self.mod = _load_module()

    @staticmethod
    def _env(data):
        return {"success": True, "data": data}

    def test_workflow_runs_is_lean_keeps_parent_run_id_drops_steps(self):
        runs = [{
            "id": "run-1", "workflow_id": "wf", "status": "Success", "run_type": "cron",
            "started_at": "t0", "finished_at": "t1", "tokens_used": 12,
            "batch_total": 0, "parent_run_id": None,
            "step_results": [{"output": "x" * 9999}], "state": {"big": "y"},
        }]
        with mock.patch.object(self.mod, "_http", return_value=self._env(runs)):
            out = self.mod.call_workflow_runs({"workflow_id": "wf"})
        self.assertEqual(out[0]["id"], "run-1")
        self.assertEqual(out[0]["status"], "Success")
        self.assertIn("parent_run_id", out[0], "parent_run_id kept (child enumeration)")
        self.assertNotIn("step_results", out[0], "heavy step_results dropped from the list")
        self.assertNotIn("state", out[0])

    def test_workflow_runs_limit_truncates(self):
        runs = [{"id": f"r{i}"} for i in range(10)]
        with mock.patch.object(self.mod, "_http", return_value=self._env(runs)):
            out = self.mod.call_workflow_runs({"workflow_id": "wf", "limit": 3})
        self.assertEqual(len(out), 3)

    def test_workflow_runs_forwards_history_route(self):
        fake = mock.MagicMock(return_value=self._env([]))
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_workflow_runs({"workflow_id": "wf-9"})
        method, path = fake.call_args.args[:2]
        self.assertEqual((method, path), ("GET", "/api/workflows/wf-9/runs"))

    def test_workflow_run_get_truncates_long_step_output(self):
        run = {"id": "r", "step_results": [
            {"step_name": "big", "status": "Failed", "duration_ms": 5,
             "output": "E" * 5000},
        ]}
        with mock.patch.object(self.mod, "_http", return_value=self._env(run)):
            out = self.mod.call_workflow_run_get({"workflow_id": "wf", "run_id": "r"})
        s = out["step_results"][0]
        self.assertEqual(s["step_name"], "big")
        self.assertLess(len(s["output"]), 2000)
        self.assertIn("truncated", s["output"])

    def test_workflow_run_get_requires_both_ids(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_workflow_run_get({"workflow_id": "wf"})

    def test_workflow_cancel_run_posts_cancel_route(self):
        fake = mock.MagicMock(return_value=self._env({"cancelled": True}))
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_workflow_cancel_run({"workflow_id": "wf", "run_id": "r5"})
        method, path = fake.call_args.args[:2]
        self.assertEqual((method, path), ("POST", "/api/workflows/wf/runs/r5/cancel"))

    def test_run_history_tools_registered(self):
        names = [t["name"] for t in self.mod.TOOLS]
        for n in ["workflow_runs", "workflow_run_get", "workflow_cancel_run"]:
            self.assertIn(n, names)
            self.assertIn(n, self.mod.DISPATCH)


class AgentLibraryCrudTests(unittest.TestCase):
    """0.8.8 (2026-06-24) — create/update/delete for skills · profiles ·
    directives (the Agent-step bindings). Load-merge-write update, category
    validation, custom-only enforced server-side. Closes the loop with the
    `*s_list` read tools."""

    def setUp(self):
        self.mod = _load_module()

    @staticmethod
    def _env(data):
        return {"success": True, "data": data}

    # ── create ───────────────────────────────────────────────────────
    def test_skill_create_posts_required_and_optional_fields(self):
        fake = mock.MagicMock(return_value=self._env({"id": "custom-x", "name": "Rev"}))
        with mock.patch.object(self.mod, "_http", fake):
            out = self.mod.call_skill_create({
                "name": "Rev", "description": "d", "icon": "🔍",
                "category": "Domain", "content": "# body", "license": "MIT",
            })
        method, path, body = fake.call_args.args
        self.assertEqual((method, path), ("POST", "/api/skills"))
        self.assertEqual(body["category"], "Domain")
        self.assertEqual(body["license"], "MIT")
        self.assertEqual(out["id"], "custom-x")

    def test_skill_create_rejects_bad_category(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_skill_create({
                "name": "x", "description": "d", "icon": "i",
                "category": "Nope", "content": "c",
            })

    def test_create_missing_required_raises(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_directive_create({"name": "x"})  # missing description/icon/category/content

    def test_profile_create_posts_to_profiles(self):
        fake = mock.MagicMock(return_value=self._env({"id": "custom-p"}))
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_profile_create({
                "name": "Archi", "role": "architect", "avatar": "🏛",
                "color": "#6C5CE7", "category": "Technical", "persona_prompt": "p",
            })
        method, path, _ = fake.call_args.args
        self.assertEqual((method, path), ("POST", "/api/profiles"))

    # ── get (full body; list remains lean) ───────────────────────────
    def test_skill_get_returns_full_markdown_body(self):
        full = {
            "id": "custom-review", "name": "Review", "content": "# Full body",
            "license": "MIT", "allowed_tools": "Read, Grep",
        }
        fake = mock.MagicMock(return_value=self._env([full]))
        with mock.patch.object(self.mod, "_http", fake):
            out = self.mod.call_skill_get({"skill_id": "custom-review"})
        self.assertEqual(out, full)
        self.assertEqual(fake.call_args.args, ("GET", "/api/skills"))

    def test_profile_and_directive_get_return_omitted_bodies(self):
        cases = [
            ("profile", self.mod.call_profile_get, "/api/profiles",
             {"id": "p-1", "persona_prompt": "You are an architect"}),
            ("directive", self.mod.call_directive_get, "/api/directives",
             {"id": "d-1", "content": "Always cite sources"}),
        ]
        for kind, fn, path, full in cases:
            with self.subTest(kind=kind):
                fake = mock.MagicMock(return_value=self._env([full]))
                with mock.patch.object(self.mod, "_http", fake):
                    out = fn({"id": full["id"]})
                self.assertEqual(out, full)
                self.assertEqual(fake.call_args.args, ("GET", path))

    def test_get_rejects_missing_or_unknown_id(self):
        with self.assertRaisesRegex(RuntimeError, "skill_id"):
            self.mod.call_skill_get({})
        with mock.patch.object(self.mod, "_http", return_value=self._env([])):
            with self.assertRaisesRegex(RuntimeError, "not found"):
                self.mod.call_directive_get({"directive_id": "missing"})

    # ── update (load-merge-write) ─────────────────────────────────────
    def test_directive_update_merges_over_existing(self):
        existing = [{
            "id": "custom-d", "name": "Old", "description": "od", "icon": "i",
            "category": "Output", "content": "oc", "conflicts": ["x"], "is_builtin": False,
        }]
        calls = []

        def fake_http(method, path, body=None):
            calls.append((method, path, body))
            if method == "GET":
                return self._env(existing)
            return self._env({"id": "custom-d", "name": "New"})

        with mock.patch.object(self.mod, "_http", fake_http):
            self.mod.call_directive_update({"directive_id": "custom-d", "name": "New"})

        put = next(c for c in calls if c[0] == "PUT")
        self.assertEqual(put[1], "/api/directives/custom-d")
        # patched field overridden, untouched fields carried from existing
        self.assertEqual(put[2]["name"], "New")
        self.assertEqual(put[2]["content"], "oc")
        self.assertEqual(put[2]["conflicts"], ["x"])

    def test_update_unknown_id_raises(self):
        with mock.patch.object(self.mod, "_http", return_value=self._env([])):
            with self.assertRaises(RuntimeError):
                self.mod.call_skill_update({"skill_id": "nope"})

    # ── delete ────────────────────────────────────────────────────────
    def test_profile_delete_calls_delete_route(self):
        fake = mock.MagicMock(return_value=self._env(True))
        with mock.patch.object(self.mod, "_http", fake):
            self.mod.call_profile_delete({"profile_id": "custom-p"})
        method, path = fake.call_args.args[:2]
        self.assertEqual((method, path), ("DELETE", "/api/profiles/custom-p"))

    def test_delete_missing_id_raises(self):
        with self.assertRaises(RuntimeError):
            self.mod.call_skill_delete({})

    # ── registration ──────────────────────────────────────────────────
    def test_all_crud_tools_registered(self):
        names = [t["name"] for t in self.mod.TOOLS]
        for n in ["skill_get", "skill_create", "skill_update", "skill_delete",
                  "profile_get", "profile_create", "profile_update", "profile_delete",
                  "directive_get", "directive_create", "directive_update", "directive_delete"]:
            self.assertIn(n, names, f"{n} missing from TOOLS")
            self.assertIn(n, self.mod.DISPATCH, f"{n} missing from DISPATCH")


class DiscListTests(unittest.TestCase):
    """F12 — disc_list browses available discussions, compact + newest-first,
    shared-only by default."""

    def setUp(self):
        self.mod = _load_module()

    def _fake(self, discs):
        return {"success": True, "data": discs, "error": None}

    def test_registered_in_tools_and_dispatch(self):
        names = [t["name"] for t in self.mod.TOOLS]
        self.assertIn("disc_list", names)
        self.assertIn("disc_list", self.mod.DISPATCH)

    def test_shared_only_by_default_and_newest_first(self):
        discs = [
            {"id": "d1", "title": "Local", "shared_id": None, "message_count": 2,
             "updated_at": "2026-06-29T10:00:00+00:00"},
            {"id": "d2", "title": "Shared A", "shared_id": "s-a", "message_count": 5,
             "updated_at": "2026-06-29T09:00:00+00:00"},
            {"id": "d3", "title": "Shared B", "shared_id": "s-b", "message_count": 1,
             "updated_at": "2026-06-29T11:00:00+00:00"},
        ]
        with mock.patch.object(self.mod, "_http", return_value=self._fake(discs)):
            out = self.mod.call_disc_list({})
        # Only shared discs, newest-first (d3 > d2); local d1 excluded.
        self.assertEqual(out["disc_count"], 2)
        self.assertEqual([d["disc_id"] for d in out["discussions"]], ["d3", "d2"])
        self.assertEqual(out["discussions"][0]["shared_id"], "s-b")
        self.assertEqual(out["discussions"][0]["message_count"], 1)

    def test_shared_only_false_includes_local(self):
        discs = [
            {"id": "d1", "title": "Local", "shared_id": None, "message_count": 0,
             "updated_at": "2026-06-29T10:00:00+00:00"},
            {"id": "d2", "title": "Shared", "shared_id": "s", "message_count": 0,
             "updated_at": "2026-06-29T09:00:00+00:00"},
        ]
        with mock.patch.object(self.mod, "_http", return_value=self._fake(discs)):
            out = self.mod.call_disc_list({"shared_only": False})
        self.assertEqual(out["disc_count"], 2)

    def test_limit_caps_results(self):
        discs = [
            {"id": f"d{i}", "title": "x", "shared_id": f"s{i}", "message_count": 0,
             "updated_at": f"2026-06-29T{i:02d}:00:00+00:00"}
            for i in range(10)
        ]
        with mock.patch.object(self.mod, "_http", return_value=self._fake(discs)):
            out = self.mod.call_disc_list({"limit": 3})
        self.assertEqual(out["disc_count"], 3)


class HttpAuthHeaderTests(unittest.TestCase):
    """0.8.11 — the sidecar authenticates to the backend: `_http`/`_http_text`
    send `Authorization: Bearer <token>` iff KRONN_AUTH_TOKEN is set. This is the
    contract that makes an auth-enabled / LAN-exposed backend reachable by its
    own sidecar (was a silent 401 before the boot injects the token)."""

    def setUp(self):
        self.mod = _load_module()

    def _ok_response(self):
        cm = mock.MagicMock()
        cm.__enter__.return_value.read.return_value = b'{"success": true, "data": {}}'
        cm.__exit__.return_value = False
        return cm

    def test_http_adds_bearer_when_token_present(self):
        with mock.patch.dict(os.environ,
                             {"KRONN_BACKEND_URL": "http://127.0.0.1:3140", "KRONN_AUTH_TOKEN": "sekret"},
                             clear=True), \
             mock.patch("urllib.request.urlopen", return_value=self._ok_response()) as urlopen:
            self.mod._http("GET", "/api/health")
        req = urlopen.call_args.args[0]
        self.assertEqual(req.get_header("Authorization"), "Bearer sekret")

    def test_http_omits_auth_when_no_token(self):
        with mock.patch.dict(os.environ,
                             {"KRONN_BACKEND_URL": "http://127.0.0.1:3140"},
                             clear=True), \
             mock.patch("urllib.request.urlopen", return_value=self._ok_response()) as urlopen:
            self.mod._http("GET", "/api/health")
        req = urlopen.call_args.args[0]
        self.assertIsNone(req.get_header("Authorization"))

    def test_current_disc_meta_adds_bearer_when_token_present(self):
        # Regression (Codex audit 2026-07-12): this read used a bare urlopen —
        # on an auth-enforced instance it 401'd silently and project/agent
        # inheritance fell back to defaults.
        cm = mock.MagicMock()
        cm.__enter__.return_value.read.return_value = b'{"success": true, "data": {"project_id": "p-1"}}'
        cm.__exit__.return_value = False
        with mock.patch.dict(os.environ,
                             {"KRONN_BACKEND_URL": "http://127.0.0.1:3140",
                              "KRONN_AUTH_TOKEN": "sekret",
                              "KRONN_DISCUSSION_ID": "d-1"},
                             clear=True), \
             mock.patch("urllib.request.urlopen", return_value=cm) as urlopen:
            meta = self.mod._current_disc_meta()
        req = urlopen.call_args.args[0]
        self.assertEqual(req.get_header("Authorization"), "Bearer sekret")
        self.assertEqual(meta.get("project_id"), "p-1")

    def test_http_text_adds_bearer_when_token_present(self):
        cm = mock.MagicMock()
        cm.__enter__.return_value.read.return_value = b'{"kind":"kronn.workflow"}'
        cm.__exit__.return_value = False
        with mock.patch.dict(os.environ,
                             {"KRONN_BACKEND_URL": "http://127.0.0.1:3140", "KRONN_AUTH_TOKEN": "tok2"},
                             clear=True), \
             mock.patch("urllib.request.urlopen", return_value=cm) as urlopen:
            self.mod._http_text("GET", "/api/workflows/x/export")
        req = urlopen.call_args.args[0]
        self.assertEqual(req.get_header("Authorization"), "Bearer tok2")


class AuditToolsTests(unittest.TestCase):
    """0.8.12 PR A — audit_prepare / audit_launch / audit_status.

    The audit endpoints are SSE-driven; these tests exercise the bridge
    side only (state machine, lifecycle, error surfacing) against fake
    streams — never a real backend, never a 20-min wait.
    """

    def setUp(self):
        self.mod = _load_module()
        self.mod._AUDIT_STREAMS.clear()

    # ── fake SSE plumbing ────────────────────────────────────────────

    class _FakeSse:
        """Iterable of pre-encoded SSE lines + a close() flag. An entry of
        `Exception` in `lines` is RAISED at that point (mid-stream cut)."""

        def __init__(self, lines):
            self._lines = list(lines)
            self.closed = False

        def __iter__(self):
            for item in self._lines:
                if isinstance(item, Exception):
                    raise item
                yield item

        def close(self):
            self.closed = True

    @staticmethod
    def _sse(event, payload):
        import json as _json
        return [
            f"event: {event}\n".encode(),
            f"data: {_json.dumps(payload)}\n".encode(),
            b"\n",
        ]

    def _wait_state(self, project_id, states, timeout=3.0):
        import time as _time
        deadline = _time.time() + timeout
        while _time.time() < deadline:
            entry = self.mod._AUDIT_STREAMS.get(project_id)
            if entry and entry["state"] in states:
                return entry
            _time.sleep(0.01)
        self.fail(f"entry never reached {states}: {self.mod._AUDIT_STREAMS.get(project_id)}")

    # ── audit_prepare ────────────────────────────────────────────────

    def test_terminal_entries_are_purged_after_their_ttl(self):
        # PR C — a long-lived bridge must not accumulate dead entries; the
        # freshest terminal memory survives until the TTL.
        import time as _time
        with self.mod._AUDIT_LOCK:
            self.mod._AUDIT_STREAMS["p-old"] = {
                "state": "done",
                "_ended_monotonic": _time.monotonic() - self.mod._AUDIT_TERMINAL_TTL_SECONDS - 10,
            }
            self.mod._AUDIT_STREAMS["p-fresh"] = {
                "state": "error", "_ended_monotonic": _time.monotonic() - 60,
            }
            self.mod._AUDIT_STREAMS["p-live"] = {"state": "running"}
        self.mod._audit_purge_terminal_entries()
        self.assertNotIn("p-old", self.mod._AUDIT_STREAMS, "expired terminal purged")
        self.assertIn("p-fresh", self.mod._AUDIT_STREAMS, "fresh terminal kept (bridge memory)")
        self.assertIn("p-live", self.mod._AUDIT_STREAMS, "live entry never purged")

    def test_audit_prepare_preserves_the_three_top_level_keys(self):
        info = {"files": [], "todos": [{"t": 1}], "tech_debt_items": []}
        with mock.patch.object(self.mod, "_http", return_value={"success": True, "data": info}):
            out = self.mod.call_audit_prepare({"project_id": "p1"})
        self.assertEqual(out, info, "AuditInfo must round-trip verbatim")

    # ── audit_status ─────────────────────────────────────────────────

    def test_audit_status_null_live_falls_back_to_db_and_says_so(self):
        # live=null must NEVER read as "finished" — the tool falls back to
        # latest/resumable and states the ambiguity explicitly.
        def fake_http(method, path, body=None):
            if path.endswith("/audit-status"):
                return {"success": True, "data": None}
            if path.endswith("/audit-latest"):
                return {"success": True, "data": {"run_id": "r-1", "status": "Completed"}}
            if path.endswith("/audit-resumable"):
                return {"success": True, "data": None}
            raise AssertionError(f"unexpected path {path}")

        with mock.patch.object(self.mod, "_http", side_effect=fake_http):
            out = self.mod.call_audit_status({"project_id": "p1"})
        self.assertIsNone(out["live"])
        self.assertIsNone(out["bridge_stream"], "no local stream ever opened")
        self.assertEqual(out["latest"]["run_id"], "r-1")
        self.assertIn("never means 'completed'", out["note"])

    # ── audit_launch ─────────────────────────────────────────────────

    def test_partial_with_empty_steps_is_refused_before_any_http(self):
        with mock.patch.object(self.mod, "_audit_open_sse",
                               side_effect=AssertionError("must not be called")):
            for bad in ([], None, [0], ["2"]):
                with self.assertRaises(RuntimeError):
                    self.mod.call_audit_launch(
                        {"project_id": "p1", "mode": "partial", "steps": bad})

    def test_blank_agent_falls_back_to_the_session_agent_type(self):
        # Copilot round 5: a whitespace `agent` must behave like an absent
        # one — the body sent to the backend carries the session's type.
        captured = {}

        def fake_open(path, body):
            captured["body"] = body
            return self._FakeSse(self._sse("start", {"total_steps": 10}))

        with mock.patch.object(self.mod, "_audit_open_sse", side_effect=fake_open), \
             mock.patch.object(self.mod, "_agent_type_for_session", return_value="ClaudeCode"):
            self.mod.call_audit_launch({"project_id": "p1", "mode": "full", "agent": "   "})
        self.assertEqual(captured["body"]["agent"], "ClaudeCode")

    def test_launch_returns_fast_on_the_start_event(self):
        import time as _time
        stream = self._FakeSse(self._sse("start", {"total_steps": 10,
                                                   "started_at": "2026-07-14T13:30:00Z"}))
        t0 = _time.time()
        with mock.patch.object(self.mod, "_audit_open_sse", return_value=stream):
            out = self.mod.call_audit_launch({"project_id": "p1", "mode": "full"})
        self.assertLess(_time.time() - t0, 4.0, "launch must not block on the audit")
        self.assertTrue(out["launched"])
        self.assertEqual(out["total_steps"], 10)
        self.assertIn("reload", out["lifecycle_warning"])

    def test_accepted_event_confirms_launch_before_start(self):
        # Codex #7 — `start` only fires after Phase 1 (template install /
        # migration), which on a fresh project can outlast the 5s start-wait.
        # The `accepted` event emitted BEFORE Phase 1 must confirm the launch
        # on its own, so a slow install no longer trips the launch timeout and
        # interrupts a healthy audit. total_steps is still absent here (that
        # rides on the later `start`).
        import time as _time
        stream = self._FakeSse(self._sse("accepted",
                                         {"audit_run_id": "run-acc", "kind": "Full"}))
        t0 = _time.time()
        with mock.patch.object(self.mod, "_audit_open_sse", return_value=stream):
            out = self.mod.call_audit_launch({"project_id": "p1", "mode": "full"})
        self.assertLess(_time.time() - t0, 4.0, "accepted must confirm without waiting on start")
        self.assertTrue(out["launched"])
        self.assertIsNone(out["total_steps"], "total_steps rides on the later start event")

    def test_backend_early_error_surfaces_as_a_distinct_mcp_error(self):
        # The "already running" refusal arrives as an SSE error event — it
        # must raise, never return a hollow `launched`.
        stream = self._FakeSse(self._sse("error", {
            "error": "Audit already running for this project"}))
        with mock.patch.object(self.mod, "_audit_open_sse", return_value=stream):
            with self.assertRaises(RuntimeError) as ctx:
                self.mod.call_audit_launch({"project_id": "p1", "mode": "full"})
        self.assertIn("already running", str(ctx.exception))

    def test_second_local_launch_is_refused_while_one_runs(self):
        with self.mod._AUDIT_LOCK:
            self.mod._AUDIT_STREAMS["p1"] = {"project_id": "p1", "state": "running"}
        with mock.patch.object(self.mod, "_audit_open_sse",
                               side_effect=AssertionError("must not be called")):
            with self.assertRaises(RuntimeError) as ctx:
                self.mod.call_audit_launch({"project_id": "p1", "mode": "full"})
        self.assertIn("already", str(ctx.exception))

    def test_reader_captures_discussion_and_run_id_from_done(self):
        lines = (self._sse("start", {"total_steps": 10})
                 + self._sse("done", {"status": "complete",
                                      "discussion_id": "d-val",
                                      "audit_run_id": "run-42"}))
        with mock.patch.object(self.mod, "_audit_open_sse",
                               return_value=self._FakeSse(lines)):
            self.mod.call_audit_launch({"project_id": "p1", "mode": "full"})
        entry = self._wait_state("p1", {"done"})
        self.assertEqual(entry["discussion_id"], "d-val")
        self.assertEqual(entry["audit_run_id"], "run-42")

    def test_partial_success_done_carries_discussion_and_run_id(self):
        # A5 v3 contract change: a FULLY-successful partial creates a scoped
        # validation discussion — the done event carries both ids and the
        # bridge must expose them like it does for full.
        lines = (self._sse("start", {"total_steps": 2, "requested_steps": [3, 8]})
                 + self._sse("warning", {"message": "Baseline write failed: disk full"})
                 + self._sse("done", {"status": "complete",
                                      "succeeded_steps": [3, 8],
                                      "unchanged_steps": [], "failed_steps": [],
                                      "discussion_id": "d-partial-val",
                                      "audit_run_id": "run-partial-7"}))
        with mock.patch.object(self.mod, "_audit_open_sse",
                               return_value=self._FakeSse(lines)):
            self.mod.call_audit_launch(
                {"project_id": "p1", "mode": "partial", "steps": [3, 8]})
        entry = self._wait_state("p1", {"done"})
        self.assertEqual(entry["discussion_id"], "d-partial-val")
        self.assertEqual(entry["audit_run_id"], "run-partial-7")
        # The warning is non-terminal: captured, and the state is still done.
        self.assertIn("Baseline write failed", entry["last_warning"])

    def test_partial_no_change_done_carries_partition_and_status(self):
        # Matrix v2: an all-unchanged refresh ends `no_change` with the
        # exact requested partition — the bridge captures all of it.
        lines = (self._sse("start", {"total_steps": 2, "requested_steps": [3, 8]})
                 + self._sse("step_unchanged", {"step": 3, "file": "docs/repo-map.md"})
                 + self._sse("done", {"status": "no_change",
                                      "succeeded_steps": [],
                                      "unchanged_steps": [3, 8],
                                      "failed_steps": [],
                                      "audit_run_id": "run-nc"}))
        with mock.patch.object(self.mod, "_audit_open_sse",
                               return_value=self._FakeSse(lines)):
            self.mod.call_audit_launch(
                {"project_id": "p1", "mode": "partial", "steps": [3, 8]})
        entry = self._wait_state("p1", {"done"})
        self.assertEqual(entry["done_status"], "no_change")
        self.assertEqual(entry["requested_steps"], [3, 8])
        self.assertEqual(entry["unchanged_steps"], [3, 8])
        self.assertEqual(entry["succeeded_steps"], [])
        self.assertEqual(entry["failed_steps"], [])
        self.assertIsNone(entry["discussion_id"])

    def test_partial_bool_forged_step_lists_are_refused(self):
        # Red-team (Codex msg 162): True == 1 in Python — a bool-forged
        # partition must not pass a validator the frontend would refuse.
        lines = (self._sse("start", {"total_steps": 1, "requested_steps": [1]})
                 + self._sse("done", {"status": "complete",
                                      "succeeded_steps": [True],
                                      "unchanged_steps": [], "failed_steps": [],
                                      "discussion_id": "d-x",
                                      "audit_run_id": "run-x"}))
        with mock.patch.object(self.mod, "_audit_open_sse",
                               return_value=self._FakeSse(lines)):
            self.mod.call_audit_launch(
                {"project_id": "p1", "mode": "partial", "steps": [1]})
        entry = self._wait_state("p1", {"protocol_error"})
        self.assertIn("not a step list", entry["error"])

    def test_partial_non_string_discussion_id_is_refused(self):
        lines = (self._sse("start", {"total_steps": 1, "requested_steps": [1]})
                 + self._sse("done", {"status": "complete",
                                      "succeeded_steps": [1],
                                      "unchanged_steps": [], "failed_steps": [],
                                      "discussion_id": 123,
                                      "audit_run_id": "run-x"}))
        with mock.patch.object(self.mod, "_audit_open_sse",
                               return_value=self._FakeSse(lines)):
            self.mod.call_audit_launch(
                {"project_id": "p1", "mode": "partial", "steps": [1]})
        entry = self._wait_state("p1", {"protocol_error"})
        self.assertIn("validation discussion", entry["error"])

    def test_protocol_error_is_terminal_for_the_ttl_purge(self):
        import time as _time
        with self.mod._AUDIT_LOCK:
            self.mod._AUDIT_STREAMS["p-proto"] = {
                "project_id": "p-proto", "state": "protocol_error",
                "_ended_monotonic": _time.monotonic() - self.mod._AUDIT_TERMINAL_TTL_SECONDS - 10,
            }
        self.mod._audit_purge_terminal_entries()
        self.assertNotIn("p-proto", self.mod._AUDIT_STREAMS,
                         "protocol_error must be purgeable like every terminal state")

    def test_partial_malformed_done_is_a_protocol_error_never_done(self):
        # Mirror of the frontend refusals (api.streaming.test.ts): a done
        # the UI would reject as malformed must never read as a bridge
        # terminal `done` — a `complete` without its validation discussion
        # here (same matrix, same fixtures family).
        lines = (self._sse("start", {"total_steps": 1, "requested_steps": [3]})
                 + self._sse("done", {"status": "complete",
                                      "succeeded_steps": [3],
                                      "unchanged_steps": [], "failed_steps": [],
                                      "audit_run_id": "run-x"}))
        with mock.patch.object(self.mod, "_audit_open_sse",
                               return_value=self._FakeSse(lines)):
            self.mod.call_audit_launch(
                {"project_id": "p1", "mode": "partial", "steps": [3]})
        entry = self._wait_state("p1", {"protocol_error"})
        self.assertIn("malformed done event", entry["error"])
        self.assertIn("validation discussion", entry["error"])
        self.assertNotEqual(entry["state"], "done")

    def test_partial_done_without_run_id_is_refused(self):
        lines = (self._sse("start", {"total_steps": 1, "requested_steps": [3]})
                 + self._sse("done", {"status": "no_change",
                                      "succeeded_steps": [],
                                      "unchanged_steps": [3], "failed_steps": []}))
        with mock.patch.object(self.mod, "_audit_open_sse",
                               return_value=self._FakeSse(lines)):
            self.mod.call_audit_launch(
                {"project_id": "p1", "mode": "partial", "steps": [3]})
        entry = self._wait_state("p1", {"protocol_error"})
        self.assertIn("missing audit_run_id", entry["error"])

    def test_last_step_event_carries_file_and_outcome(self):
        # Matrix v2: step_done carries an explicit outcome and a file — the
        # bridge must surface both, or audit_status cannot explain WHICH
        # section closed HOW without re-reading the stream.
        lines = (self._sse("start", {"total_steps": 1, "requested_steps": [3]})
                 + self._sse("step_done", {"step": 3, "label": "Repo map",
                                           "file": "docs/repo-map.md",
                                           "outcome": "unchanged"})
                 + self._sse("done", {"status": "no_change",
                                      "succeeded_steps": [],
                                      "unchanged_steps": [3],
                                      "failed_steps": [],
                                      "audit_run_id": "run-nc2"}))
        with mock.patch.object(self.mod, "_audit_open_sse",
                               return_value=self._FakeSse(lines)):
            self.mod.call_audit_launch(
                {"project_id": "p1", "mode": "partial", "steps": [3]})
        entry = self._wait_state("p1", {"done"})
        evt = entry["last_step_event"]
        self.assertEqual(evt["event"], "step_done")
        self.assertEqual(evt["file"], "docs/repo-map.md")
        self.assertEqual(evt["outcome"], "unchanged")

    def test_partial_done_without_discussion_id_is_an_explicit_null(self):
        # An INTERRUPTED partial creates no discussion — the field is an
        # explicit null, never absent, and done_status reflects the truth.
        lines = (self._sse("start", {"total_steps": 3, "requested_steps": [1, 2, 3]})
                 + self._sse("done", {"status": "interrupted",
                                      "succeeded_steps": [1], "unchanged_steps": [2],
                                      "failed_steps": [3], "audit_run_id": "run-int"}))
        with mock.patch.object(self.mod, "_audit_open_sse",
                               return_value=self._FakeSse(lines)):
            self.mod.call_audit_launch(
                {"project_id": "p1", "mode": "partial", "steps": [1, 2, 3]})
        entry = self._wait_state("p1", {"done"})
        self.assertIn("discussion_id", entry)
        self.assertIsNone(entry["discussion_id"])
        self.assertEqual(entry["done_status"], "interrupted")

    def test_mid_stream_cut_leaves_an_observable_state_no_silent_death(self):
        lines = self._sse("start", {"total_steps": 10}) + [ConnectionError("cut")]
        stream = self._FakeSse(lines)
        with mock.patch.object(self.mod, "_audit_open_sse", return_value=stream):
            out = self.mod.call_audit_launch({"project_id": "p1", "mode": "full"})
        self.assertTrue(out["launched"])
        entry = self._wait_state("p1", {"stream_error"})
        self.assertIn("cut", entry["error"])
        # The error state is set in the except block, the close happens in
        # the finally right after — poll briefly instead of racing it.
        import time as _time
        deadline = _time.time() + 3
        while not stream.closed and _time.time() < deadline:
            _time.sleep(0.01)
        self.assertTrue(stream.closed, "the finally must close the response")
        # And audit_status keeps the three layers separate.
        def fake_http(method, path, body=None):
            if path.endswith("/audit-status"):
                return {"success": True, "data": None}
            return {"success": True, "data": None}
        with mock.patch.object(self.mod, "_http", side_effect=fake_http):
            status = self.mod.call_audit_status({"project_id": "p1"})
        self.assertEqual(status["bridge_stream"]["state"], "stream_error")
        self.assertIsNone(status["live"])

    def test_no_start_event_is_a_launch_error_not_an_ambiguous_launched(self):
        # A stream that ends with no event at all: the launcher must raise
        # (Codex round: no hollow `launched`) — EOF seals the entry, so the
        # wait returns quickly via the reader's finally.
        stream = self._FakeSse([])
        with mock.patch.object(self.mod, "_audit_open_sse", return_value=stream):
            with self.assertRaises(RuntimeError) as ctx:
                self.mod.call_audit_launch({"project_id": "p1", "mode": "full"})
        self.assertTrue(
            "launch NOT confirmed" in str(ctx.exception)
            or "stream" in str(ctx.exception),
            f"got: {ctx.exception}",
        )


class AuditBridgeHardeningTests(unittest.TestCase):
    """0.8.13 dogfooding fixes: bridge_info staleness, briefing signal,
    install-template passthrough, resume_run_id validation placement."""

    def setUp(self):
        self.mod = _load_module()

    def test_bridge_info_fresh_bridge_is_not_stale(self):
        out = self.mod.call_bridge_info({})
        self.assertFalse(out["stale"], "a just-loaded bridge must not read stale")
        self.assertTrue(out["script_path"].endswith("disc-introspection-mcp.py"))

    def test_bridge_info_detects_newer_script_on_disk(self):
        self.mod._BRIDGE_SCRIPT_MTIME_AT_LOAD = 1.0  # loaded aeons ago
        out = self.mod.call_bridge_info({})
        self.assertTrue(out["stale"])
        self.assertIn("reconnect", out["hint"])

    def test_briefing_state_present_and_absent(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            self.assertFalse(self.mod._briefing_state({"path": tmp})["present"])
            os.makedirs(os.path.join(tmp, "docs"), exist_ok=True)
            with open(os.path.join(tmp, "docs", "briefing.md"), "w") as f:
                f.write("# Briefing")
            state = self.mod._briefing_state({"path": tmp})
            self.assertTrue(state["present"])
            self.assertEqual(state["path"], "docs/briefing.md")
        self.assertFalse(self.mod._briefing_state({})["present"])

    def test_audit_install_template_posts_and_wraps_status(self):
        fake = mock.MagicMock(return_value={"success": True, "data": "TemplateInstalled"})
        with mock.patch.object(self.mod, "_http", fake):
            out = self.mod.call_audit_install_template({"project_id": "p1"})
        fake.assert_called_once_with("POST", "/api/projects/p1/install-template")
        self.assertEqual(out["audit_status"], "TemplateInstalled")

    def test_audit_launch_rejects_bad_resume_run_id_before_any_state(self):
        # The raise must happen BEFORE a stream entry exists — a phantom
        # "launching" entry would block every future launch on the project.
        # resume_run_id must be a non-empty string (the backend validates the
        # row itself); ints, empties and blanks are refused here.
        for bad in (16, -1, 3.5, "", "   "):
            with self.assertRaises(RuntimeError):
                self.mod.call_audit_launch({
                    "project_id": "p-resume", "mode": "full", "resume_run_id": bad,
                })
        self.assertNotIn("p-resume", self.mod._AUDIT_STREAMS)

    def test_audit_launch_forwards_resume_run_id_to_the_backend(self):
        # A valid resume_run_id must survive validation and reach the launch
        # body verbatim (trimmed) — the backend is authoritative for kind +
        # checkpoint. The mocked opener proves the value got through.
        boom = ConnectionError("sentinel — no backend in tests")
        with mock.patch.object(self.mod, "_audit_open_sse", side_effect=boom) as opener:
            with self.assertRaises(RuntimeError) as ctx:
                self.mod.call_audit_launch({
                    "project_id": "p-resume-run", "mode": "full", "resume_run_id": " run-xyz ",
                })
        self.assertIn("could not open the audit stream", str(ctx.exception))
        self.assertNotIn("must be a non-empty string", str(ctx.exception))
        self.assertEqual(opener.call_args.args[1], {"agent": mock.ANY, "resume_run_id": "run-xyz"})
        self.mod._AUDIT_STREAMS.pop("p-resume-run", None)


class OnboardingTests(unittest.TestCase):
    """0.8.13 — first-contact onboarding: marker file + kronn_intro."""

    def setUp(self):
        self.mod = _load_module()
        import tempfile
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.mod._ONBOARD_MARKER = os.path.join(self.tmp.name, "onboarded.json")

    def test_first_contact_then_marked_done(self):
        self.assertFalse(self.mod._onboarding_done_for("ClaudeCode"))
        self.mod._CLIENT_INFO["name"] = "ClaudeCode"
        out = self.mod.call_kronn_intro({})
        self.assertIn("Kronn en 2 minutes", out["guide"])
        # Les 7 domaines que le tour DOIT couvrir.
        for domain in ("Discussions sauvegardées", "Mode join", "Quick Prompts",
                       "Workflows", "API configurée", "désagentification", "Audits"):
            self.assertIn(domain, out["guide"], f"tour incomplet: {domain} absent")
        self.assertEqual(out["onboarding_marked_done_for"], "ClaudeCode")
        self.assertTrue(self.mod._onboarding_done_for("ClaudeCode"))
        # Un autre client garde SON premier contact.
        self.assertFalse(self.mod._onboarding_done_for("Codex"))

    def test_marker_file_corruption_is_first_contact(self):
        os.makedirs(os.path.dirname(self.mod._ONBOARD_MARKER), exist_ok=True)
        with open(self.mod._ONBOARD_MARKER, "w") as f:
            f.write("not json")
        self.assertFalse(self.mod._onboarding_done_for("ClaudeCode"))


class StableIdentityKeyTests(unittest.TestCase):
    """0.8.13 presence root-fix — the binding key must be STABLE for a given
    CLI identity (so a reloaded bridge finds its own file) and DISTINCT across
    identities (so two CLIs never share a resume credential). `_identity_key_from`
    is pure, so the invariants are testable without spawning a process tree."""

    def setUp(self):
        self.mod = _load_module()

    def test_same_ancestor_yields_same_key(self):
        # An MCP reconnect keeps the same CLI ancestor → same key → same file.
        a = self.mod._identity_key_from((4321, "starttok"))
        b = self.mod._identity_key_from((4321, "starttok"))
        self.assertEqual(a, b)

    def test_distinct_ancestors_yield_distinct_keys(self):
        keys = {
            self.mod._identity_key_from((100, "t")),
            self.mod._identity_key_from((101, "t")),   # different pid
            self.mod._identity_key_from((100, "t2")),  # different start token
        }
        self.assertEqual(len(keys), 3, "pid AND start-token must both discriminate")

    def test_no_ancestor_disables_persistence_never_keys_on_cwd(self):
        # Fail-closed: no durable identity ⇒ no key. Keying on cwd would let
        # two CLI tabs in the same repo share one resume credential.
        self.assertIsNone(self.mod._identity_key_from(None))

    def test_key_is_short_hex(self):
        k = self.mod._identity_key_from((1, "t"))
        self.assertEqual(len(k), 16)
        self.assertTrue(all(c in "0123456789abcdef" for c in k))

    def test_ancestor_walk_returns_outermost_match_not_nearest(self):
        # A reconnect may respawn an intermediate runner whose cmdline also
        # carries the CLI name; the NEAREST match would then rotate on reload.
        # The walk must climb to the TOPMOST CLI-looking ancestor and return it.
        cmd = {100: "node mcp-runner codex", 200: "codex", 300: "/bin/zsh"}
        ppid = {100: 200, 200: 300, 300: 1}
        tok = {100: "near", 200: "outer", 300: "shell"}
        with mock.patch.object(self.mod.os, "getppid", return_value=100), \
             mock.patch.object(self.mod, "_cmdline_of", side_effect=lambda p: cmd.get(p)), \
             mock.patch.object(self.mod, "_ppid_of", side_effect=lambda p: ppid.get(p)), \
             mock.patch.object(self.mod, "_start_token_of", side_effect=lambda p: tok.get(p)):
            anc = self.mod._cli_ancestor_identity()
        self.assertEqual(anc, (200, "outer"), "outermost CLI ancestor must win over the nearer runner")

    def test_ancestor_walk_returns_none_when_no_cli_match(self):
        ppid = {100: 200, 200: 1}
        with mock.patch.object(self.mod.os, "getppid", return_value=100), \
             mock.patch.object(self.mod, "_cmdline_of", side_effect=lambda p: "/bin/bash"), \
             mock.patch.object(self.mod, "_ppid_of", side_effect=lambda p: ppid.get(p)), \
             mock.patch.object(self.mod, "_start_token_of", side_effect=lambda p: "t"):
            self.assertIsNone(self.mod._cli_ancestor_identity())


class ResumeBindingTests(unittest.TestCase):
    """0.8.13 presence root-fix — the persisted resume credential: 0600 file,
    atomic write, and `_attempt_resume` re-attaching after an MCP reload without
    a fresh kr-join token."""

    def setUp(self):
        import tempfile
        self.mod = _load_module()
        self.mod._CURRENT_DISC_ID = None  # unbound, as after a reload
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        # Isolate the binding to the tempdir (never touch the real ~/.config).
        self.mod._BINDING_DIR = self._tmp.name
        self.mod._BINDING_PATH_CACHE["computed"] = True
        self.mod._BINDING_PATH_CACHE["path"] = os.path.join(self._tmp.name, "binding.json")

    def _envelope(self, data):
        return {"success": True, "data": data}

    def test_binding_roundtrip_is_0600(self):
        import stat
        self.mod._write_binding("d-1", "kr-resume-aaa")
        got = self.mod._read_binding()
        self.assertEqual(got, {"disc_id": "d-1", "resume_token": "kr-resume-aaa"})
        mode = stat.S_IMODE(os.stat(self.mod._binding_path()).st_mode)
        self.assertEqual(mode, 0o600, "resume credential file must be owner-only")

    def test_write_binding_ignores_empty_inputs(self):
        self.mod._write_binding("", "tok")
        self.mod._write_binding("d-1", "")
        self.assertIsNone(self.mod._read_binding())

    def test_read_binding_none_when_missing_or_corrupt_or_incomplete(self):
        self.assertIsNone(self.mod._read_binding())  # no file
        with open(self.mod._binding_path(), "w") as f:
            f.write("not json")
        self.assertIsNone(self.mod._read_binding())
        with open(self.mod._binding_path(), "w") as f:
            f.write('{"disc_id": "d-1"}')  # no resume_token
        self.assertIsNone(self.mod._read_binding())

    def test_clear_binding_removes_file(self):
        self.mod._write_binding("d-1", "kr-resume-aaa")
        self.mod._clear_binding()
        self.assertIsNone(self.mod._read_binding())
        self.mod._clear_binding()  # idempotent, no raise on missing file

    def test_read_binding_refuses_a_symlink(self):
        # An attacker who plants a symlink at the binding path must not be able
        # to redirect the credential read (O_NOFOLLOW).
        target = os.path.join(self._tmp.name, "real.json")
        with open(target, "w") as f:
            f.write('{"disc_id": "d", "resume_token": "kr-resume-x"}')
        os.chmod(target, 0o600)
        os.symlink(target, self.mod._binding_path())
        self.assertIsNone(self.mod._read_binding())

    def test_read_binding_refuses_symlink_even_without_o_nofollow(self):
        # Cross-platform: where O_NOFOLLOW is absent it degrades to 0, so
        # os.open follows the link — the lstat + (dev,ino) match must still
        # refuse a symlinked credential path.
        target = os.path.join(self._tmp.name, "real2.json")
        with open(target, "w") as f:
            f.write('{"disc_id": "d", "resume_token": "kr-resume-x"}')
        os.chmod(target, 0o600)
        os.symlink(target, self.mod._binding_path())
        with mock.patch.object(self.mod.os, "O_NOFOLLOW", 0, create=True):
            self.assertIsNone(self.mod._read_binding())

    def test_read_binding_refuses_group_or_world_readable(self):
        # A credential file that isn't strictly owner-only is treated as
        # tampered — refuse it rather than trust it.
        path = self.mod._binding_path()
        with open(path, "w") as f:
            f.write('{"disc_id": "d", "resume_token": "kr-resume-x"}')
        os.chmod(path, 0o644)
        self.assertIsNone(self.mod._read_binding())

    def test_write_produces_owner_only_regular_file(self):
        import stat
        self.mod._write_binding("d-1", "kr-resume-z")
        st = os.lstat(self.mod._binding_path())
        self.assertTrue(stat.S_ISREG(st.st_mode), "must be a regular file, not a symlink")
        self.assertEqual(stat.S_IMODE(st.st_mode), 0o600)

    def test_no_identity_disables_all_persistence(self):
        # Fail-closed: when no durable identity resolved, `_binding_path` is
        # None → write/read/clear are no-ops and resume never calls the backend.
        self.mod._BINDING_PATH_CACHE["computed"] = True
        self.mod._BINDING_PATH_CACHE["path"] = None
        self.mod._write_binding("d-1", "kr-resume-x")  # no-op, no crash
        self.assertIsNone(self.mod._read_binding())
        http = mock.MagicMock()
        with mock.patch.object(self.mod, "_http", http):
            self.assertIsNone(self.mod._attempt_resume())
        http.assert_not_called()

    def test_attempt_resume_success_rebinds_and_rotates(self):
        self.mod._write_binding("d-42", "kr-resume-old")
        http = mock.MagicMock(return_value=self._envelope(
            {"disc_id": "d-42", "session_pk": 7, "resume_token": "kr-resume-new"}))
        with mock.patch.object(self.mod, "_http", http):
            resumed = self.mod._attempt_resume()
        self.assertEqual(resumed, "d-42")
        self.assertEqual(self.mod._CURRENT_DISC_ID, "d-42")
        # The rotated credential replaced the old one on disk.
        self.assertEqual(self.mod._read_binding()["resume_token"], "kr-resume-new")
        # The request went to peer-resume with the OLD credential.
        method, path = http.call_args.args[0], http.call_args.args[1]
        body = http.call_args.args[2]
        self.assertEqual((method, path), ("POST", "/api/discussions/peer-resume"))
        self.assertEqual(body["resume_token"], "kr-resume-old")

    def test_attempt_resume_failure_keeps_binding_and_returns_none(self):
        self.mod._write_binding("d-42", "kr-resume-old")
        with mock.patch.object(self.mod, "_http", side_effect=RuntimeError("backend down")):
            resumed = self.mod._attempt_resume()
        self.assertIsNone(resumed)
        self.assertIsNone(self.mod._CURRENT_DISC_ID)
        # Binding preserved — a transient outage must not cost the capability.
        self.assertEqual(self.mod._read_binding()["resume_token"], "kr-resume-old")

    def test_attempt_resume_without_binding_makes_no_call(self):
        http = mock.MagicMock()
        with mock.patch.object(self.mod, "_http", http):
            self.assertIsNone(self.mod._attempt_resume())
        http.assert_not_called()

    def test_reload_with_new_bridge_id_resumes_same_room(self):
        # Replaces the obsolete PR118 "adhoc id survives reload" test. A
        # reconnect legitimately gives a NEW bridge session id; the resume
        # credential is what re-attaches — _attempt_resume must post the
        # CURRENT (new) session_id and return the SAME room.
        self.mod._write_binding("d-room", "kr-resume-1")
        http = mock.MagicMock(return_value=self._envelope(
            {"disc_id": "d-room", "session_pk": 5, "resume_token": "kr-resume-2"}))
        with mock.patch.object(self.mod, "_session_id_for_caller", return_value="bridge-after-reload"), \
             mock.patch.object(self.mod, "_http", http):
            resumed = self.mod._attempt_resume()
        self.assertEqual(resumed, "d-room", "re-attaches to the same room across a reload")
        body = http.call_args.args[2]
        self.assertEqual(body["session_id"], "bridge-after-reload",
                         "resume posts the NEW bridge id so the backend rebinds the row")
        self.assertEqual(body["resume_token"], "kr-resume-1")

    def test_disc_id_lazily_resumes_after_reload(self):
        # The end-to-end reload path: unbound + no env, but a binding exists →
        # `_disc_id()` transparently re-attaches instead of raising.
        self.mod._write_binding("d-99", "kr-resume-x")
        http = mock.MagicMock(return_value=self._envelope(
            {"disc_id": "d-99", "session_pk": 3, "resume_token": "kr-resume-y"}))
        with mock.patch.dict(os.environ, {}, clear=True), \
             mock.patch.object(self.mod, "_http", http):
            self.assertEqual(self.mod._disc_id(), "d-99")

    def test_disc_id_raises_when_unbound_no_env_no_binding(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(RuntimeError) as ctx:
                self.mod._disc_id()
        self.assertIn("no disc bound", str(ctx.exception))

    def test_disc_join_persists_binding_and_hides_credential_from_model(self):
        http = mock.MagicMock(return_value=self._envelope(
            {"disc_id": "d-join", "session_pk": 1, "resume_token": "kr-resume-j",
             "peer_count": 1, "disc_title": "T", "recent_messages": [], "next_steps": ""}))
        with mock.patch.object(self.mod, "_http", http):
            returned = self.mod.call_disc_join({"token": "kr-join-abc"})
        self.assertEqual(self.mod._CURRENT_DISC_ID, "d-join")
        # Credential persisted 0600…
        self.assertEqual(self.mod._read_binding(),
                         {"disc_id": "d-join", "resume_token": "kr-resume-j"})
        # …but stripped from the value handed to the model.
        self.assertNotIn("resume_token", returned,
                         "the resume credential must never reach the model context")

    def test_disc_append_scopes_presence_to_this_session(self):
        # 0.8.13 — append must carry session_id so the backend heartbeat/clear
        # touches only THIS (resumed) row, not every same-agent_type session.
        self.mod._CURRENT_DISC_ID = "d-app"
        http = mock.MagicMock(return_value=self._envelope({"appended": 1}))
        with mock.patch.object(self.mod, "_http", http):
            self.mod.call_disc_append({"content": "hi peers"})
        method, path, body = http.call_args.args[0], http.call_args.args[1], http.call_args.args[2]
        self.assertEqual((method, path), ("POST", "/api/disc/append"))
        self.assertEqual(body["session_id"], self.mod._session_id_for_caller())


if __name__ == "__main__":
    unittest.main()
