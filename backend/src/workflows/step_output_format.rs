// 0.8.5 — canonical step-output envelope.
//
// **Why this module exists.** Pre-refactor every step type emitted its
// own variant of "structured output":
//   - Agent (Structured/TypedSchema) + Exec → `---STEP_OUTPUT---\n{…}\n---END_STEP_OUTPUT---`
//   - ApiCall                                → `{…}\n[SIGNAL: OK]` (bare JSON + signal)
//   - JsonData / Notify / Batch*             → `{…}` (bare JSON, no signal)
// The runner's `extract_step_envelope` tried two strategies (markers
// first, last bare JSON with `data`+`status` second) to absorb the
// variance, but the duplication was a real regression surface —
// witnessed by the EW-7247 AutoPilot dogfooding on 2026-05-17 where
// `{{steps.fetch_issue.data.body}}` failed because the consumer
// assumed a JsonData shape but the producer was an ApiCall.
//
// **The canonical Kronn step-output envelope** (this module's product):
//
// ```text
// <optional human-readable prefix line(s)>
// ---STEP_OUTPUT---
// { "data": <any JSON>, "status": "OK"|"ERROR"|"NO_RESULTS"|…, "summary": "<one line>" }
// ---END_STEP_OUTPUT---
// [SIGNAL: <primary>]
// [SIGNAL: <optional secondary>]
// ```
//
// Every envelope-producing step type now funnels through
// `format_step_output` so the markers + signals are emitted with
// byte-for-byte consistency. Consumers reach into the result via
// `TemplateContext::set_step_output` which still keeps the
// strategy-2 fallback for legacy records loaded from DB.
//
// **Exceptions** (no envelope, intentional):
// - Gate steps emit their rendered `gate_message` verbatim — the
//   step has no semantic data to pass downstream, it's a pause.
// - Agent steps with `output_format: FreeText` emit raw text — the
//   user opted out of structure and the save-time validator catches
//   any consumer that tries to read `.data` from it.

use serde_json::{json, Value};

/// Build a Kronn step-output envelope.
///
/// - `data` is the step's semantic payload (any JSON value).
/// - `status` is one of `"OK"`, `"ERROR"`, `"NO_RESULTS"`, or a custom
///   code if the step type needs a richer signal vocabulary.
/// - `summary` is a one-line human-readable description (used in run
///   logs and as the consumer-facing `{{steps.X.summary}}` value).
/// - `prefix` is optional human text rendered BEFORE the envelope —
///   useful for step types that want operators to see a friendly
///   one-liner above the JSON when expanding a run row.
/// - `signals` is the trailing `[SIGNAL: …]` line(s). Always at least
///   one (e.g. `"OK"`). Multiple signals stack on separate lines.
///
/// The returned string is what each step type writes to
/// `StepResult.output`. The runner's `set_step_output` then extracts
/// the envelope via the strategy-1 markers — guaranteed match.
pub fn format_step_output(
    data: Value,
    status: &str,
    summary: &str,
    prefix: Option<&str>,
    signals: &[&str],
) -> String {
    let envelope = json!({
        "data": data,
        "status": status,
        "summary": summary,
    });
    // Compact JSON is intentional: the envelope is consumed by
    // machines (`set_step_output` → JSON parse). Pretty printing
    // would inflate the run log size without helping operators
    // (who read the `prefix` line and the SIGNAL lines, not the JSON).
    let envelope_str = serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".to_string());

    let mut out = String::with_capacity(envelope_str.len() + 128);
    if let Some(p) = prefix {
        if !p.is_empty() {
            out.push_str(p);
            // Blank line between human prefix and the marker block so
            // operators can visually parse the structure.
            if !p.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
    }
    out.push_str("---STEP_OUTPUT---\n");
    out.push_str(&envelope_str);
    out.push_str("\n---END_STEP_OUTPUT---\n");
    for sig in signals {
        out.push_str("[SIGNAL: ");
        out.push_str(sig);
        out.push_str("]\n");
    }
    // Trim trailing newline — `evaluate_conditions` reads the last 5
    // lines, and a trailing empty line would push a meaningful SIGNAL
    // out of the window.
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Convenience constructor when there's no prefix and a single signal
/// (the common case for JsonData, Notify, batch fan-outs).
pub fn format_step_output_simple(data: Value, status: &str, summary: &str) -> String {
    format_step_output(data, status, summary, None, &[status])
}

/// Test-only helper: parse the canonical envelope produced by
/// `format_step_output` into a `serde_json::Value` shaped
/// `{ data, status, summary }`. Existing step-type tests used to
/// `serde_json::from_str(&output)` directly on the bare JSON line —
/// after the 0.8.5 homogenisation that fails because the output now
/// has markers + signals around it. Wrapping the extraction here
/// keeps test bodies short and the parsing logic in one place.
///
/// Panics if the output doesn't contain a canonical envelope —
/// regression tests want a loud failure, not silent `None`.
#[cfg(test)]
pub fn parse_envelope_for_test(output: &str) -> Value {
    let env = crate::workflows::template::extract_step_envelope(output)
        .expect("output must contain a parseable Kronn envelope (---STEP_OUTPUT--- markers or strategy-2 JSON)");
    let data: Value = serde_json::from_str(&env.data_json)
        .unwrap_or(Value::Null);
    serde_json::json!({
        "data": data,
        "status": env.status,
        "summary": env.summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_step_output_emits_canonical_markers() {
        let s = format_step_output(
            json!({ "key": "EW-1" }),
            "OK",
            "1 issue fetched",
            None,
            &["OK"],
        );
        assert!(s.contains("---STEP_OUTPUT---"));
        assert!(s.contains("---END_STEP_OUTPUT---"));
        assert!(s.contains("[SIGNAL: OK]"));
        // No trailing blank line — would shift signals out of the
        // last-5-lines window scanned by evaluate_conditions.
        assert!(!s.ends_with('\n'));
    }

    #[test]
    fn format_step_output_respects_prefix_with_blank_line_separator() {
        let s = format_step_output(
            json!({}),
            "OK",
            "done",
            Some("HTTP 200 — fetched 1 issue"),
            &["OK"],
        );
        // Prefix is on its own line(s), followed by a blank line, then the marker.
        assert!(s.starts_with("HTTP 200 — fetched 1 issue\n\n---STEP_OUTPUT---\n"));
    }

    #[test]
    fn format_step_output_supports_multiple_signals() {
        // Exec emits both a generic (OK/ERROR) signal AND a specific
        // exit_<code> signal so users can branch on either.
        let s = format_step_output(
            json!({ "exit_code": 2 }),
            "ERROR",
            "exec exit 2",
            None,
            &["ERROR", "exit_2"],
        );
        assert!(s.contains("[SIGNAL: ERROR]"));
        assert!(s.contains("[SIGNAL: exit_2]"));
        // Signals come at the END (last 5 lines per evaluate_conditions).
        let tail: Vec<&str> = s.lines().rev().take(5).collect();
        assert!(tail.iter().any(|l| l.contains("[SIGNAL: ERROR]")));
        assert!(tail.iter().any(|l| l.contains("[SIGNAL: exit_2]")));
    }

    #[test]
    fn format_step_output_envelope_extractor_can_parse_output() {
        // End-to-end pin: a Kronn envelope produced by this module
        // MUST round-trip through `extract_step_envelope` cleanly.
        // Regression here would break every step→step plumbing.
        let s = format_step_output(
            json!({ "key": "EW-7247", "fields": { "summary": "Hello" } }),
            "OK",
            "GET /issue → 1 item",
            Some("HTTP 200"),
            &["OK"],
        );
        let env = crate::workflows::template::extract_step_envelope(&s)
            .expect("canonical envelope must be parseable");
        assert_eq!(env.status, "OK");
        assert_eq!(env.summary, "GET /issue → 1 item");
        // data_json round-trips through serde — pin a substring rather
        // than the exact byte string so future helper tweaks (key
        // ordering, etc.) don't break the test.
        assert!(env.data_json.contains("EW-7247"));
        assert!(env.data_json.contains("Hello"));
    }

    #[test]
    fn format_step_output_simple_is_equivalent_to_full_with_default_signal() {
        let a = format_step_output_simple(json!({ "x": 1 }), "OK", "done");
        let b = format_step_output(json!({ "x": 1 }), "OK", "done", None, &["OK"]);
        assert_eq!(a, b);
    }

    #[test]
    fn format_step_output_handles_empty_data_object() {
        let s = format_step_output(json!({}), "OK", "noop", None, &["OK"]);
        let env = crate::workflows::template::extract_step_envelope(&s).unwrap();
        assert_eq!(env.status, "OK");
        // Empty object renders as `{}` in JSON — `.data` stringification.
        assert_eq!(env.data_json, "{}");
    }
}
