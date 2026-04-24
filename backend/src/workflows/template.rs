//! Liquid-compatible template engine for workflow prompts.
//!
//! Supports `{{variable}}` syntax with nested access via dots.
//! Built-in variables: issue.*, steps.<name>.output, previous_step.output

use std::collections::HashMap;
use anyhow::Result;

/// Template context holding all available variables.
#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    values: HashMap<String, String>,
}

impl TemplateContext {
    pub fn new() -> Self {
        Self { values: HashMap::new() }
    }

    /// Set a simple variable: `key` → accessible as `{{key}}`
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.values.insert(key.into(), value.into());
    }

    /// Set an issue context (from tracker trigger).
    pub fn set_issue(&mut self, title: &str, body: &str, number: &str, url: &str, labels: &[String]) {
        self.values.insert("issue.title".into(), title.into());
        self.values.insert("issue.body".into(), body.into());
        self.values.insert("issue.number".into(), number.into());
        self.values.insert("issue.url".into(), url.into());
        self.values.insert("issue.labels".into(), labels.join(", "));
    }

    /// Record a step result for use by subsequent steps.
    /// For structured outputs, also sets `.data`, `.summary`, `.status` and
    /// the always-serialized `.data_json` variables.
    ///
    /// `data` vs `data_json`: `.data` unwraps string values (convenient for
    /// prompt interpolation — `"tickets": "EW-1, EW-2"` renders cleanly),
    /// while `.data_json` preserves JSON representation (convenient for
    /// piping into an HTTP body or a downstream JSON parser). Needed for
    /// the désagentification ApiCall→ApiCall path where nothing re-parses.
    pub fn set_step_output(&mut self, step_name: &str, output: &str) {
        self.values.insert(format!("steps.{}.output", step_name), output.into());
        self.values.insert("previous_step.output".into(), output.into());

        // Try to extract structured envelope
        if let Some(envelope) = extract_step_envelope(output) {
            self.values.insert(format!("steps.{}.data", step_name), envelope.data.clone());
            self.values.insert(format!("steps.{}.summary", step_name), envelope.summary.clone());
            self.values.insert(format!("steps.{}.status", step_name), envelope.status.clone());
            self.values.insert(format!("steps.{}.data_json", step_name), envelope.data_json.clone());
            self.values.insert("previous_step.data".into(), envelope.data);
            self.values.insert("previous_step.summary".into(), envelope.summary);
            self.values.insert("previous_step.status".into(), envelope.status);
            self.values.insert("previous_step.data_json".into(), envelope.data_json);
        }
    }

    /// Render a template string, replacing all `{{variable}}` occurrences.
    pub fn render(&self, template: &str) -> Result<String> {
        let mut result = String::with_capacity(template.len());
        let mut chars = template.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '{' && chars.peek() == Some(&'{') {
                chars.next(); // consume second '{'
                // Read variable name until '}}'
                let mut var_name = String::new();
                loop {
                    match chars.next() {
                        Some('}') if chars.peek() == Some(&'}') => {
                            chars.next(); // consume second '}'
                            break;
                        }
                        Some(ch) => var_name.push(ch),
                        None => {
                            // Unclosed template — output as-is
                            result.push_str("{{");
                            result.push_str(&var_name);
                            return Ok(result);
                        }
                    }
                }

                let key = var_name.trim();
                if let Some(value) = self.values.get(key) {
                    result.push_str(value);
                } else {
                    // Unknown variable — leave placeholder for debugging
                    result.push_str("{{");
                    result.push_str(key);
                    result.push_str("}}");
                }
            } else {
                result.push(c);
            }
        }

        Ok(result)
    }
}

/// Validate that every `{{steps.X.Y}}` / `{{previous_step.Y}}` reference in
/// the workflow's step prompts points to a valid upstream step whose
/// `output_format` is `Structured`. Returns `Ok(())` if the workflow is
/// internally consistent, or a list of human-readable errors otherwise.
///
/// Design-time companion to the runtime `find_unresolved_critical_refs`:
/// together they block the Workflow B bug class at two layers — save-time
/// validation catches the mistake the moment the user clicks Save, and the
/// runtime check catches any remaining edge cases (e.g. envelope extraction
/// failing even though `output_format` is Structured).
pub fn validate_step_references(steps: &[crate::models::WorkflowStep]) -> Result<(), Vec<String>> {
    use crate::models::StepOutputFormat;
    static RE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(
            r"\{\{\s*(?:steps\.([A-Za-z0-9_\-]+)\.(data|summary|status|data_json)|previous_step\.(data|summary|status|data_json))\s*\}\}"
        ).unwrap()
    });

    let mut errors = Vec::new();
    for (idx, step) in steps.iter().enumerate() {
        for caps in RE.captures_iter(&step.prompt_template) {
            if let (Some(name), Some(field)) = (caps.get(1), caps.get(2)) {
                let target_name = name.as_str();
                let target_field = field.as_str();
                // Look only in strictly-upstream steps (can't read self or future)
                let upstream = steps.iter().take(idx).find(|s| s.name == target_name);
                match upstream {
                    Some(target) if target.output_format == StepOutputFormat::Structured => {}
                    Some(target) => errors.push(format!(
                        "Étape '{}' référence {{{{steps.{}.{}}}}}, mais l'étape '{}' est en output_format: FreeText. Passe-la en Structured pour qu'elle expose .data / .summary / .status.",
                        step.name, target_name, target_field, target.name
                    )),
                    None => {
                        if steps.iter().any(|s| s.name == target_name) {
                            // Either self-reference or forward-reference
                            errors.push(format!(
                                "Étape '{}' référence {{{{steps.{}.{}}}}}, mais cette étape n'est pas exécutée avant. Les références en avant ou sur soi-même ne sont pas supportées.",
                                step.name, target_name, target_field
                            ));
                        } else {
                            errors.push(format!(
                                "Étape '{}' référence {{{{steps.{}.{}}}}}, mais aucune étape ne porte le nom '{}'.",
                                step.name, target_name, target_field, target_name
                            ));
                        }
                    }
                }
            } else if let Some(field) = caps.get(3) {
                let target_field = field.as_str();
                if idx == 0 {
                    errors.push(format!(
                        "Étape '{}' utilise {{{{previous_step.{}}}}} mais c'est la première étape du workflow — il n'y a pas de précédente.",
                        step.name, target_field
                    ));
                } else {
                    let prev = &steps[idx - 1];
                    if prev.output_format != StepOutputFormat::Structured {
                        errors.push(format!(
                            "Étape '{}' utilise {{{{previous_step.{}}}}}, mais l'étape précédente '{}' est en output_format: FreeText. Passe-la en Structured.",
                            step.name, target_field, prev.name
                        ));
                    }
                }
            }
        }
    }

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

/// Return the names of producer steps that need their `output_format`
/// flipped to `Structured` to satisfy the inter-step contract.
///
/// A producer is "healable" when:
///   - It exists strictly upstream of the consumer
///   - A downstream step references `{{steps.X.data|summary|status|data_json}}`
///     or (for the immediate predecessor) `{{previous_step.*}}`
///   - Its current `output_format` is `FreeText`
///
/// Forward references, self-references, and references to missing steps are
/// NOT healable — they're structural bugs that need manual intervention.
/// Those are reported by `validate_step_references`; the healer ignores them.
pub fn healable_producer_names(steps: &[crate::models::WorkflowStep]) -> Vec<String> {
    use crate::models::StepOutputFormat;
    static RE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(
            r"\{\{\s*(?:steps\.([A-Za-z0-9_\-]+)\.(?:data|summary|status|data_json)|previous_step\.(?:data|summary|status|data_json))\s*\}\}"
        ).unwrap()
    });

    let mut names = std::collections::BTreeSet::new();
    for (idx, step) in steps.iter().enumerate() {
        for caps in RE.captures_iter(&step.prompt_template) {
            if let Some(name_match) = caps.get(1) {
                let target_name = name_match.as_str();
                // Strictly upstream only
                if let Some(target) = steps.iter().take(idx).find(|s| s.name == target_name) {
                    if target.output_format == StepOutputFormat::FreeText {
                        names.insert(target.name.clone());
                    }
                }
            } else if idx > 0 {
                // previous_step.* — the immediate predecessor
                let prev = &steps[idx - 1];
                if prev.output_format == StepOutputFormat::FreeText {
                    names.insert(prev.name.clone());
                }
            }
        }
    }
    names.into_iter().collect()
}

/// Scan a rendered prompt for unresolved references that would poison an
/// agent call. We only flag the ones that signal a broken inter-step
/// contract: `{{steps.X.data|summary|status}}` and
/// `{{previous_step.data|summary|status}}`. The `.output` form always
/// resolves once a step has produced *any* text, so we ignore it.
///
/// Why fail-fast on these: when the upstream step runs as `FreeText` or the
/// envelope extraction fails, `set_step_output` never inserts the `.data`
/// keys. The template engine then renders `{{steps.X.data}}` literally,
/// the agent receives that placeholder in its prompt, and panics with a
/// cryptic "tickets pas injectés" — which is really "Workflow B shipped
/// broken and nobody told you."
pub fn find_unresolved_critical_refs(rendered: &str) -> Vec<String> {
    static RE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        // Matches `{{steps.NAME.data|data_json|summary|status}}` or the
        // `{{previous_step.*}}` equivalent, tolerating whitespace inside
        // the braces the way TemplateContext::render does.
        regex_lite::Regex::new(
            r"\{\{\s*(steps\.[A-Za-z0-9_\-]+\.(?:data|summary|status|data_json)|previous_step\.(?:data|summary|status|data_json))\s*\}\}"
        ).unwrap()
    });
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for caps in RE.captures_iter(rendered) {
        if let Some(m) = caps.get(1) {
            let s = m.as_str().to_string();
            if seen.insert(s.clone()) {
                out.push(s);
            }
        }
    }
    out
}

// ── Structured output extraction ────────────────────────────────────────────

/// Extracted envelope from a step's output.
#[derive(Debug, Clone)]
pub struct StepEnvelope {
    /// String-rendered data. Unwrapped when the JSON field was a string
    /// (so `"data": "hello"` → `data = "hello"` without the quotes).
    pub data: String,
    pub status: String,
    pub summary: String,
    /// Always-serialized JSON form of `data`. Preserves the JSON
    /// representation (so `"data": "hello"` → `data_json = "\"hello\""`).
    /// Lets downstream ApiCall steps inject the value as a valid JSON
    /// body without re-parsing.
    pub data_json: String,
}

/// Instructions injected into prompts when `output_format = Structured`.
pub const STRUCTURED_OUTPUT_INSTRUCTIONS: &str = "\n\n\
---\n\
You must structure your response as follows:\n\n\
1. First, do your analysis/reasoning in plain text.\n\n\
2. Then, end your response with EXACTLY this format:\n\n\
---STEP_OUTPUT---\n\
{\"data\": <your structured result>, \"status\": \"OK\", \"summary\": \"<one sentence>\"}\n\
---END_STEP_OUTPUT---\n\n\
Rules:\n\
- status must be one of: OK, NO_RESULTS, ERROR\n\
- summary must be a single sentence describing what you found/did\n\
- data contains your actual output (array, object, or string) that the next step will consume\n\
- The ---STEP_OUTPUT--- block must be the LAST thing in your response\n\
- Do NOT wrap the JSON in markdown code fences inside the delimiters\n\
- If you found nothing relevant, use status NO_RESULTS with data as an empty array []";

/// Repair prompt sent when the LLM didn't produce the envelope.
pub const REPAIR_PROMPT_TEMPLATE: &str = "\
Your previous response did not include the required output format.\n\n\
Here is what you wrote:\n---\n{PREVIOUS_OUTPUT}\n---\n\n\
Now rewrite ONLY the structured output block. Do not repeat your analysis.\n\n\
---STEP_OUTPUT---\n\
{{\"data\": <extract the key result from above>, \"status\": \"OK\", \"summary\": \"<one sentence>\"}}\n\
---END_STEP_OUTPUT---";

/// Try to extract a `---STEP_OUTPUT--- ... ---END_STEP_OUTPUT---` envelope from raw text.
pub fn extract_step_envelope(text: &str) -> Option<StepEnvelope> {
    // Strategy 1: delimited block
    if let Some(start) = text.find("---STEP_OUTPUT---") {
        let after_delim = &text[start + "---STEP_OUTPUT---".len()..];
        if let Some(end) = after_delim.find("---END_STEP_OUTPUT---") {
            let json_str = after_delim[..end].trim();
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                return envelope_from_json(&parsed);
            }
            // Try stripping markdown code fences
            let stripped = json_str
                .trim_start_matches("```json").trim_start_matches("```")
                .trim_end_matches("```")
                .trim();
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(stripped) {
                return envelope_from_json(&parsed);
            }
        }
    }

    // Strategy 2: find last JSON object with "data" and "status" fields
    let mut last_match = None;
    for (i, _) in text.rmatch_indices('{') {
        let candidate = &text[i..];
        // Find the matching closing brace
        let mut depth = 0i32;
        let mut end_idx = 0;
        for (j, ch) in candidate.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 { end_idx = j + 1; break; }
                }
                _ => {}
            }
        }
        if end_idx == 0 { continue; }
        let json_candidate = &candidate[..end_idx];
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_candidate) {
            if parsed.get("data").is_some() && parsed.get("status").is_some() {
                last_match = envelope_from_json(&parsed);
                break; // found the last (rightmost) valid envelope
            }
        }
    }

    last_match
}

fn envelope_from_json(val: &serde_json::Value) -> Option<StepEnvelope> {
    let data = val.get("data")?;
    let status = val.get("status")?.as_str().unwrap_or("OK");
    let summary = val.get("summary").and_then(|s| s.as_str()).unwrap_or("");

    // `data`: unwrap strings for clean prompt interpolation.
    let data_str = if let Some(s) = data.as_str() {
        s.to_string()
    } else {
        serde_json::to_string(data).unwrap_or_default()
    };
    // `data_json`: always-serialized JSON, usable directly as an HTTP body.
    let data_json = serde_json::to_string(data).unwrap_or_default();

    Some(StepEnvelope {
        data: data_str,
        status: status.to_string(),
        summary: summary.to_string(),
        data_json,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_variable() {
        let mut ctx = TemplateContext::new();
        ctx.set("name", "world");
        assert_eq!(ctx.render("Hello {{name}}!").unwrap(), "Hello world!");
    }

    #[test]
    fn test_issue_context() {
        let mut ctx = TemplateContext::new();
        ctx.set_issue("Bug 500", "Server crash", "42", "https://gh/42", &["bug".into()]);
        let result = ctx.render("Fix: {{issue.title}} (#{{issue.number}})").unwrap();
        assert_eq!(result, "Fix: Bug 500 (#42)");
    }

    #[test]
    fn test_step_output() {
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("analyze", "Root cause: null pointer");
        let result = ctx.render("Previous: {{previous_step.output}}").unwrap();
        assert_eq!(result, "Previous: Root cause: null pointer");
        let result2 = ctx.render("From analyze: {{steps.analyze.output}}").unwrap();
        assert_eq!(result2, "From analyze: Root cause: null pointer");
    }

    #[test]
    fn test_unknown_variable_preserved() {
        let ctx = TemplateContext::new();
        assert_eq!(ctx.render("{{unknown}}").unwrap(), "{{unknown}}");
    }

    #[test]
    fn test_no_templates() {
        let ctx = TemplateContext::new();
        assert_eq!(ctx.render("plain text").unwrap(), "plain text");
    }

    #[test]
    fn test_whitespace_in_braces() {
        let mut ctx = TemplateContext::new();
        ctx.set("name", "test");
        assert_eq!(ctx.render("{{ name }}").unwrap(), "test");
    }

    #[test]
    fn test_issue_body() {
        let mut ctx = TemplateContext::new();
        ctx.set_issue("Title", "Detailed body text", "7", "https://gh/7", &[]);
        assert_eq!(ctx.render("{{issue.body}}").unwrap(), "Detailed body text");
    }

    #[test]
    fn test_unclosed_template_no_panic() {
        let ctx = TemplateContext::new();
        // Must not panic; returns what was consumed before EOF
        let result = ctx.render("prefix {{unclosed").unwrap();
        assert!(result.starts_with("prefix {{"));
    }

    #[test]
    fn test_empty_template_string() {
        let ctx = TemplateContext::new();
        assert_eq!(ctx.render("").unwrap(), "");
    }

    #[test]
    fn test_passthrough_no_variables() {
        let ctx = TemplateContext::new();
        let s = "Hello world, no braces here!";
        assert_eq!(ctx.render(s).unwrap(), s);
    }

    // ── Extraction tests ──

    #[test]
    fn test_extract_delimited_envelope() {
        let text = "Some reasoning here.\n\n---STEP_OUTPUT---\n{\"data\": [1, 2, 3], \"status\": \"OK\", \"summary\": \"Found 3 items\"}\n---END_STEP_OUTPUT---";
        let env = extract_step_envelope(text).unwrap();
        assert_eq!(env.status, "OK");
        assert_eq!(env.summary, "Found 3 items");
        assert_eq!(env.data, "[1,2,3]");
    }

    #[test]
    fn test_extract_delimited_with_code_fences() {
        let text = "Analysis done.\n\n---STEP_OUTPUT---\n```json\n{\"data\": {\"count\": 5}, \"status\": \"OK\", \"summary\": \"5 results\"}\n```\n---END_STEP_OUTPUT---";
        let env = extract_step_envelope(text).unwrap();
        assert_eq!(env.status, "OK");
        assert_eq!(env.data, "{\"count\":5}");
    }

    #[test]
    fn test_extract_no_delimiters_fallback_json() {
        let text = "Here is the analysis.\n\n{\"data\": [\"a\", \"b\"], \"status\": \"NO_RESULTS\", \"summary\": \"Nothing found\"}";
        let env = extract_step_envelope(text).unwrap();
        assert_eq!(env.status, "NO_RESULTS");
        assert_eq!(env.summary, "Nothing found");
    }

    #[test]
    fn test_extract_no_envelope_returns_none() {
        let text = "Just plain text with no JSON at all.";
        assert!(extract_step_envelope(text).is_none());
    }

    #[test]
    fn test_extract_string_data() {
        let text = "---STEP_OUTPUT---\n{\"data\": \"The root cause is a null pointer.\", \"status\": \"OK\", \"summary\": \"Found root cause\"}\n---END_STEP_OUTPUT---";
        let env = extract_step_envelope(text).unwrap();
        assert_eq!(env.data, "The root cause is a null pointer.");
    }

    // ── Unresolved critical references ──
    //
    // Prior behavior: `render()` silently left `{{steps.X.data}}` in place
    // when the upstream step wasn't Structured, and the agent received the
    // literal placeholder. These tests pin the detection that feeds the
    // runner's fail-fast path.

    #[test]
    fn find_unresolved_critical_refs_detects_steps_data() {
        let rendered = "Use this list: {{steps.main.data}} to analyze.";
        let refs = find_unresolved_critical_refs(rendered);
        assert_eq!(refs, vec!["steps.main.data"]);
    }

    #[test]
    fn find_unresolved_critical_refs_detects_previous_step_fields() {
        let rendered = "summary={{previous_step.summary}} status={{previous_step.status}}";
        let refs = find_unresolved_critical_refs(rendered);
        assert_eq!(refs, vec!["previous_step.summary", "previous_step.status"]);
    }

    #[test]
    fn find_unresolved_critical_refs_dedups_repeats() {
        let rendered = "{{steps.a.data}} / {{steps.a.data}} / {{steps.a.data}}";
        let refs = find_unresolved_critical_refs(rendered);
        assert_eq!(refs, vec!["steps.a.data"], "Repeats must be deduplicated");
    }

    // ── healable_producer_names ──
    //
    // The healing pass uses this to auto-upgrade FreeText producers that
    // are referenced via .data/.summary/etc. Not healable: forward refs,
    // self refs, missing-name refs. Those need human triage.

    #[test]
    fn healable_detects_freetext_producer() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("main", "Fetch", FreeText),
            step("use", "Use {{steps.main.data}}", FreeText),
        ];
        assert_eq!(healable_producer_names(&steps), vec!["main".to_string()]);
    }

    #[test]
    fn healable_dedups_multiple_references() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("main", "Fetch", FreeText),
            step("a", "{{steps.main.data}}", FreeText),
            step("b", "{{steps.main.summary}} {{steps.main.data_json}}", FreeText),
        ];
        // Still just `main` — healing idempotent on a single producer
        assert_eq!(healable_producer_names(&steps), vec!["main".to_string()]);
    }

    #[test]
    fn healable_skips_already_structured() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("main", "Fetch", Structured),
            step("use", "{{steps.main.data}}", FreeText),
        ];
        assert!(healable_producer_names(&steps).is_empty(),
            "Already-Structured producers must not reappear");
    }

    #[test]
    fn healable_ignores_forward_references() {
        use crate::models::StepOutputFormat::*;
        // Forward ref is a structural bug, not something the healer fixes.
        let steps = vec![
            step("a", "Use {{steps.b.data}}", FreeText),
            step("b", "Produce", FreeText),
        ];
        assert!(healable_producer_names(&steps).is_empty(),
            "Forward refs must stay for validate_step_references to flag");
    }

    #[test]
    fn healable_ignores_unknown_step_name() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("only", "Use {{steps.ghost.data}}", FreeText),
        ];
        assert!(healable_producer_names(&steps).is_empty());
    }

    #[test]
    fn healable_previous_step_upgrades_predecessor() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("a", "Fetch", FreeText),
            step("b", "{{previous_step.summary}}", FreeText),
        ];
        assert_eq!(healable_producer_names(&steps), vec!["a".to_string()]);
    }

    #[test]
    fn healable_previous_step_on_first_step_noop() {
        use crate::models::StepOutputFormat::*;
        // First step's `previous_step.*` is a structural bug — not healable.
        let steps = vec![
            step("first", "{{previous_step.data}}", FreeText),
        ];
        assert!(healable_producer_names(&steps).is_empty());
    }

    #[test]
    fn healable_regression_workflow_b() {
        use crate::models::StepOutputFormat::*;
        // Exact shape of Workflow B. A single boot healing pass upgrades
        // `main` from FreeText to Structured, after which the next run
        // will produce the `---STEP_OUTPUT---` envelope.
        let steps = vec![
            step("main", "Récupère les tickets EW", FreeText),
            step("analyze", "Analyse ces tickets : {{steps.main.data}}", FreeText),
        ];
        assert_eq!(healable_producer_names(&steps), vec!["main".to_string()]);
    }

    #[test]
    fn find_unresolved_critical_refs_detects_data_json() {
        // Regression for the data_json addition: the new variable is part
        // of the structured contract and must fail-fast / validate the
        // same way as `.data`.
        let rendered = "{{steps.main.data_json}} is the body";
        let refs = find_unresolved_critical_refs(rendered);
        assert_eq!(refs, vec!["steps.main.data_json"]);
    }

    #[test]
    fn validate_flags_data_json_on_freetext_producer() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("main", "Fetch", FreeText),
            step("use", "POST body {{steps.main.data_json}}", FreeText),
        ];
        let err = validate_step_references(&steps).unwrap_err();
        assert!(err[0].contains("data_json"), "data_json must participate in validation, got {:?}", err);
    }

    #[test]
    fn find_unresolved_critical_refs_ignores_output_and_non_ref() {
        // `.output` always resolves once a step has run, so never flag it.
        // And `{{foo}}` / `{{steps.x.other}}` are not part of the contract.
        let rendered = "{{steps.x.output}} {{foo}} {{steps.x.tokens}}";
        let refs = find_unresolved_critical_refs(rendered);
        assert!(refs.is_empty(), "Non-contract refs must be ignored, got {:?}", refs);
    }

    #[test]
    fn find_unresolved_critical_refs_tolerates_whitespace() {
        let rendered = "{{  steps.collect.data  }}";
        let refs = find_unresolved_critical_refs(rendered);
        assert_eq!(refs, vec!["steps.collect.data"]);
    }

    #[test]
    fn find_unresolved_critical_refs_ignores_resolved_refs() {
        // If the upstream step was Structured, set_step_output populated the
        // variable and render() substituted it. Nothing should be flagged.
        let mut ctx = TemplateContext::new();
        let output = "---STEP_OUTPUT---\n{\"data\": \"EW-1\", \"status\": \"OK\", \"summary\": \"one\"}\n---END_STEP_OUTPUT---";
        ctx.set_step_output("collect", output);
        let rendered = ctx.render("List: {{steps.collect.data}}").unwrap();
        let refs = find_unresolved_critical_refs(&rendered);
        assert!(refs.is_empty(), "Resolved refs must disappear, got {:?}", refs);
    }

    #[test]
    fn find_unresolved_critical_refs_regression_workflow_b() {
        // The exact shape of the Workflow B bug: step 1 ran as FreeText (or
        // its envelope extraction failed) so `steps.main.data` never got
        // populated. The runner must detect this before calling the agent.
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("main", "| Ticket | Résumé |\n|--|--|\n| EW-7181 | ... |"); // no envelope
        let rendered = ctx.render("Analyse les tickets: {{steps.main.data}}").unwrap();
        let refs = find_unresolved_critical_refs(&rendered);
        assert_eq!(refs, vec!["steps.main.data"],
            "Workflow B regression: FreeText upstream must be caught before agent call");
    }

    // ── validate_step_references ──
    //
    // Design-time validation: refuse to save a workflow where a step
    // references .data/.summary/.status from an upstream step that isn't
    // Structured. This is the UX-level companion to the runtime fail-fast.

    fn step(name: &str, prompt: &str, fmt: crate::models::StepOutputFormat) -> crate::models::WorkflowStep {
        crate::models::WorkflowStep {
            name: name.into(),
            step_type: crate::models::StepType::default(),
            description: None,
            agent: crate::models::AgentType::ClaudeCode,
            prompt_template: prompt.into(),
            mode: crate::models::StepMode::Normal,
            output_format: fmt,
            mcp_config_ids: vec![],
            agent_settings: None,
            on_result: vec![],
            stall_timeout_secs: None,
            retry: None,
            skill_ids: vec![],
            directive_ids: vec![],
            profile_ids: vec![],
            delay_after_secs: None,
            batch_quick_prompt_id: None,
            batch_items_from: None,
            batch_wait_for_completion: None,
            batch_max_items: None,
            batch_workspace_mode: None,
            batch_chain_prompt_ids: vec![],
            notify_config: None,
        }
    }

    #[test]
    fn validate_ok_when_upstream_is_structured() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("main", "Fetch tickets", Structured),
            step("analyze", "Analyze {{steps.main.data}}", FreeText),
        ];
        assert!(validate_step_references(&steps).is_ok());
    }

    #[test]
    fn validate_ok_when_no_refs_at_all() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("a", "Just do A", FreeText),
            step("b", "Just do B", FreeText),
        ];
        assert!(validate_step_references(&steps).is_ok(),
            "No contract refs = no validation required");
    }

    #[test]
    fn validate_regression_workflow_b() {
        use crate::models::StepOutputFormat::*;
        // Exact shape of Workflow B: step 1 in FreeText, step 2 references .data.
        let steps = vec![
            step("main", "Récupère les tickets EW", FreeText),
            step("analyze", "Analyse ces tickets : {{steps.main.data}}", FreeText),
        ];
        let err = validate_step_references(&steps).unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(err[0].contains("main"), "Error must name the offending producer");
        assert!(err[0].contains("Structured"), "Error must suggest the fix");
    }

    #[test]
    fn validate_blocks_forward_reference() {
        use crate::models::StepOutputFormat::*;
        // Step 1 references step 2 — the runner executes linearly so this
        // can never work, even if step 2 is Structured.
        let steps = vec![
            step("a", "Use {{steps.b.data}}", FreeText),
            step("b", "Produce data", Structured),
        ];
        let err = validate_step_references(&steps).unwrap_err();
        assert!(err[0].contains("en avant"), "Error must explain forward-ref is unsupported, got {:?}", err);
    }

    #[test]
    fn validate_blocks_self_reference() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("loop", "Refer to {{steps.loop.data}}", Structured),
        ];
        let err = validate_step_references(&steps).unwrap_err();
        assert!(err[0].contains("en avant") || err[0].contains("soi"),
            "Self-ref must be flagged, got {:?}", err);
    }

    #[test]
    fn validate_blocks_unknown_step_name() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("only", "Look at {{steps.ghost.data}}", FreeText),
        ];
        let err = validate_step_references(&steps).unwrap_err();
        assert!(err[0].contains("ghost"), "Error must name the missing step");
    }

    #[test]
    fn validate_previous_step_on_first_step_fails() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("first", "{{previous_step.data}}", FreeText),
        ];
        let err = validate_step_references(&steps).unwrap_err();
        assert!(err[0].contains("première étape") || err[0].contains("précédente"),
            "Must flag that the first step has no predecessor, got {:?}", err);
    }

    #[test]
    fn validate_previous_step_requires_structured_predecessor() {
        use crate::models::StepOutputFormat::*;
        let steps = vec![
            step("a", "Produce", FreeText),
            step("b", "{{previous_step.summary}}", FreeText),
        ];
        let err = validate_step_references(&steps).unwrap_err();
        assert!(err[0].contains("'a'") || err[0].contains("a "),
            "Must point at the predecessor by name, got {:?}", err);
    }

    #[test]
    fn validate_output_ref_does_not_require_structured() {
        use crate::models::StepOutputFormat::*;
        // `.output` is always populated — FreeText upstream is fine for it.
        let steps = vec![
            step("a", "Produce", FreeText),
            step("b", "Raw text: {{steps.a.output}} and {{previous_step.output}}", FreeText),
        ];
        assert!(validate_step_references(&steps).is_ok());
    }

    #[test]
    fn validate_collects_all_errors() {
        use crate::models::StepOutputFormat::*;
        // Multiple issues in one pass — the user should see all of them,
        // not fix-and-retry one at a time.
        let steps = vec![
            step("a", "Just produce", FreeText),
            step("b", "{{steps.a.data}} {{steps.ghost.summary}}", FreeText),
            step("c", "{{previous_step.status}}", FreeText),
        ];
        let err = validate_step_references(&steps).unwrap_err();
        assert!(err.len() >= 3, "Must return all errors at once, got {:?}", err);
    }

    // ── data_json template variable ──
    //
    // `data_json` is the always-serialized sibling of `data`. Existing
    // workflows relying on `.data` keep working; ApiCall chainers get a
    // JSON-ready payload without a re-parse in between.

    #[test]
    fn data_json_preserves_string_quoting() {
        let mut ctx = TemplateContext::new();
        let output = "---STEP_OUTPUT---\n{\"data\": \"hello\", \"status\": \"OK\", \"summary\": \"s\"}\n---END_STEP_OUTPUT---";
        ctx.set_step_output("s", output);
        // `.data` unwraps the string
        assert_eq!(ctx.render("{{steps.s.data}}").unwrap(), "hello");
        // `.data_json` keeps it JSON-quoted
        assert_eq!(ctx.render("{{steps.s.data_json}}").unwrap(), "\"hello\"");
    }

    #[test]
    fn data_json_matches_data_for_arrays_and_objects() {
        let mut ctx = TemplateContext::new();
        let output = "---STEP_OUTPUT---\n{\"data\": [1,2,3], \"status\": \"OK\", \"summary\": \"s\"}\n---END_STEP_OUTPUT---";
        ctx.set_step_output("s", output);
        // Both render to the same JSON when `data` is not a string.
        assert_eq!(ctx.render("{{steps.s.data}}").unwrap(), "[1,2,3]");
        assert_eq!(ctx.render("{{steps.s.data_json}}").unwrap(), "[1,2,3]");
    }

    #[test]
    fn data_json_valid_for_api_body_piping() {
        // Regression for the ApiCall→ApiCall pipeline: the string produced
        // by `.data_json` must roundtrip through serde_json::from_str.
        let mut ctx = TemplateContext::new();
        let output = "---STEP_OUTPUT---\n{\"data\": {\"id\": 42, \"tags\": [\"a\", \"b\"]}, \"status\": \"OK\", \"summary\": \"s\"}\n---END_STEP_OUTPUT---";
        ctx.set_step_output("s", output);
        let rendered = ctx.render("{{steps.s.data_json}}").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&rendered)
            .expect("data_json must be valid JSON");
        assert_eq!(parsed["id"], 42);
        assert_eq!(parsed["tags"][0], "a");
    }

    #[test]
    fn previous_step_data_json_exposed() {
        let mut ctx = TemplateContext::new();
        let output = "---STEP_OUTPUT---\n{\"data\": [\"x\"], \"status\": \"OK\", \"summary\": \"s\"}\n---END_STEP_OUTPUT---";
        ctx.set_step_output("main", output);
        assert_eq!(ctx.render("{{previous_step.data_json}}").unwrap(), "[\"x\"]");
    }

    #[test]
    fn data_json_absent_when_no_envelope() {
        // Mirror of existing .data behavior — a FreeText upstream leaves
        // the variable unresolved, which our fail-fast / validator catch.
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("s", "just raw text, no envelope");
        assert_eq!(
            ctx.render("{{steps.s.data_json}}").unwrap(),
            "{{steps.s.data_json}}"
        );
    }

    #[test]
    fn test_structured_variables_available() {
        let mut ctx = TemplateContext::new();
        let output = "Reasoning.\n\n---STEP_OUTPUT---\n{\"data\": [\"pr-1\", \"pr-2\"], \"status\": \"OK\", \"summary\": \"2 orphan PRs\"}\n---END_STEP_OUTPUT---";
        ctx.set_step_output("collect", output);

        assert_eq!(ctx.render("{{steps.collect.data}}").unwrap(), "[\"pr-1\",\"pr-2\"]");
        assert_eq!(ctx.render("{{steps.collect.summary}}").unwrap(), "2 orphan PRs");
        assert_eq!(ctx.render("{{steps.collect.status}}").unwrap(), "OK");
        assert_eq!(ctx.render("{{previous_step.data}}").unwrap(), "[\"pr-1\",\"pr-2\"]");
        assert_eq!(ctx.render("{{previous_step.summary}}").unwrap(), "2 orphan PRs");
        // .output still contains the full raw text
        assert!(ctx.render("{{previous_step.output}}").unwrap().contains("Reasoning"));
    }
}
