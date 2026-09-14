# Claude catalogue discovery and selector freshness (KT-645)

Claude catalogue discovery is independent of the ACP execution opt-in. Kronn
starts the installed CLI in stream-JSON mode and sends one SDK `initialize`
control request. It reads the response's `models`, the same catalogue exposed
by `supportedModels()` / `initializationResult().models`. It sends no user
prompt, model selection, permission approval or inference request.
[src: file: backend/src/core/model_catalog/mod.rs:292]
[src: file: backend/src/core/model_catalog/claude_discovery.rs:109]
[src: url: https://code.claude.com/docs/en/agent-sdk/typescript]

The process uses safe mode, an empty strict MCP configuration, no tools,
disabled hooks and no session persistence. It retains the CLI's authentication
environment: `--bare` is deliberately not used because it bypasses normal
OAuth/keychain discovery. An older CLI without the required flags fails
explicitly; Kronn does not retry with weaker isolation, install an SDK, require
an additional API key, or change the configured execution transport. The
initialization is bounded to 18 seconds inside the common 20-second discovery
budget, with bounded output and owned-child shutdown.
[src: file: backend/src/core/model_catalog/claude_discovery.rs:77]

The catalogue preserves `ModelInfo.value` exactly, including aliases and
context suffixes. `resolvedModel` is not substituted for it. For example,
`claude-fable-5-1[1m]` remains a Claude CLI selection in `agent:claude-code`,
never an API model identifier in `http:<connection>`. Reported effort levels
remain separate from model tiers; a missing effort list stays empty and no
default effort is invented. Shape, bounds and identity uniqueness are checked
on discovery output, not on free-form user model settings.
[src: file: backend/src/core/model_catalog/claude_discovery.rs:32]
[src: file: backend/src/db/model_catalog.rs:26]

Listing is not proof that an account can execute a model. The UI says that
access has not been tested. Cache identity remains the backend runtime target,
not a credential fingerprint or authorization grant. Recheck explicitly after
changing the CLI account; no account, token or credential is copied into the
catalogue. CLI aliases can change their resolution; the special `default`
choice is labelled accordingly and is never automatically assigned to a tier.
The same warning is shown in the Settings catalogue table, not just in pickers.
[src: file: frontend/src/lib/modelCatalogSelection.ts:76]
[src: file: frontend/src/components/settings/ModelCatalogSection.tsx:424]

Opening a selector serves its saved snapshot first, then refreshes stale CLI
targets without clearing visible options or user choices. Browser requests are
deduplicated per target and limited to two simultaneous refreshes. Backend
refreshes are serialized per database and target, with a second freshness
check under the lock: overlapping forced requests share the completed attempt,
but a later explicit recheck runs again. Success and failure attempts have a
ten-minute retry TTL. HTTP and Ollama are excluded from this automatic CLI
refresh; their existing explicit boundaries remain unchanged.
[src: file: frontend/src/hooks/useModelCatalogSnapshot.ts:6]
[src: file: backend/src/core/model_catalog/mod.rs:506]

Failures retain the last good catalogue and its timestamp with an explicit
error. A successful empty catalogue marks previous live entries unavailable;
their return restores availability. Manual/migrated aliases and existing
tiers are not removed merely because the CLI lists a different spelling.
Refresh does not choose a replacement or write configuration.
[src: file: backend/src/db/model_catalog.rs:600]

Retaining a catalogue and allowing a launch are distinct decisions. Claude
preflight tolerates a discovery `Timeout`/`ProviderError` only when the exact
selected model is still `Available`. It keeps cached provenance and the error
visible and leaves authentication to the execution boundary. `AuthRequired`,
`CliMissing`, absent identities and positively unavailable models remain blocked
when accompanied by a blocking refresh error. Other runtimes are unchanged.
[src: file: backend/src/core/model_catalog/mod.rs:653]

Regression coverage includes the initialization-only protocol, malformed and
oversized responses, stderr EOF, namespace separation, missing effort levels,
concurrent refreshes, failure cooldown, disappearance/reappearance, late UI
results and explicit Fable selection. Tests use isolated database/protocol
fixtures and mocked UI APIs; they do not assert an authorized billable call.
[src: file: backend/src/core/model_catalog/claude_discovery.rs:251]
[src: file: frontend/src/hooks/__tests__/useModelCatalogSnapshot.test.tsx:1]
[src: file: frontend/src/components/settings/__tests__/AgentsSection.catalog.test.tsx:1]

Integration tests link the production library: its unit-test discovery override
does not apply there. A migration fixture that calls launch preflight without
a refresh log can invoke the installed CLI, whose response may legitimately
restore a model marked unavailable by the fixture. Migration-only tests seed
an explicit fresh cached-discovery result and assert its attempt timestamp is
unchanged; they still verify the exact unavailable model and refusal reason.
[src: file: backend/tests/model_catalog_migration.rs:10]
