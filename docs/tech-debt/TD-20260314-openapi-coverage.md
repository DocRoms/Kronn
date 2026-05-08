# TD-20260314-openapi-coverage

- **ID**: TD-20260314-openapi-coverage (renamed from `TD-20260314-no-api-docs` once the scaffold shipped 2026-05-10)
- **Area**: Docs / Backend
- **Severity**: Low (scaffold serves a valid spec; richer docs unblock external integrators incrementally)
- **Status**: 🟡 Scaffold shipped — incremental enrichment ongoing

## What's done

- `utoipa = "5"` + `utoipa-swagger-ui = "9"` deps in `backend/Cargo.toml`.
- `backend/src/api/openapi.rs` defines the `ApiDoc` struct with the
  metadata, security note, and tag list.
- `GET /api/openapi.json` serves the spec (mounted by SwaggerUi's
  `.url()` helper).
- `GET /api/docs` serves the interactive Swagger UI.
- Reference endpoint `/api/version/check` documented end-to-end —
  contributors can copy its `#[utoipa::path(...)]` macro shape onto
  their own handlers.
- 3 unit tests: spec carries running version, contains the version
  endpoint (regression sentinel), serialises to valid JSON.

## What's missing

~170 routes are routed but not yet annotated. The spec lists them
implicitly via tags but doesn't document request/response shapes per
route. External integrators can hit endpoints they discover by
reading the source, but a curl-from-the-Swagger-UI flow for arbitrary
endpoints isn't there yet.

## Why we don't blanket-add `#[utoipa::path]` everywhere

It'd require ToSchema derives on every request/response struct, and
~170 path macros. That's a 1-2 day grind for marginal value — most
endpoints are internal-only (UI or CLI calling), and external
integrators are rare today.

## Suggested direction

Per-PR enrichment:

1. When a contributor adds or changes an endpoint, they also add the
   `#[utoipa::path(...)]` macro and the `paths(...)` entry in
   `ApiDoc`. Existing endpoints get covered as they get touched.
2. Track the coverage ratio in a single `docs/api-coverage.md`
   (count of documented vs total routes). Aim for ~30 % within 6
   months as the natural drift.
3. For external-facing endpoints (anything a customer / integrator
   would hit, e.g. webhook receivers when we add them), require the
   macro at PR time.

No DoD — this is a long-tail enrichment, not a sprint goal.

## Where (pointers)

- `backend/src/api/openapi.rs` — the scaffold
- `backend/src/api/version.rs:check` — example of an endpoint with a
  full `#[utoipa::path(...)]` macro
- `backend/src/lib.rs:380-385` — Swagger UI mount

## Next step

None — this TD just tracks the residual so it doesn't get forgotten.
