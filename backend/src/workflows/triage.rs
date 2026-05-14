//! Feasibility-Gated Implementation — triage step contract.
//!
//! Designed 0.8.3 against EW-7247 (Africanews→Euronews multi-brand
//! migration) — see `project_feasibility_gated_implementation` memory.
//!
//! The triage step forces an agent into "feasibility audit" mode: read
//! a ticket + repo, classify every sub-task into one of four buckets,
//! emit a JSON manifest that downstream steps consume via
//! `{{steps.triage.data}}` and a human reviews at a Gate.
//!
//! The four buckets:
//! - **`clear`** — straightforward implementation, no judgment call.
//! - **`decided`** — agent had multiple viable options and picked one.
//!   Each entry is a *traced freedom*: the choice is logged in the
//!   manifest AND insert as a `// KRONN-ASSUMED(<id>): <why>` marker
//!   in the generated code, so a human can challenge any decision at
//!   the Gate or later via grep.
//! - **`mocked`** — value or integration is faked because the real one
//!   is not yet available (env var, placeholder URL). Marker:
//!   `// KRONN-MOCKED(<id>): <strategy>`.
//! - **`blocked`** — agent cannot proceed; the feature stays
//!   un-implemented until an external dependency is resolved. Marker:
//!   `// KRONN-TODO(<id>): waiting on <needed_from>`.
//!
//! The shape is intentionally narrow: no `estimated_diff` field (would
//! be fictional — the agent has no compiler), no free-form "notes",
//! no nested categories. Every entry has a stable `id` so markers in
//! the code line up with manifest entries deterministically.

use crate::models::{
    agent_decisions::{
        CATEGORY_BLOCKED, CATEGORY_DECIDED, CATEGORY_MOCKED, STATUS_PENDING,
    },
    AgentDecision, OnInvalid, StepOutputFormat,
};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

/// JSON Schema (subset compatible with
/// `template::validate_envelope_against_schema`) for the triage
/// manifest. Returned as `serde_json::Value` so callers can embed it
/// into `StepOutputFormat::TypedSchema { schema, .. }` directly.
///
/// Validated fields: required arrays + per-entry `id` + `what`. Extra
/// fields per entry (where, options_considered, why, placeholder,
/// strategy, revisit_when, needed_from, workaround) are tolerated by
/// the validator (it only enforces `properties` it knows about).
pub fn triage_manifest_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "required": ["clear", "decided", "mocked", "blocked", "files_touched"],
        "properties": {
            "clear": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "what"],
                    "properties": {
                        "id": { "type": "string", "minLength": 1 },
                        "what": { "type": "string", "minLength": 1 },
                        "where": { "type": "string" }
                    }
                }
            },
            "decided": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "what", "chosen", "why"],
                    "properties": {
                        "id": { "type": "string", "minLength": 1 },
                        "what": { "type": "string", "minLength": 1 },
                        "options_considered": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "chosen": { "type": "string", "minLength": 1 },
                        "why": { "type": "string", "minLength": 1 }
                    }
                }
            },
            "mocked": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "what", "placeholder"],
                    "properties": {
                        "id": { "type": "string", "minLength": 1 },
                        "what": { "type": "string", "minLength": 1 },
                        "placeholder": { "type": "string", "minLength": 1 },
                        "strategy": { "type": "string" },
                        "revisit_when": { "type": "string" }
                    }
                }
            },
            "blocked": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "what", "why", "needed_from"],
                    "properties": {
                        "id": { "type": "string", "minLength": 1 },
                        "what": { "type": "string", "minLength": 1 },
                        "why": { "type": "string", "minLength": 1 },
                        "needed_from": { "type": "string", "minLength": 1 },
                        "workaround": { "type": "string" }
                    }
                }
            },
            "files_touched": {
                "type": "array",
                "items": { "type": "string" }
            }
        }
    })
}

/// System prompt addendum appended to the user prompt of a triage
/// step. Forces the "audit, don't code" mindset so the agent doesn't
/// jump straight to implementation. Inserted by
/// `build_triage_addendum()` in `template.rs` after the regular
/// `StepOutputFormat::TypedSchema` instructions.
pub const TRIAGE_PROMPT_ADDENDUM: &str = "
---
TRIAGE MODE — feasibility audit, not implementation.

You are NOT writing code in this step. You are producing a JSON manifest that classifies every sub-task of the ticket into one of four buckets.

RULES:
1. Do NOT write code. Do NOT propose patches. Do NOT include diff blocks.
2. Do NOT invent values for missing information (URLs, secrets, IDs, channel numbers, credentials, namespaces). If a value is missing, the corresponding entry goes into `blocked` or `mocked`, never `clear` or `decided`.
3. Classify EVERY sub-task you find. Use these buckets:
   - `clear`: straightforward implementation, only one reasonable way to do it. Examples: 'create file X with content Y', 'rename function A to B'.
   - `decided`: you have multiple viable options and you must pick one. The `chosen` field carries your choice, `why` explains it in one sentence, `options_considered` lists the alternatives you rejected. Examples: library choice, pattern choice (EventListener vs Compiler Pass), file organization.
   - `mocked`: the real value or integration is missing but a safe placeholder lets the rest ship. `placeholder` describes the fake (env var name, empty string, no-op stub), `strategy` describes how to replace it later, `revisit_when` is the trigger condition.
   - `blocked`: cannot proceed without external input. `why` describes what's missing, `needed_from` names the team/person/system that must respond, `workaround` (optional) is what to do until they respond.
4. Every entry has a stable `id` (kebab-case, descriptive, e.g. `brand-context-impl`, `adobe-dtm-urls-an`). The downstream implement step will insert `KRONN-(ASSUMED|MOCKED|TODO)(<id>): <why>` markers in the generated code using these ids — keep them stable.
5. `files_touched` is your best estimate of which paths the implementation will modify. Exhaustive is better than minimal.
6. Output ONLY the JSON envelope. No markdown explanation outside it.

CROSS-REPO EVIDENCE (when this project has linked repositories — see `## Linked repositories` block above):
- Linked repos are READ-ONLY reference codebases. NEVER modify them.
- If the ticket describes a MIGRATION (e.g. porting brand X from legacy_repo into current_repo), READ the legacy repo's `docs/AGENTS.md` FIRST, then concrete files (color tokens, templates, constants) that the migration must preserve. Lift real values — do NOT invent them.
- For EVERY `decided` or `mocked` item where evidence COULD exist in a linked repo, you MUST cite the source as a file:line reference in the `why` (decided) or `strategy` (mocked) field. Format: `evidence: <linked_repo_name>/<path>:<line>` — e.g. `evidence: front_africanews/assets/css/tokens.css:12 (--brand-primary: #f5a623)`.
- When a value lifted from a linked repo is concrete and unambiguous, the item moves from `mocked` to `decided` (or even `clear` if no alternative exists). A mocked item must explain why evidence does NOT exist in the linked repos.
- If the ticket is unrelated to any linked repo, ignore them. Do not invent ties.

DO NOT skip this triage step or rationalize 'I know what to do, let me just code it'. The triage manifest IS the work for this step. The implementation runs in the next step against your validated manifest.
";

/// Detect a step that is a "triage" step — by convention, the
/// description starts with `[TRIAGE]` OR the `output_format` is a
/// `TypedSchema` whose root has the exact `required` array of the
/// triage schema (`["clear","decided","mocked","blocked","files_touched"]`).
///
/// The description marker is the recommended path for hand-authored
/// workflows (no need to embed the schema in every step). The schema
/// match is the fallback for legacy workflows that already happen to
/// use the triage shape without the description marker.
pub fn is_triage_step(step_description: Option<&str>, output_format: &StepOutputFormat) -> bool {
    if let Some(desc) = step_description {
        if desc.trim_start().starts_with("[TRIAGE]") {
            return true;
        }
    }
    if let StepOutputFormat::TypedSchema { schema, .. } = output_format {
        if let Some(required) = schema.get("required").and_then(|v| v.as_array()) {
            let names: std::collections::HashSet<&str> = required
                .iter()
                .filter_map(|v| v.as_str())
                .collect();
            let expected: std::collections::HashSet<&str> =
                ["clear", "decided", "mocked", "blocked", "files_touched"]
                    .iter()
                    .copied()
                    .collect();
            return names == expected;
        }
    }
    false
}

/// Default `StepOutputFormat` for triage steps — wired with the
/// triage schema and `on_invalid: Fail` so an invalid manifest never
/// reaches the implement step.
pub fn triage_output_format() -> StepOutputFormat {
    StepOutputFormat::TypedSchema {
        schema: triage_manifest_schema(),
        on_invalid: OnInvalid::Fail,
    }
}

/// Parse a validated triage manifest JSON value into a vector of
/// `AgentDecision` rows ready for the DB. `clear` entries are
/// skipped — they're trivial by definition and would only add noise.
///
/// `run_id`, `workflow_id`, `step_name` come from the runner context.
/// `project_id` and `ticket_ref` are derived from the run's project
/// binding and trigger context; pass `None` when unavailable.
///
/// The function does NOT validate the manifest against the schema —
/// that's the runner's job upstream via
/// `validate_envelope_against_schema`. By the time the ingest runs,
/// the manifest is known-valid.
pub fn manifest_to_decisions(
    manifest: &serde_json::Value,
    run_id: &str,
    workflow_id: &str,
    step_name: &str,
    project_id: Option<&str>,
    ticket_ref: Option<&str>,
) -> Vec<AgentDecision> {
    let mut out = Vec::new();
    let now = Utc::now();
    let common_id = |dec_id: &str| -> String {
        // `id` is a fresh UUID per ingest call. The composite
        // (run_id, decision_id) is the dedup key, not `id`.
        let _ = dec_id;
        Uuid::new_v4().to_string()
    };

    // ── decided ──────────────────────────────────────────────
    if let Some(arr) = manifest.get("decided").and_then(|v| v.as_array()) {
        for entry in arr {
            let dec_id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if dec_id.is_empty() { continue; }
            let what = entry.get("what").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let chosen = entry.get("chosen").and_then(|v| v.as_str()).map(String::from);
            let why = entry.get("why").and_then(|v| v.as_str()).map(String::from);
            // `options_considered` is a JSON array. We store it as a
            // serialized string in `options_json` so the DB layer
            // doesn't need its own schema.
            let options_json = entry.get("options_considered")
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into()));
            out.push(AgentDecision {
                id: common_id(dec_id),
                run_id: run_id.into(),
                step_name: step_name.into(),
                workflow_id: workflow_id.into(),
                project_id: project_id.map(String::from),
                ticket_ref: ticket_ref.map(String::from),
                category: CATEGORY_DECIDED.into(),
                decision_id: dec_id.into(),
                what,
                chosen,
                options_json,
                why,
                placeholder: None,
                strategy: None,
                revisit_when: None,
                needed_from: None,
                workaround: None,
                gate_status: STATUS_PENDING.into(),
                override_value: None,
                code_locations: None,
                created_at: now,
                resolved_at: None,
            });
        }
    }

    // ── mocked ───────────────────────────────────────────────
    if let Some(arr) = manifest.get("mocked").and_then(|v| v.as_array()) {
        for entry in arr {
            let dec_id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if dec_id.is_empty() { continue; }
            let what = entry.get("what").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let placeholder = entry.get("placeholder").and_then(|v| v.as_str()).map(String::from);
            let strategy = entry.get("strategy").and_then(|v| v.as_str()).map(String::from);
            let revisit_when = entry.get("revisit_when").and_then(|v| v.as_str()).map(String::from);
            out.push(AgentDecision {
                id: common_id(dec_id),
                run_id: run_id.into(),
                step_name: step_name.into(),
                workflow_id: workflow_id.into(),
                project_id: project_id.map(String::from),
                ticket_ref: ticket_ref.map(String::from),
                category: CATEGORY_MOCKED.into(),
                decision_id: dec_id.into(),
                what,
                chosen: None,
                options_json: None,
                why: None,
                placeholder,
                strategy,
                revisit_when,
                needed_from: None,
                workaround: None,
                gate_status: STATUS_PENDING.into(),
                override_value: None,
                code_locations: None,
                created_at: now,
                resolved_at: None,
            });
        }
    }

    // ── blocked ──────────────────────────────────────────────
    if let Some(arr) = manifest.get("blocked").and_then(|v| v.as_array()) {
        for entry in arr {
            let dec_id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if dec_id.is_empty() { continue; }
            let what = entry.get("what").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let why = entry.get("why").and_then(|v| v.as_str()).map(String::from);
            let needed_from = entry.get("needed_from").and_then(|v| v.as_str()).map(String::from);
            let workaround = entry.get("workaround").and_then(|v| v.as_str()).map(String::from);
            out.push(AgentDecision {
                id: common_id(dec_id),
                run_id: run_id.into(),
                step_name: step_name.into(),
                workflow_id: workflow_id.into(),
                project_id: project_id.map(String::from),
                ticket_ref: ticket_ref.map(String::from),
                category: CATEGORY_BLOCKED.into(),
                decision_id: dec_id.into(),
                what,
                chosen: None,
                options_json: None,
                why,
                placeholder: None,
                strategy: None,
                revisit_when: None,
                needed_from,
                workaround,
                gate_status: STATUS_PENDING.into(),
                override_value: None,
                code_locations: None,
                created_at: now,
                resolved_at: None,
            });
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::template;

    #[test]
    fn triage_schema_validates_a_minimal_valid_manifest() {
        let manifest = json!({
            "clear": [{ "id": "scss-tokens", "what": "create _tokens.africanews.scss" }],
            "decided": [{
                "id": "brand-context-impl",
                "what": "hostname → brand resolution",
                "chosen": "EventListener on KernelRequest",
                "why": "Runtime resolution per-request",
                "options_considered": ["Compiler Pass", "Decorator on Request"]
            }],
            "mocked": [{
                "id": "adobe-dtm-an",
                "what": "Adobe DTM URLs for AN",
                "placeholder": "env var KRONN_ADOBE_DTM_AN_URL_PROD",
                "strategy": "fallback empty when env missing",
                "revisit_when": "Data team provides real URLs"
            }],
            "blocked": [{
                "id": "adobe-visitor-an",
                "what": "Adobe visitorNamespace for AN",
                "why": "not in ticket",
                "needed_from": "Data team"
            }],
            "files_touched": ["src/Service/BrandContext.php", "config/services.yaml"]
        });
        let schema = triage_manifest_schema();
        let json_str = serde_json::to_string(&manifest).unwrap();
        let result = template::validate_envelope_against_schema(&json_str, &schema);
        assert!(result.is_ok(), "valid manifest should pass, got: {result:?}");
    }

    #[test]
    fn triage_schema_rejects_missing_top_level_array() {
        let manifest = json!({
            "clear": [],
            "decided": [],
            "mocked": [],
            // Missing "blocked"
            "files_touched": []
        });
        let schema = triage_manifest_schema();
        let json_str = serde_json::to_string(&manifest).unwrap();
        let result = template::validate_envelope_against_schema(&json_str, &schema);
        assert!(result.is_err(), "missing 'blocked' must fail");
    }

    #[test]
    fn triage_schema_rejects_decided_without_chosen() {
        let manifest = json!({
            "clear": [],
            "decided": [{
                "id": "x",
                "what": "y",
                "why": "z"
                // Missing "chosen"
            }],
            "mocked": [],
            "blocked": [],
            "files_touched": []
        });
        let schema = triage_manifest_schema();
        let json_str = serde_json::to_string(&manifest).unwrap();
        let result = template::validate_envelope_against_schema(&json_str, &schema);
        assert!(result.is_err(), "decided entry without 'chosen' must fail");
    }

    #[test]
    fn triage_schema_rejects_blocked_without_needed_from() {
        let manifest = json!({
            "clear": [],
            "decided": [],
            "mocked": [],
            "blocked": [{
                "id": "x",
                "what": "y",
                "why": "z"
                // Missing "needed_from"
            }],
            "files_touched": []
        });
        let schema = triage_manifest_schema();
        let json_str = serde_json::to_string(&manifest).unwrap();
        let result = template::validate_envelope_against_schema(&json_str, &schema);
        assert!(result.is_err(), "blocked entry without 'needed_from' must fail");
    }

    #[test]
    fn triage_addendum_mandates_cross_repo_evidence() {
        // The triage prompt addendum is the contract that turns a bare
        // Agent step into a feasibility-gated triage step. The cross-repo
        // section is what makes v5+ runs use linked_repos as evidence
        // sources instead of inventing values. Lock the key directives so
        // an unintentional edit of the addendum (e.g. trimming the
        // section "to save tokens") fails CI loudly.
        let addendum = TRIAGE_PROMPT_ADDENDUM;
        assert!(
            addendum.contains("CROSS-REPO EVIDENCE"),
            "addendum must keep the CROSS-REPO EVIDENCE section header"
        );
        assert!(
            addendum.contains("Linked repositories"),
            "addendum must reference the `## Linked repositories` block injected by the runner"
        );
        assert!(
            addendum.contains("READ-ONLY"),
            "addendum must mark linked repos as read-only to prevent accidental writes there"
        );
        assert!(
            addendum.contains("evidence:"),
            "addendum must teach the `evidence: <repo>/<path>:<line>` format"
        );
        assert!(
            addendum.contains("mocked` to `decided"),
            "addendum must teach promotion from mocked → decided when evidence exists"
        );
    }

    #[test]
    fn is_triage_step_detects_description_marker() {
        let fmt = StepOutputFormat::FreeText;
        assert!(is_triage_step(Some("[TRIAGE] Feasibility audit"), &fmt));
        assert!(is_triage_step(Some("  [TRIAGE] leading space ok"), &fmt));
        assert!(!is_triage_step(Some("Regular step"), &fmt));
        assert!(!is_triage_step(None, &fmt));
    }

    #[test]
    fn is_triage_step_detects_schema_shape() {
        let fmt = triage_output_format();
        assert!(is_triage_step(None, &fmt));
        assert!(is_triage_step(Some("Some other description"), &fmt));
    }

    #[test]
    fn is_triage_step_false_on_unrelated_schema() {
        let fmt = StepOutputFormat::TypedSchema {
            schema: json!({
                "type": "object",
                "required": ["status", "score"]
            }),
            on_invalid: OnInvalid::Continue,
        };
        assert!(!is_triage_step(None, &fmt));
    }

    #[test]
    fn manifest_to_decisions_skips_clear_and_files_touched() {
        let manifest = json!({
            "clear": [{ "id": "c1", "what": "trivial" }],
            "decided": [{
                "id": "brand-impl",
                "what": "BrandContext",
                "chosen": "EventListener",
                "why": "runtime resolution",
                "options_considered": ["Compiler Pass", "Decorator"]
            }],
            "mocked": [{
                "id": "adobe-dtm",
                "what": "Adobe URLs",
                "placeholder": "env var",
                "strategy": "fallback empty",
                "revisit_when": "Data team"
            }],
            "blocked": [{
                "id": "visitor-ns",
                "what": "visitorNamespace",
                "why": "not in ticket",
                "needed_from": "Data team",
                "workaround": "env var conditional"
            }],
            "files_touched": ["src/X.php"]
        });

        let rows = manifest_to_decisions(
            &manifest, "run_a", "wf_test", "triage",
            Some("proj_test"), Some("EW-7247"),
        );
        // 3 rows (decided + mocked + blocked), clear skipped.
        assert_eq!(rows.len(), 3, "rows: {rows:?}");
        let cats: Vec<&str> = rows.iter().map(|r| r.category.as_str()).collect();
        assert!(cats.contains(&"decided"));
        assert!(cats.contains(&"mocked"));
        assert!(cats.contains(&"blocked"));
        // Each row carries the runtime context.
        for r in &rows {
            assert_eq!(r.run_id, "run_a");
            assert_eq!(r.workflow_id, "wf_test");
            assert_eq!(r.step_name, "triage");
            assert_eq!(r.project_id.as_deref(), Some("proj_test"));
            assert_eq!(r.ticket_ref.as_deref(), Some("EW-7247"));
            assert_eq!(r.gate_status, "pending");
        }
        // decided row has options_json populated as a JSON array string.
        let decided = rows.iter().find(|r| r.category == "decided").unwrap();
        assert_eq!(decided.chosen.as_deref(), Some("EventListener"));
        assert!(decided.options_json.as_deref().unwrap().contains("Compiler Pass"));
    }

    #[test]
    fn manifest_to_decisions_skips_entries_without_id() {
        // An entry missing `id` would break the (run_id, decision_id)
        // unique constraint downstream — we drop it.
        let manifest = json!({
            "clear": [], "decided": [], "mocked": [],
            "blocked": [
                { "id": "ok", "what": "real", "why": "x", "needed_from": "team" },
                { "what": "no id", "why": "x", "needed_from": "team" }
            ],
            "files_touched": []
        });
        let rows = manifest_to_decisions(
            &manifest, "run_a", "wf", "triage", None, None,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].decision_id, "ok");
    }

    #[test]
    fn triage_output_format_is_typed_schema_fail() {
        let fmt = triage_output_format();
        match fmt {
            StepOutputFormat::TypedSchema { on_invalid, .. } => {
                assert_eq!(on_invalid, OnInvalid::Fail);
            }
            other => panic!("expected TypedSchema, got {other:?}"),
        }
    }
}
