# Execution variables

Kronn resolves automation variables from one of three declared sources:

- `user_input`: entered for the current launch;
- `project_env`: read from an encrypted MCP configuration linked to the
  selected project, through a reference such as `<env.API_TOKEN>`;
- `kronn_context`: read from an allowlisted scalar runtime context through a
  reference such as `<context.issue_key>`.

Templates, versions, clones and exports store the declaration and reference,
never the resolved project value. A launcher without the required project or
trusted runtime context fails its preflight instead of inventing a value.

## Launch contract

The launch form separates values the operator must enter from values provided
by the selected project. Project values are resolved into an encrypted,
ten-minute preview and stay masked until the operator explicitly uses the eye
control. A revealed value is fetched individually, audited, remasked after 30
seconds, on blur, on a second click or when the form closes. Preview errors are
value-free.

The preview is informational, not the execution snapshot. QP, QA, QE and
Workflow launch paths call the same Rust resolver again immediately before
dispatch. Consequently, a project configuration changed after the form was
opened is used by the new run. A provided value is read-only unless the
template author set `allow_manual_override`; enabling that override is an
explicit launch action and its effective provenance is recorded.

[src: file: backend/src/core/execution_variables.rs]
[src: file: backend/src/api/execution_variables.rs]
[src: file: frontend/src/components/workflows/ProvidedVariablesPreview.tsx]

## Snapshot and discussion trace

One run gets one immutable encrypted snapshot. Every Workflow step and any
technical resume of that run reuse it; a new run resolves a new snapshot. The
durable `execution_context` discussion card contains only snapshot identity,
resolution time, variable names, declared/effective source references and
override provenance. It never contains the values.

The inspector retrieves one historical value only after an explicit reveal.
Metadata access first verifies that the opaque run id still belongs to its
durable discussion/run and project scope. Reveals are audited without storing
the returned value.

[src: file: backend/src/db/execution_variable_snapshots.rs]
[src: file: frontend/src/components/MessageBubble.tsx]

## Retention

Encrypted values are retained for 30 days by default. The server setting is
available under Settings → Database and a discussion may inherit or override
it. `0` keeps values only for the run lifetime. Reading a value never extends
retention; an extension is a separate audited operation. Purging removes the
ciphertext irreversibly while preserving value-free provenance and the keyed,
non-reversible fingerprint used for diagnostics.

Launch previews are always capped at ten minutes and cannot be extended,
regardless of the history-retention setting.

## Regression coverage

The backend suite covers missing/ambiguous sources, overrides, immutable
same-run reuse, fresh values on new runs across a database restart, encryption,
authorization, reveal audit, expiry, zero-retention cleanup and irreversible
purge. API tests prove that metadata and preview payloads stay masked. Frontend
tests cover masked rendering, explicit reveal/remask, manual override and
value-free errors.

[src: file: backend/src/core/execution_variables.rs]
[src: file: backend/src/db/execution_variable_snapshots.rs]
[src: file: backend/tests/api_tests.rs]
[src: file: frontend/src/components/workflows/__tests__/ProvidedVariablesPreview.test.tsx]
