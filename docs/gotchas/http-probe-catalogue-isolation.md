# HTTP connection tests and durable catalogue identity

The settings test endpoint serves two different purposes. A draft tests the
submitted endpoint and key without owning a durable runtime target. An unchanged
saved connection can refresh its own `http:<connection-id>` catalogue. Passing
`connection_id` alone does not authorize a draft to replace that catalogue.
[src: file: backend/src/api/external_api_connections.rs:689-728]
[src: file: backend/src/db/model_catalog.rs:48]

An explicit `api_key`, including an empty value, is a draft credential. The probe
does not reconcile a saved catalogue in that case. Without an explicit key, the
stored endpoint and provider must match before the stored credential is reused.
The saved connection is captured before discovery; its row and active credential
are checked again before persistence. A changed or deleted connection, or a
rotated credential, makes the result ineligible for durable reconciliation.
The response still describes the submitted test, not a new saved configuration.
[src: file: backend/src/api/external_api_connections.rs:689-827]

Credential comparison is held under the configuration read lock, while the row
comparison, catalogue reconciliation and refresh log use one SQLite transaction.
A persistence error is surfaced instead of reporting a partially updated or
silently cached catalogue. This is a probe commit boundary, not a claim that all
connection CRUD and the separate configuration file form one transaction.
[src: file: backend/src/api/external_api_connections.rs:749-827]

Successful empty discovery marks previously live models as disappeared without
deleting them. A failed saved probe preserves availability and last-seen values,
but marks live provenance cached and records its normalized failure. Operator
aliases, tier assignments, cost and privacy overlays survive reconciliation.
[src: file: backend/src/db/model_catalog.rs:537-652]
[src: file: backend/tests/http_probe_catalog.rs:253-341]

Text-tier selectors use the catalogue of the current connection test. Missing
saved IDs remain visible and are disabled as choices, with an unavailable
message. Untested, failed or invalidated tests show an unverified message instead.
A returning model becomes selectable again. Saving keeps the exact selected ID;
no replacement is inferred. The shared searchable picker remains responsible
for keyboard selection. Media capability-unknown behavior and the explicit
OpenRouter preset are unchanged.
[src: file: frontend/src/components/settings/ExternalApiSection.tsx:342-404]
[src: file: frontend/src/components/settings/__tests__/ExternalApiSection.test.tsx:111-173]

Regression coverage uses the real Router and isolated SQLite with local mock
providers. Held responses are released by notifications, not sleeps. The tests
cover draft success/failure, row/key changes during a probe, log-write rollback,
namespace preservation, normalized saved failure, and empty/reappearing models.
No user configuration, real provider, migration or repair is exercised.
[src: file: backend/tests/http_probe_catalog.rs:1-461]
