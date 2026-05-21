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
        # And the side-effect : clientInfo stashed for downstream tools.
        self.assertEqual(self.mod._CLIENT_INFO["name"], "claude-code")
        self.assertEqual(self.mod._CLIENT_INFO["version"], "1.2.3")

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


if __name__ == "__main__":
    unittest.main()
