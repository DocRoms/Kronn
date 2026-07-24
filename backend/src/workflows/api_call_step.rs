//! `StepType::ApiCall` — pure extraction + pagination logic.
//!
//! Désagentification: this module holds the **side-effect-free** building
//! blocks (JSONPath evaluation, pagination shape detection, array concat
//! across pages). HTTP execution, auth resolution, rate limiting, retry,
//! and security guards live in `execute` (next module up, P0.3 + P0.4).
//! Keeping them split lets us hammer extraction edge cases with unit tests
//! that never touch the network.
//!
//! See `docs/operations/deagent-apicall.md` for the strategic context.

use crate::models::{ExtractSpec, PaginationSpec};
use serde_json::Value;
use serde_json_path::JsonPath;

/// Result of evaluating an `ExtractSpec` against a response body.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractionOutcome {
    /// The extracted value. `null` when the path matched nothing and no
    /// `fallback` was provided.
    pub value: Value,
    /// `true` when the match set was empty AND no fallback rescued us.
    /// Flips `status: NO_RESULTS` when the spec has `fail_on_empty`.
    pub is_empty: bool,
}

/// Walks the JSONPath in `spec.path` over `response` and returns the
/// extracted value following the semantics:
///   - 0 matches + fallback present → fallback, `is_empty = true`
///   - 0 matches + no fallback → `null`, `is_empty = true`
///   - 1 match → the single value (not wrapped in array — most natural
///     for UI consumption), `is_empty = false`
///   - N matches → JSON array of all matches (preserves order),
///     `is_empty = false`
///
/// Errors only on invalid JSONPath syntax. An empty array literal (`[]`)
/// returned by the API is a legitimate "no data" signal, not an error.
///
/// Design note: the RFC 9535 `JsonPath::query` always yields a
/// `NodeList`; we unwrap size-1 results because downstream steps and
/// templates ("{{steps.X.data}}") are vastly easier to read when the
/// user gets the scalar they asked for, not a one-element array.
pub fn apply_extract(
    spec: &ExtractSpec,
    response: &Value,
) -> Result<ExtractionOutcome, ExtractError> {
    let path = JsonPath::parse(&spec.path).map_err(|e| ExtractError::InvalidPath {
        path: spec.path.clone(),
        reason: e.to_string(),
    })?;
    let nodes = path.query(response);
    let values: Vec<&Value> = nodes.all();

    if values.is_empty() {
        let value = spec.fallback.clone().unwrap_or(Value::Null);
        return Ok(ExtractionOutcome {
            value,
            is_empty: true,
        });
    }

    // Size-1 result: unwrap. `$.total` → `42`, not `[42]`. The fallback
    // exists for "nothing matched" anyway, so ambiguity is low.
    if values.len() == 1 {
        return Ok(ExtractionOutcome {
            value: values[0].clone(),
            is_empty: false,
        });
    }

    let collected = Value::Array(values.into_iter().cloned().collect());
    Ok(ExtractionOutcome {
        value: collected,
        is_empty: false,
    })
}

/// Pagination shapes we can auto-detect from a response body.
/// This mirrors `PaginationSpec` minus `None` (no pagination requested).
#[derive(Debug, Clone, PartialEq)]
pub enum DetectedPagination {
    /// Jira v3, Confluence: `{ startAt, maxResults, total, issues: [...] }`
    Offset,
    /// Cloudflare GraphQL, GitHub GraphQL: `{ ... pageInfo: { endCursor, hasNextPage } }`
    /// or top-level `nextPageToken` (Jira v3 migration).
    Cursor,
    /// Stripe, Shopify: `{ has_more: bool, data: [...] }` with `page=N`.
    Page,
    /// No pagination markers found — treat as single page.
    None,
}

/// Inspects the response body for familiar pagination markers and returns
/// the detected shape. Heuristic — correct on common REST APIs, explicit
/// `PaginationSpec::{Offset, Cursor, Page}` is always available for edge
/// cases. Ordered so the most specific markers win first.
pub fn detect_pagination(response: &Value) -> DetectedPagination {
    let obj = match response.as_object() {
        Some(o) => o,
        None => return DetectedPagination::None,
    };

    // Jira v3 migration: both `issues[]` and `nextPageToken`. Cursor wins.
    if obj.contains_key("nextPageToken") || obj.contains_key("next_cursor") {
        return DetectedPagination::Cursor;
    }

    // GraphQL-style pageInfo — traverse one level into common wrapper keys
    // (`data`, `viewer`) to find it. Deeper walks are caller-specific.
    if has_graphql_page_info(obj) {
        return DetectedPagination::Cursor;
    }

    // Classic offset pagination: Jira, Confluence, Bitbucket.
    let has_offset = obj.contains_key("startAt") || obj.contains_key("offset");
    let has_total = obj.contains_key("total") || obj.contains_key("totalSize");
    if has_offset && has_total {
        return DetectedPagination::Offset;
    }

    // has_more / page pattern: Stripe, Shopify, HubSpot.
    if obj.contains_key("has_more") || obj.contains_key("hasMore") {
        return DetectedPagination::Page;
    }

    DetectedPagination::None
}

fn has_graphql_page_info(obj: &serde_json::Map<String, Value>) -> bool {
    // Shallow recursion: `data.<anything>.pageInfo`. A full walk would
    // false-positive on unrelated `pageInfo` keys deep inside entity data.
    let data = match obj.get("data").and_then(|v| v.as_object()) {
        Some(d) => d,
        None => return false,
    };
    for (_, inner) in data {
        if let Some(inner_obj) = inner.as_object() {
            if inner_obj.contains_key("pageInfo") {
                return true;
            }
            // One more level — `data.viewer.zones.pageInfo`
            for (_, deeper) in inner_obj {
                if let Some(deeper_obj) = deeper.as_object() {
                    if deeper_obj.contains_key("pageInfo") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Hard cap for pagination walks to prevent runaway loops. An API that
/// claims `hasMore: true` forever would otherwise lock a worker for
/// hours. Matches the strategic decision in `docs/operations/deagent-apicall.md`.
pub const DEFAULT_MAX_PAGES: u32 = 50;

/// Returns the `max_pages` bound on a pagination spec. `None` → default cap.
pub fn pagination_max_pages(spec: &PaginationSpec) -> u32 {
    let override_value = match spec {
        PaginationSpec::Auto { max_pages }
        | PaginationSpec::Offset { max_pages, .. }
        | PaginationSpec::Cursor { max_pages, .. }
        | PaginationSpec::Page { max_pages, .. }
        | PaginationSpec::LinkHeader { max_pages, .. } => *max_pages,
        PaginationSpec::None => return 1,
    };
    override_value.unwrap_or(DEFAULT_MAX_PAGES)
}

/// Errors surfaced by the extraction layer. Wrapped into the step's
/// stderr so the user sees actionable messages in the wizard / run detail.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("Invalid JSONPath '{path}': {reason}")]
    InvalidPath { path: String, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ─── apply_extract ──────────────────────────────────────────────

    #[test]
    fn apply_extract_returns_scalar_for_single_match() {
        // User expectation: `$.total` on `{ total: 42 }` returns `42`, not `[42]`.
        // Downstream template `{{steps.X.data}}` should render "42" cleanly.
        let spec = ExtractSpec {
            path: "$.total".into(),
            fallback: None,
            fail_on_empty: false,
        };
        let response = json!({ "total": 42, "issues": [] });
        let out = apply_extract(&spec, &response).unwrap();
        assert_eq!(out.value, json!(42));
        assert!(!out.is_empty);
    }

    #[test]
    fn apply_extract_returns_array_for_many_matches() {
        // Canonical fan-out to BatchQuickPrompt: "$.issues[*].key" on 3 issues.
        let spec = ExtractSpec {
            path: "$.issues[*].key".into(),
            fallback: None,
            fail_on_empty: false,
        };
        let response = json!({
            "issues": [
                { "key": "KR-1" },
                { "key": "KR-2" },
                { "key": "KR-3" },
            ]
        });
        let out = apply_extract(&spec, &response).unwrap();
        assert_eq!(out.value, json!(["KR-1", "KR-2", "KR-3"]));
        assert!(!out.is_empty);
    }

    #[test]
    fn apply_extract_empty_match_uses_fallback() {
        let spec = ExtractSpec {
            path: "$.issues[*].key".into(),
            fallback: Some(json!([])),
            fail_on_empty: false,
        };
        let response = json!({ "issues": [] });
        let out = apply_extract(&spec, &response).unwrap();
        assert_eq!(out.value, json!([]));
        assert!(out.is_empty);
    }

    #[test]
    fn apply_extract_empty_match_no_fallback_returns_null() {
        let spec = ExtractSpec {
            path: "$.foo.bar".into(),
            fallback: None,
            fail_on_empty: false,
        };
        let out = apply_extract(&spec, &json!({})).unwrap();
        assert_eq!(out.value, Value::Null);
        assert!(out.is_empty);
    }

    #[test]
    fn apply_extract_supports_filter_expression() {
        // RFC 9535 filter — used by CF experts for "zones with >1M requests".
        let spec = ExtractSpec {
            path: "$.zones[?(@.requests > 100)].name".into(),
            fallback: None,
            fail_on_empty: false,
        };
        let response = json!({
            "zones": [
                { "name": "a", "requests": 50 },
                { "name": "b", "requests": 200 },
                { "name": "c", "requests": 300 },
            ]
        });
        let out = apply_extract(&spec, &response).unwrap();
        assert_eq!(out.value, json!(["b", "c"]));
    }

    #[test]
    fn apply_extract_invalid_path_returns_error_not_panic() {
        // The wizard echoes this error verbatim; must not panic.
        let spec = ExtractSpec {
            path: "$[**$invalid".into(),
            fallback: None,
            fail_on_empty: false,
        };
        let err = apply_extract(&spec, &json!({})).unwrap_err();
        match err {
            ExtractError::InvalidPath { path, reason } => {
                assert_eq!(path, "$[**$invalid");
                assert!(!reason.is_empty());
            }
        }
    }

    #[test]
    fn apply_extract_unicode_values_roundtrip_safely() {
        // Kronn users run this in French/ES; JSON paths must not mangle text.
        let spec = ExtractSpec {
            path: "$.title".into(),
            fallback: None,
            fail_on_empty: false,
        };
        let response = json!({ "title": "Résumé — éco" });
        let out = apply_extract(&spec, &response).unwrap();
        assert_eq!(out.value, json!("Résumé — éco"));
    }

    // ─── detect_pagination ──────────────────────────────────────────

    #[test]
    fn detect_pagination_jira_offset() {
        // Shape from a real `GET /rest/api/3/search`.
        let response = json!({
            "startAt": 0,
            "maxResults": 50,
            "total": 137,
            "issues": [{ "key": "KR-1" }]
        });
        assert_eq!(detect_pagination(&response), DetectedPagination::Offset);
    }

    #[test]
    fn detect_pagination_jira_v3_migration_cursor_wins() {
        // Jira v3 migration: still has startAt for backward compat but
        // also exposes nextPageToken — the new canonical cursor. Cursor
        // must win or we paginate through the old offset path forever.
        let response = json!({
            "startAt": 0,
            "maxResults": 50,
            "total": 137,
            "nextPageToken": "abc123",
            "issues": []
        });
        assert_eq!(detect_pagination(&response), DetectedPagination::Cursor);
    }

    #[test]
    fn detect_pagination_graphql_cursor() {
        let response = json!({
            "data": {
                "viewer": {
                    "zones": {
                        "edges": [],
                        "pageInfo": { "endCursor": "xyz", "hasNextPage": true }
                    }
                }
            }
        });
        assert_eq!(detect_pagination(&response), DetectedPagination::Cursor);
    }

    #[test]
    fn detect_pagination_stripe_page() {
        let response = json!({
            "has_more": true,
            "data": [{ "id": "evt_1" }]
        });
        assert_eq!(detect_pagination(&response), DetectedPagination::Page);
    }

    #[test]
    fn detect_pagination_no_markers_is_none() {
        // Chartbeat returns a flat object — no pagination.
        let response = json!({ "pages": [], "visitors": 12 });
        assert_eq!(detect_pagination(&response), DetectedPagination::None);
    }

    #[test]
    fn detect_pagination_handles_array_at_root() {
        // Some APIs return a bare array. `as_object()` fails → None.
        // Must not panic. Caller decides whether to walk or not.
        let response = json!([1, 2, 3]);
        assert_eq!(detect_pagination(&response), DetectedPagination::None);
    }

    // ─── pagination_max_pages ───────────────────────────────────────

    #[test]
    fn pagination_max_pages_uses_default_when_unset() {
        let spec = PaginationSpec::Auto { max_pages: None };
        assert_eq!(pagination_max_pages(&spec), DEFAULT_MAX_PAGES);
    }

    #[test]
    fn pagination_max_pages_honors_override() {
        let spec = PaginationSpec::Cursor {
            cursor_param: "after".into(),
            next_path: "$.cursor".into(),
            max_pages: Some(3),
        };
        assert_eq!(pagination_max_pages(&spec), 3);
    }

    #[test]
    fn pagination_max_pages_none_spec_is_one() {
        // `PaginationSpec::None` means "don't paginate at all" = 1 request.
        assert_eq!(pagination_max_pages(&PaginationSpec::None), 1);
    }
}
