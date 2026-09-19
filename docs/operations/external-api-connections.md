# External API connections

Kronn's **Settings → Agents → External API mode** manages named connections to
OpenAI-compatible chat services. LiteLLM, NVIDIA and OpenRouter provide endpoint
presets; `Other` uses the same contract for an operator-supplied service. Every
connection owns a display name, a unique discussion alias, an endpoint and
optional Economy, Default and Reasoning models. [src: file: frontend/src/components/settings/AgentsSection.tsx:1536-1538]
[src: file: backend/src/models/external_api_connection.rs:5-30]

The required HTTP surface is `GET /v1/models` plus
`POST /v1/chat/completions`. Kronn stores a base URL without a trailing `/v1`
and appends the route itself, so an endpoint copied with or without `/v1` reaches
the same final route. [src: file: backend/src/api/external_api_connections.rs:101-140]

## Add and test a connection

1. Open **Settings → Agents**, find **External API mode**, then choose **Add a
   connection**.
2. Pick `LiteLLM`, `NVIDIA`, `OpenRouter` or `Other`. The first three presets
   seed their known endpoint; `Other` requires the complete base URL reachable
   from the Kronn backend.
   [src: file: frontend/src/components/settings/ExternalApiSection.tsx:49-54]
3. Enter a display name and a unique mention alias. The stored alias is
   lower-cased without its leading `@`; `@name` is what a discussion uses.
   Whitespace and a second `@` are rejected. [src: file: backend/src/api/external_api_connections.rs:82-99]
4. Enter the optional API key and select **Test connection**. Kronn first reads
   `/v1/models`. When a key and at least one model are available, it also sends a
   minimal non-streaming chat request with `max_tokens: 1` to distinguish a
   valid credential from a public model catalogue. This second request reaches
   the configured provider and may be accounted for by that provider.
   [src: file: backend/src/api/external_api_connections.rs:130-171]
   NVIDIA and OpenRouter are exceptions: their public catalogues are not
   entitlement lists, so Kronn never probes an arbitrary first entry. OpenRouter
   first validates the key through its non-billable current-key endpoint. The
   first test then loads the catalogue; after tier models are selected, testing
   again invokes each unique selected model. A model-specific `404` keeps the
   catalogue available and is reported as an inaccessible model rather than a
   broken endpoint.
   [src: file: backend/src/api/external_api_connections.rs:147-280]
5. After a successful test, map any returned models to Economy, Default and
   Reasoning, then save. The model fields remain read-only until the current
   endpoint and credential have passed that test. Changing the endpoint, key or
   preset invalidates the prior result; previous mappings stay visible but
   cannot be changed until the new connection state succeeds.
   [src: file: frontend/src/components/settings/ExternalApiSection.tsx:411-457]

The model tiers remain optional: leaving one blank stores no per-connection
override for that tier. The connection card shows the saved endpoint,
credential presence and all three mappings, and supports a fresh test without
returning the stored key to the browser. Editing shows a masked key state; only
an explicit click on the eye requests the stored value. **Replace** opens a
blank editable field, while saving without replacement preserves the current
credential. [src: file: backend/src/api/external_api_connections.rs:411-455]
[src: file: frontend/src/components/settings/ExternalApiSection.tsx:246-268]
[src: file: frontend/src/components/settings/ExternalApiSection.tsx:579-682]

## LiteLLM proxy

Kronn's installer uses:

```sh
uv tool install 'litellm[proxy]' --with 'fastapi<0.140'
```

The FastAPI bound is intentional for the LiteLLM version currently supported by
Kronn. Start the installed proxy with its operator-owned configuration, for
example `litellm --config <config.yaml>`, and make sure the aliases it should
expose appear in `/v1/models`. [src: file: backend/src/agents/mod.rs:92-105]
[src: file: backend/src/api/lite_llm.rs:142-153]

LiteLLM documents the `model_list` configuration and proxy startup options in
its official getting-started guide. Do not copy provider credentials into this
repository; resolve them through the proxy's environment or secret mechanism.
[src: url: https://docs.litellm.ai/]

When Kronn runs in a container, enter an endpoint that the backend container can
reach; the browser's own `localhost` is not proof that the backend shares that
network namespace. [src: file: backend/src/api/lite_llm.rs:41-58]

## OpenRouter

The OpenRouter preset seeds `https://openrouter.ai/api/v1`, requires a complete
key with its `sk-or-v1-` prefix and exposes the provider's live model catalogue
to the searchable tier selectors. Because the catalogue itself is public,
Kronn validates the credential separately before treating the connection as
ready. Testing selected tiers can make billed chat requests; the credential
check and catalogue request do not select a paid model.
[src: file: frontend/src/components/settings/ExternalApiSection.tsx:49-54]
[src: file: backend/src/api/external_api_connections.rs:89-99]
[src: file: backend/src/api/external_api_connections.rs:152-284]

OpenRouter remains a named external connection internally. Agent selectors show
its display name and current tier model, and Quick Prompt comparisons persist
the stable connection identifier. Re-running a comparison keeps the provider
and reasoning tier while resolving the model currently mapped to that tier, so
an unavailable or deliberately replaced model is not forced back into a retry.
[src: file: backend/src/db/external_api_connections.rs:133-155]
[src: file: backend/src/models/quick.rs:48-105]

## Credential handling

The SQLite connection row stores a credential slug, never the key value. API
responses include that opaque `credential_slug` metadata and a
`has_credential` boolean, but ordinary list/edit responses never return the key
itself. The authenticated reveal action returns it only after an explicit user
request. Create/update accepts the key as a write-only field: omitting it during
an edit preserves the current key, while an explicit blank value clears it.
[src: file: backend/src/models/external_api_connection.rs:5-20]
[src: file: backend/src/api/external_api_connections.rs:26-66]
[src: file: backend/src/api/external_api_connections.rs:425-454]
[src: file: backend/src/api/external_api_connections.rs:524-603]

The current implementation stores connection keys as `ApiKey.value` entries in
Kronn's local serialized configuration. `#[ts(skip)]` keeps that value out of
the generated TypeScript model; it is not an encryption marker. On Unix,
temporary configuration files are written with mode `0600`, and Kronn applies
owner-only permissions to its configuration directory (`0700`) and file
(`0600`). [src: file: backend/src/models/setup.rs:352-360]
[src: file: backend/src/core/config.rs:203-208]
[src: file: backend/src/core/config.rs:338-345]

Deletion is not transactional across SQLite and the local configuration. Kronn
deletes the connection row first, then removes the matching token entry and
attempts to save the configuration. A save failure is logged while the API
still returns success, so a credential entry can remain in the configuration
after such a failure. A later successful configuration save persists the
already-cleaned in-memory state; after a restart, manual cleanup of the stale
entry may instead be necessary.
[src: file: backend/src/api/external_api_connections.rs:573-613]

## Use in a discussion

Mention the saved alias, such as `@nvidia` or `@company-proxy`, in the message.
Kronn resolves named connection mentions before canonicalizing the requested
agent targets, and carries the exact connection identifier in that target.
[src: file: backend/src/db/external_api_connections.rs:75-133]
[src: file: backend/src/api/discussions/messaging.rs:424-455]

The new-discussion form merges configured OpenRouter/Other aliases with native
agent aliases. Each alias exposes its Economy, Default and Reasoning model in
the shared selector, and creation persists the connection-qualified initial
target rather than only the generic `Custom` wire type.
[src: file: frontend/src/components/NewDiscussionForm.tsx:186-212]
[src: file: frontend/src/components/NewDiscussionForm.tsx:489-543]

Discussion headers, messages and comparison views recover the provider name
from that durable `MessageTarget.connection_id`; editable titles are not the
identity source once the detailed discussion is loaded. OpenAI-compatible
usage markers are also counted for `Custom`, so OpenRouter token totals follow
the same per-turn aggregation as LiteLLM and NVIDIA.
[src: file: frontend/src/lib/externalAgentIdentity.ts:46-81]
[src: file: backend/src/agents/runner.rs:8717-8744]

Aliases are unique without regard to case. If an alias is already owned by
another connection, create or update fails instead of silently routing to the
wrong endpoint. [src: file: backend/src/db/external_api_connections.rs:79-100]

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
[src: file: frontend/src/lib/externalAgentIdentity.ts]

## Upgrade from legacy settings

At startup, Kronn backfills the former single LiteLLM and NVIDIA settings into
stable named connections. The insertion is idempotent and preserves a
connection already created or edited through the new workflow. The canonical
NVIDIA legacy row also receives the hosted default endpoint when its former
configuration did not contain one. [src: file: backend/src/lib.rs:31-47]
[src: file: backend/src/db/external_api_connections.rs:223-285]

The canonical LiteLLM and NVIDIA named cards are now the source of truth for
their endpoint and Economy, Default and Reasoning models. Saving either card
immediately refreshes the global agent selectors, and startup projects any
previously saved connection values into the compatibility configuration still
read by the runner. This prevents selector tooltips and executions from keeping
stale pre-migration model names. [src: file: backend/src/api/external_api_connections.rs:595-695]
[src: file: backend/src/db/external_api_connections.rs:18-59]
[src: file: frontend/src/components/settings/AgentsSection.tsx:1535-1544]

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
  [src: file: backend/src/api/external_api_connections.rs:214-290]
- **Authentication error:** re-enter the key. Kronn intentionally refuses to
  reuse a stored credential when the connection identifier, preset or canonical
  endpoint no longer matches the saved row. [src: file: backend/src/api/external_api_connections.rs:320-364]
- **HTTP error during test:** the model catalogue may be public while chat is
  restricted. A connection is considered usable only when both the catalogue
  and, when a key is supplied, the authenticated chat probe succeed.
  [src: file: backend/src/api/external_api_connections.rs:130-160]
- **The test failed but the connection works:** fixed in 0.13.0. The probe used
  to send a model of its own choosing rather than one of the three the operator
  had configured, so a connection that only serves the models it was set up for
  failed a test it should have passed. The probe now chats with a configured
  model, and an HTTP failure is reported with a hint keyed to the status and the
  model actually tried — not a bare code.
  [src: file: backend/src/api/external_api_connections.rs]
- **An agent cannot find the connection to generate an image or a video:**
  fixed in 0.13.2. In a plain discussion, `agent_list`, the only place the
  connection id appears, refused any agent that was not a member of a room, and
  `media_generate` answered `unknown connection` to the model name the agent
  tried instead. The catalogue now answers in every discussion, and a
  generation accepts the connection's id, alias or display name; a refusal
  lists the connections that can generate the requested modality.
  [src: file: backend/src/api/media.rs]
- **Wrong model after an endpoint edit:** test again before selecting tiers;
  model choices are tied to the exact tested endpoint and credential state.
  [src: file: frontend/src/components/settings/ExternalApiSection.tsx:362-400]
