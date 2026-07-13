# TD-20260630-apicall-link-header-pagination

- **ID**: TD-20260630-apicall-link-header-pagination
- **Area**: Backend / Workflows (ApiCall executor)
- **Problem (fact)**: The workflow `ApiCall` auto-pagination (`walk_pages` in
  `backend/src/workflows/api_call_executor.rs`, driven by `PaginationSpec`)
  only handles **object-envelope** list APIs — responses shaped like
  `{ "issues": [...] }` / `{ "data": [...] }` / `{ "results": [...] }` with a
  **body** field carrying the next-page signal (`has_more` / `total` /
  `cursor`). It does **not** handle the **GitHub style**: a **bare top-level
  array** (`[ {...}, {...} ]`) paginated via the **`Link` response header**
  (`rel="next"` / `rel="last"`), with **no** in-body continuation field.
  `detect_items_key` requires an object → returns `None` on a bare array → the
  page accumulator never runs, and `PaginationSpec::Page` terminates after page
  1 because `has_more_path` resolves to nothing. GitHub also ignores
  `sort`/`direction` on `/pulls/{n}/reviews` (verified empirically 2026-06-30:
  `direction=desc` returns the OLDEST page), so "fetch the 100 most recent" is
  not expressible as a query param — the newest items are always on the *last*
  page, reachable only via the `Link` header.
- **Why we can't fix now (constraint)**: `send_with_retry` returns only the
  parsed JSON **body** (`Result<Value, String>`) — it discards the response
  headers, so there's no `Link` header to follow. A correct fix threads the
  headers (or at least the parsed `Link` rel-map) out of `send_with_retry`
  through `walk_pages`, adds a bare-array accumulation path (today everything
  keys off `detect_items_key` returning an object key), and adds a new
  `PaginationSpec` variant (or extends `Auto`) for header-driven walking — a
  signature ripple across the executor that's too broad to bundle with the
  unrelated PR-Review fixes it surfaced under.
- **Impact**: correctness (list endpoints silently truncate at one page) ·
  hidden snowball bugs. Concretely, on the PR-Review workflow
  (`0a792084-…`): `fetch_reviews` only saw the first 30 reviews (GitHub default
  page size) on long-lived PRs, so `skip_check`'s "did the bot already review
  this head?" dedup was blind to the recent same-head reviews (they sit on page
  2+) → the bot **re-reviewed the same PR on every cron run** (observed on PR
  1800 with 42 reviews). Same latent truncation affects `fetch_comments` and
  `fetch_files` (a PR with >100 changed files would drop files past 100 from
  `DIFF_FILES`, so the agent can't inline-comment them).
- **Mitigation in place (2026-06-30)**: added `per_page=100` to `fetch_reviews`
  + `fetch_comments` (`fetch_files` already had it) in workflow `0a792084-…`.
  Covers any PR with ≤100 reviews/comments/files — sufficient in practice (PR
  1800 = 42, and once the dedup works the count stops snowballing). The angle
  this TD covers is only the **>100** case, which `per_page=100` cannot reach
  (the newest items would be on a page we never request).
- **Where (pointers)**:
  - `backend/src/workflows/api_call_executor.rs` — `walk_pages` (≈1017-1163),
    `detect_items_key` (object-only; returns `None` on a bare array),
    `send_with_retry` (≈1242-1330; returns body only, drops headers).
  - `backend/src/models/workflows.rs` — `enum PaginationSpec` (≈700-749);
    variants are all body-signal based (`Offset.total_path`, `Cursor.next_path`,
    `Page.has_more_path`).
  - `backend/src/workflows/api_call_step.rs` — `pagination_max_pages`,
    `DEFAULT_MAX_PAGES` (the cap + `PAGINATION_TRUNCATED` signal reuse).
- **Suggested direction (non-binding)**:
  - Surface the response **`Link` header** (parsed rel-map, or raw) out of
    `send_with_retry` so `walk_pages` can read `rel="next"`.
  - Add a `PaginationSpec::LinkHeader { page_size_param, page_size, max_pages }`
    variant (or teach `Auto` to detect a `Link: rel=next` header) that walks
    `?page=1,2,…` following `rel="next"` until absent, and **accumulates a bare
    top-level array** (extend `detect_items_key` / the accumulator so the "items"
    can be the whole body when it's an array). Reuse the existing `max_pages`
    cap + `[SIGNAL: PAGINATION_TRUNCATED]`.
  - Fixes every GitHub list call at once (reviews, comments, files, commits) and
    any other `Link`-paginated bare-array API — so workflows stop silently
    truncating long lists.
- **Next step**: create ticket.
- **Status (plan 0.9, C2 — 2026-07-13)**: ✅ RESOLVED.
  - `send_with_retry` now returns the raw `Link` response header alongside the parsed body; `parse_link_next` extracts `rel="next"` (quoted and unquoted forms).
  - New `PaginationSpec::LinkHeader { page_size_param, page_size, max_pages }` variant: seeds page 1 (e.g. `per_page=100`), then follows the server's own `rel="next"` URL verbatim (its query params adopted wholesale) until absent — reusing the existing `max_pages` cap + `[SIGNAL: PAGINATION_TRUNCATED]`.
  - `walk_pages` accumulates **bare top-level arrays** (the merged result is then a plain array, so `$[*].id` extracts work); object-envelope bodies with a `Link` header (GitHub search) still merge via `detect_items_key`.
  - Locked by wiremock tests: 3-page bare-array merge in order, single page without `Link`, cap → truncated signal, and `parse_link_next` unit cases.
  - The PR-Review workflow's `per_page=100` mitigation can now be replaced by `{"type":"LinkHeader","page_size_param":"per_page","page_size":100}` on `fetch_reviews`/`fetch_comments`/`fetch_files` to cover the >100 case.

## Notes

- Surfaced 2026-06-30 while fixing the PR-Review workflow's "comments the same
  PR every cron run" bug. Root cause was the page-1-only truncation of
  `fetch_reviews` (GitHub default 30/page) defeating the `skip_check` dedup.
  Shipped the `per_page=100` mitigation; this TD captures the durable,
  general fix (header-driven pagination) deliberately left out of scope. Note
  this is **distinct** from the already-fixed `post_fallback` duplicate
  (which posted two reviews within a single run) — that was a workflow
  `on_result`/step-ordering bug, fixed separately.
