# External API connections

Kronn's **Settings → Agents → External API mode** manages named connections to
OpenAI-compatible chat services. LiteLLM, NVIDIA and OpenRouter provide endpoint
presets; `Other` uses the same contract for an operator-supplied service. Every
connection owns a display name, a unique discussion alias, an endpoint and
optional Economy, Default and Reasoning models. [src: file: frontend/src/components/settings/AgentsSection.tsx:1791-1796#ExternalApiSection]
[src: file: backend/src/models/external_api_connection.rs:11-20#ExternalApiConnection]

The required HTTP surface is `GET /v1/models` plus
`POST /v1/chat/completions`. Kronn stores a base URL without a trailing `/v1`
and appends the route itself, so an endpoint copied with or without `/v1` reaches
the same final route. Only a final `/v1` path segment is removed: `/v1beta`,
`/api/v1/proxy` and a host such as `v1.example.com` are kept. [src: file: backend/src/api/external_api_connections.rs:245-264#normalize_endpoint]

## Add and test a connection

1. Open **Settings → Agents**, find **External API mode**, then choose **Add a
   connection**.
2. Pick `LiteLLM`, `NVIDIA`, `OpenRouter` or `Other`. The first three presets
   seed their known endpoint; `Other` requires the complete base URL reachable
   from the Kronn backend.
   [src: file: frontend/src/components/settings/ExternalApiSection.tsx:54-61#PRESET_ENDPOINTS]
3. Enter a display name and a unique mention alias. The stored alias is
   lower-cased without its leading `@`; `@name` is what a discussion uses.
   Whitespace and a second `@` are rejected. [src: file: backend/src/api/external_api_connections.rs:226-243#canonicalize_alias]
4. Enter the optional API key and select **Test connection**. Kronn first reads
   `/v1/models`. When a key and at least one model are available, it also sends a
   minimal non-streaming chat request with `max_tokens: 1` to distinguish a
   valid credential from a public model catalogue. This second request reaches
   the configured provider and may be accounted for by that provider.
   [src: file: backend/src/api/external_api_connections.rs:281-292#probe_models]
   [src: file: backend/src/api/external_api_connections.rs:630-636#chat_probe]
   NVIDIA and OpenRouter are exceptions: their public catalogues are not
   entitlement lists, so Kronn never probes an arbitrary first entry. OpenRouter
   first validates the key through its non-billable current-key endpoint. The
   first test then loads the catalogue; after tier models are selected, testing
   again invokes each unique selected model. On NVIDIA, a model-specific `404`
   keeps the catalogue available and is reported as an inaccessible model rather
   than a broken endpoint.
   [src: file: backend/src/api/external_api_connections.rs:549-555#probe_openrouter_credential]
   [src: file: backend/src/api/external_api_connections.rs:336-358#requested_models]
   [src: file: backend/src/api/external_api_connections.rs:530-534#http_error_hint]
5. After a successful test, map any returned models to Economy, Default and
   Reasoning, then save. The model fields remain read-only until the current
   endpoint and credential have passed that test. Changing the endpoint, key or
   preset invalidates the prior result; previous mappings stay visible but
   cannot be changed until the new connection state succeeds.
   [src: file: frontend/src/components/settings/ExternalApiSection.tsx:235-235#modelsUnlocked]
   [src: file: frontend/src/components/settings/ExternalApiSection.tsx:650-656#changeConnection]

The model tiers remain optional: leaving one blank stores no per-connection
override for that tier. The connection card shows the saved endpoint,
credential presence and all three mappings, and supports a fresh test without
returning the stored key to the browser. Editing shows a masked key state; only
an explicit click on the eye requests the stored value. **Replace** opens a
blank editable field, while saving without replacement preserves the current
credential. [src: file: backend/src/api/external_api_connections.rs:1811-1817#reveal_credential]
[src: file: backend/src/api/external_api_connections.rs:2090-2091#set_credential]
[src: file: frontend/src/components/settings/ExternalApiSection.tsx:997-1006#has_credential]
[src: file: frontend/src/components/settings/ExternalApiSection.tsx:321-335#SecretField]

## LiteLLM proxy

Kronn's installer uses:

```sh
uv tool install 'litellm[proxy]' --with 'fastapi<0.140'
```

The FastAPI bound is intentional for the LiteLLM version currently supported by
Kronn. Start the installed proxy with its operator-owned configuration, for
example `litellm --config <config.yaml>`, and make sure the aliases it should
expose appear in `/v1/models`. [src: file: backend/src/agents/mod.rs:138-147#install_cmd]
[src: file: backend/src/api/lite_llm.rs:186-187#probe]

LiteLLM documents the `model_list` configuration and proxy startup options in
its official getting-started guide. Do not copy provider credentials into this
repository; resolve them through the proxy's environment or secret mechanism.
[src: url: https://docs.litellm.ai/]

When Kronn runs in a container, enter an endpoint that the backend container can
reach; the browser's own `localhost` is not proof that the backend shares that
network namespace. [src: file: backend/src/api/lite_llm.rs:145-163#resolve_base_url]

## OpenRouter

The OpenRouter preset seeds `https://openrouter.ai/api/v1`, requires a complete
key with its `sk-or-v1-` prefix and exposes the provider's live model catalogue
to the searchable tier selectors. Because the catalogue itself is public,
Kronn validates the credential separately before treating the connection as
ready. Testing selected tiers can make billed chat requests; the credential
check and catalogue request do not select a paid model.
[src: file: frontend/src/components/settings/ExternalApiSection.tsx:54-61#PRESET_ENDPOINTS]
[src: file: backend/src/api/external_api_connections.rs:213-224#validate_credential_format]
[src: file: backend/src/api/external_api_connections.rs:544-555#probe_openrouter_credential]

OpenRouter remains a named external connection internally. Agent selectors show
its display name and current tier model, and Quick Prompt comparisons persist
the stable connection identifier. Re-running a comparison keeps the provider
and reasoning tier while resolving the model currently mapped to that tier, so
an unavailable or deliberately replaced model is not forced back into a retry.
[src: file: frontend/src/pages/WorkflowsPage.tsx:117-130#externalConnectionSwitchTarget]
[src: file: backend/src/models/quick.rs:436-440#connection_id]
[src: file: frontend/src/lib/compareTargets.ts:5-9#CompareTarget]
[src: file: backend/src/db/workflows.rs:955-1004#connection_tier_model]

## Credential handling

The SQLite connection row stores a credential slug, never the key value. API
responses include that opaque `credential_slug` metadata and a
`has_credential` boolean, but ordinary list/edit responses never return the key
itself. The authenticated reveal action returns it only after an explicit user
request. Create/update accepts the key as a write-only field: omitting it during
an edit preserves the current key, while an explicit blank value clears it.
[src: file: backend/src/models/external_api_connection.rs:5-16#credential_slug]
[src: file: backend/src/api/external_api_connections.rs:27-35#has_credential]
[src: file: backend/src/api/external_api_connections.rs:1811-1817#reveal_credential]
[src: file: backend/src/api/external_api_connections.rs:37-49#api_key]
[src: file: backend/src/api/external_api_connections.rs:1767-1771#set_credential]

Connection keys are `ApiKey` entries of the live configuration, persisted
encrypted with the instance key in the `stored_credentials` table since 0.14.3
(KT-1007): `config::save` stores them there, reads them back, and writes
`config.toml` without them. `#[ts(skip)]` only keeps the value out of the
generated TypeScript model. See [key-management.md](key-management.md).
[src: file: backend/src/core/credential_store.rs:346-349#sync_for_save]
[src: file: backend/src/models/setup.rs:484-489#ApiKey]

Deletion is not transactional across SQLite and the local configuration. Kronn
deletes the connection row first, then removes the matching token entry from a
copy of the configuration and saves it. If that save fails, the failure is
logged and the API returns an error saying the connection was removed but its
key was not; the live configuration keeps the key, which must then be deleted
from the API keys.
[src: file: backend/src/api/external_api_connections.rs:2105-2129#delete]

## Use in a discussion

Mention the saved alias, such as `@nvidia` or `@company-proxy`, in the message.
Kronn resolves named connection mentions before canonicalizing the requested
agent targets, and carries the exact connection identifier in that target.
[src: file: backend/src/db/external_api_connections.rs:201-235#target_for_connection]
[src: file: backend/src/api/discussions/messaging.rs:526-558#resolve_connection_mentions]

The new-discussion form merges configured OpenRouter/Other aliases with native
agent aliases. Each alias exposes its Economy, Default and Reasoning model in
the shared selector, and creation persists the connection-qualified initial
target rather than only the generic `Custom` wire type.
[src: file: frontend/src/components/NewDiscussionForm.tsx:219-230#launchTargets]
[src: file: frontend/src/lib/externalAgentIdentity.ts:63-79#externalAgentTargets]
[src: file: frontend/src/components/NewDiscussionForm.tsx:646-653#initialTargets]

Discussion headers, messages and comparison views recover the provider name
from that durable `MessageTarget.connection_id`; editable titles are not the
identity source once the detailed discussion is loaded. OpenAI-compatible
usage markers are also counted for `Custom`, so OpenRouter token totals follow
the same per-turn aggregation as LiteLLM and NVIDIA.
[src: file: frontend/src/lib/externalAgentIdentity.ts:81-107#discussionConnectionId]
[src: file: backend/src/agents/runner.rs:14350-14378#ollama_tokens]

Aliases are unique without regard to case. If an alias is already owned by
another connection, create or update fails instead of silently routing to the
wrong endpoint. [src: file: backend/src/db/external_api_connections.rs:178-199#ensure_mention_alias_available]

### A connection is an agent everywhere, not only in the router (0.13.0)

Routing a connection worked; being treated as an agent did not. The wire type a
connection carries is `AgentType::Custom`, and `Custom` was missing from the
frontend's `KNOWN_AGENTS` table — the static list several unrelated surfaces
each consult to answer "is this a real agent?". One absent entry produced seven
separate symptoms, none of which looked related:

| Surface | What the operator saw |
|---|---|
| Room membership | the connection was treated as a disabled agent and muted |
| Streaming | "connection lost" on a run that was working |
| Composer | `@openrouter` never appeared in the mention list |
| Worker catalogue | no entry, so no delegation |
| Message header | the generic label instead of the alias |
| Message colour | no identity colour of its own |
| Handoff | could not be named as a collaboration target |

They are one fix, not seven. The lesson is in
[`gotchas/known-agents-static-tables.md`](../gotchas/known-agents-static-tables.md):
when a surface answers a question about agents from a hand-written table, a new
agent kind is absent from it by default, and absence reads as *disabled*.

Identity colour is derived from the alias rather than assigned, so a new
connection has a distinct, stable colour on first use with nothing to configure.
[src: file: frontend/src/lib/externalAgentIdentity.ts:133-141#externalAgentColor]

## Upgrade from legacy settings

At startup, Kronn backfills the former single LiteLLM and NVIDIA settings into
stable named connections. The insertion is idempotent and preserves a
connection already created or edited through the new workflow. The canonical
NVIDIA legacy row also receives the hosted default endpoint when its former
configuration did not contain one. [src: file: backend/src/lib.rs:187-210#bootstrap_external_api_connections]
[src: file: backend/src/db/external_api_connections.rs:334-365#backfill_legacy_config]

The canonical LiteLLM and NVIDIA named cards are now the source of truth for
their endpoint and Economy, Default and Reasoning models. Saving either card
immediately refreshes the global agent selectors, and startup projects any
previously saved connection values into the compatibility configuration still
read by the runner. This prevents selector tooltips and executions from keeping
stale pre-migration model names. [src: file: backend/src/api/external_api_connections.rs:2086-2090#sync_runtime_config]
[src: file: backend/src/db/external_api_connections.rs:21-46#sync_runtime_config]
[src: file: frontend/src/components/settings/AgentsSection.tsx:1796-1801#onModelTiersChanged]

The former standalone LiteLLM settings card (`LiteLlmCard`) and its frontend
client were removed (KT-668): nothing mounted them, and the named connection
above is the only UI. The `/api/lite-llm/*` backend routes (`health`, `models`,
`test`, `model-failures`) are deliberately kept and now have no frontend caller.
`test` is the one route that writes the legacy `agents.lite_llm.base_url` and the
`litellm` key; `health` is also called in-process by the orchestrator. Dropping
them is a separate decision about that legacy configuration, not dead-code
cleanup. [src: file: backend/src/lib.rs:2173-2183]

After upgrading, verify both migrated cards, run their connection tests and
confirm the intended models for all three tiers before using them.

## Dispatch, codecs and the discussion's sticky connection (KT-545)

How a connection is actually dispatched to — codec selection, the
pre-dispatch capability gate, and how a discussion keeps addressing the same
connection across ordinary replies — is covered in
[`docs/operations/http-transport.md`](http-transport.md). This page stays
the configuration guide (creating/testing/tiering a connection); that one is
the runtime-resolution guide.

## Troubleshooting

- **No models:** confirm that `GET <base>/v1/models` returns a JSON `data` array
  from the network namespace where the Kronn backend runs.
  [src: file: backend/src/api/external_api_connections.rs:1286-1333#fetch_catalogue]
- **Authentication error:** re-enter the key. Kronn intentionally refuses to
  reuse a stored credential when the preset or canonical endpoint no longer
  matches the saved row. [src: file: backend/src/api/external_api_connections.rs:1539-1551#saved_connection]
- **HTTP error during test:** the model catalogue may be public while chat is
  restricted. A connection is considered usable only when both the catalogue
  and, when a key is supplied, the authenticated chat probe succeed. On a
  LiteLLM proxy a refused tier model is reported per tier instead of failing the
  connection.
  [src: file: backend/src/api/external_api_connections.rs:281-292#probe_models]
  [src: file: backend/src/api/external_api_connections.rs:389-395#probe_tier_models]
- **The test failed but the connection works:** fixed in 0.13.0. The probe used
  to send a model of its own choosing rather than one of the three the operator
  had configured, so a connection that only serves the models it was set up for
  failed a test it should have passed. The probe now chats with a configured
  model when one is set, and an HTTP failure is reported with a hint keyed to the status and the
  model actually tried — not a bare code.
  [src: file: backend/src/api/external_api_connections.rs:1592-1610#requested_models]
  [src: file: backend/src/api/external_api_connections.rs:527-539#http_error_hint]
- **An agent cannot find the connection to generate an image or a video:**
  fixed in 0.13.2. In a plain discussion, `agent_list`, the only place the
  connection id appears, refused any agent that was not a member of a room, and
  `media_generate` answered `unknown connection` to the model name the agent
  tried instead. The catalogue now answers in every discussion, and a
  generation accepts the connection's id, alias or display name; a refusal
  lists the connections that can generate the requested modality.
  [src: file: backend/src/api/orchestration.rs:13863-13869#task_worker_catalogue]
  [src: file: backend/scripts/disc-introspection-mcp.py:6114-6129#call_agent_list]
  [src: file: backend/src/db/external_api_connections.rs:144-162#resolve]
  [src: file: backend/src/api/media.rs:150-206#ConnectionReference]
  [src: file: backend/src/api/media.rs:404-433#usable_connections]
- **Wrong model after an endpoint edit:** test again before selecting tiers;
  model choices are tied to the exact tested endpoint and credential state.
  [src: file: frontend/src/components/settings/ExternalApiSection.tsx:650-656#changeConnection]
