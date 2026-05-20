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


if __name__ == "__main__":
    unittest.main()
