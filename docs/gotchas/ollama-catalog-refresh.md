# Ollama catalogue refresh boundaries

`GET /api/model-catalogs` is a durable snapshot read. It includes the
`agent:ollama` target but must not contact Ollama. An explicit catalogue
refresh or `GET /api/ollama/models` performs catalogue discovery; the separate
health probe does not reconcile the catalogue.
[src: file: backend/src/api/model_catalog.rs:103-190]
[src: file: backend/src/api/ollama.rs:60-149]

Both active paths use the same bounded `/api/tags` decoder. A syntactically
valid empty `models` array is authoritative; transport failures, non-success
HTTP status, oversized bodies, invalid JSON/schema, and invalid model IDs are
recorded as refresh failures and do not reconcile a partial or empty result.
The request has a five-second deadline, including streamed body reads, and a
one-MiB response limit checked both before and during streaming. Exact duplicate
IDs collapse to one entry. Diagnostics never echo the upstream response body.
[src: file: backend/src/core/model_catalog/ollama_discovery.rs:13-116]

Successful discovery reconciles rows and its refresh log in one transaction;
the runtime tier cache is refreshed only after commit. Failure provenance and
the diagnostic log are also transactional. A failed database write must not
leave half a catalogue or mark prior live rows cached without its diagnostic.
Successful empty discovery marks missing models unavailable without deleting
their identity or operator settings. Reappearance restores the same identity.
Manual display names become aliases on promotion; existing aliases, tier,
cost and privacy overlays are preserved.
[src: file: backend/src/core/model_catalog/mod.rs:361-410]
[src: file: backend/src/db/model_catalog.rs:535-651]

The installed-inventory endpoint intentionally retains its compatibility
response of an empty list after discovery failure. That response is not a
successful empty catalogue: the durable refresh log records the normalized
failure while prior identities and availability remain intact (formerly live
rows become cached). If saving that failure fails, the endpoint returns an API
error instead of falsely reporting an empty inventory. Settings displays the
failure even when the last successful refresh is still inside its freshness
window.
[src: file: backend/src/api/ollama.rs:149-225]
[src: file: frontend/src/components/settings/__tests__/OllamaCard.catalog.test.tsx:160-174]

## Regression boundary

`backend/tests/ollama_model_catalog.rs` exercises the real router and SQLite
against local HTTP fixtures: snapshot without network, deduplicated inventory,
namespace isolation, overlays and database reopen, disappearance/reappearance,
invalid schemas/IDs, header and body timeouts, bounded bodies, normalized
diagnostics and transaction rollback on injected SQLite failures. It does not
run inference, pull a model, or modify the operator's database.
[src: file: backend/tests/ollama_model_catalog.rs:60-580]
