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

        // 0.7.0 Phase 3 — also extract any `---ARTIFACT:<name>---` blocks
        // and expose them as `{{artifacts.<name>}}`. Persistence to disk
        // is handled by the runner (which knows the workspace path);
        // this just makes the content available to subsequent steps via
        // the same render path. Re-emitting the same artifact on a
        // later step overwrites the previous render — Auto-Dev's
        // implement→review→implement loop relies on this.
        for (name, content) in extract_artifacts(output) {
            self.values.insert(format!("artifacts.{}", name), content);
        }

        // 0.7.0 Phase 6 — same pattern for `---STATE:<k>=<v>---` lines.
        // Persistence to the run row is handled by the runner; this
        // just exposes the freshly-written state to subsequent steps in
        // the same iteration via `{{state.<k>}}`. Re-writing a key
        // overwrites — the run.state map ends up with the last value.
        for (k, v) in extract_state(output) {
            self.values.insert(format!("state.{}", k), v);
        }
    }

    /// Seed the context with previously-persisted artifacts at run start.
    /// Used by the runner to make `{{artifacts.<name>}}` resolve before
    /// the first step that produces them — Auto-Dev's first
    /// `implement` iteration reads `{{artifacts.review | if_exists}}`
    /// which is empty on round 1 (no review yet) and populated on
    /// rounds 2+ once the previous iteration wrote it.
    pub fn seed_artifacts(&mut self, artifacts: &::std::collections::HashMap<String, String>) {
        for (name, content) in artifacts {
            self.values.insert(format!("artifacts.{}", name), content.clone());
        }
    }

    /// 0.7.0 Phase 6 — seed the context with previously-persisted run
    /// state at run start (and on resume after a Gate pause). Without
    /// this the first step of a fresh iteration would see empty
    /// `{{state.X}}` even though the durable state on the run row is
    /// non-empty.
    pub fn seed_state(&mut self, state: &::std::collections::HashMap<String, String>) {
        for (k, v) in state {
            self.values.insert(format!("state.{}", k), v.clone());
        }
    }

    /// Resolve a single `{{key}}` to its TYPED `serde_json::Value` (array /
    /// object / number / … preserved), or None if unknown. The api_body
    /// renderer uses this for whole-placeholder fields so a nested array/object
    /// is injected as real JSON rather than an escaped string. A flat string
    /// value is returned as `Value::String` (so scalars stay scalars).
    pub fn resolve_value(&self, key: &str) -> Option<serde_json::Value> {
        // Typed resolution FIRST so a `.data` / `.data_json` (whole or nested)
        // yields the PARSED value, not the flat unwrapped string. Non-data keys
        // (launch vars, `current_task.<field>`, …) fall through to the flat
        // string value, preserving scalar-as-string behaviour.
        if let Some(v) = resolve_typed_path(&self.values, key) {
            return Some(v);
        }
        self.values.get(key).map(|v| serde_json::Value::String(v.clone()))
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
                } else if let Some(value) = resolve_nested_path(&self.values, key) {
                    result.push_str(&value);
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

/// Resolve a dotted path like `steps.X.data.subtasks.0.title` against a flat
/// values map. The map only stores the top-level pseudo-keys
/// (`steps.X.data`, `steps.X.data_json`, `previous_step.data`, etc.); to look
/// up a sub-field we parse the always-JSON sibling (`*.data_json`) and walk
/// the parsed value.
///
/// **Why JSON over the unwrapped `.data`** : `data` strips quotes for clean
/// prompt interpolation (`"hello"` → `hello`), making it unparseable when
/// `data` was a plain string. `data_json` preserves a valid JSON representation
/// in every case, so traversal is always well-defined.
///
/// **Path semantics** :
///   - dot-separated segments after `.data` (e.g. `data.subtasks.0.title`)
///   - numeric segments index arrays (`subtasks.0` = first subtask)
///   - string segments index objects
///   - missing field anywhere → returns None (renderer leaves the literal
///     `{{...}}` so the broken reference is visible to the operator)
///
/// **Output shape** :
///   - JSON strings → raw string content (no surrounding quotes), to mirror
///     `.data`'s unwrapping behavior
///   - numbers / booleans / null → their stringified form
///   - arrays / objects → pretty-printed JSON (operator-friendly inside a
///     markdown code block, indexable downstream)
pub(crate) fn resolve_nested_path(values: &HashMap<String, String>, key: &str) -> Option<String> {
    resolve_typed_path(values, key).map(|v| stringify_json_leaf(&v))
}

/// Anchor a dotted key on its `.data` OR `.data_json` segment, returning
/// `(prefix, path-after-anchor)`. `steps.X.data.subtasks.0` and
/// `steps.X.data_json.subtasks.0` both anchor identically — the JSON source is
/// always the parseable `<prefix>.data_json` sibling regardless of which alias
/// the author wrote. Returns None when there's no anchor, the anchor is the
/// last segment (a bare flat key, handled elsewhere), or it's the first segment
/// (no prefix).
fn anchor_and_path(key: &str) -> Option<(String, Vec<&str>)> {
    let parts: Vec<&str> = key.split('.').collect();
    let idx = parts.iter().position(|p| *p == "data" || *p == "data_json")?;
    if idx == 0 || idx == parts.len() - 1 {
        return None;
    }
    Some((parts[..idx].join("."), parts[idx + 1..].to_vec()))
}

/// Like [`resolve_nested_path`] but returns the TYPED `serde_json::Value` at the
/// path (array/object/number/… preserved) instead of a stringified form. Used
/// by the api_body renderer to inject a real nested array/object into a JSON
/// field (`"comments": "{{steps.review.data.inlineComments}}"` → a real array),
/// which `stringify_json_leaf` could only express as an escaped string. Also
/// resolves a bare `<prefix>.data` / `.data_json` to the whole parsed payload.
pub(crate) fn resolve_typed_path(values: &HashMap<String, String>, key: &str) -> Option<serde_json::Value> {
    let parts: Vec<&str> = key.split('.').collect();
    // Whole-payload form: `<prefix>.data` or `<prefix>.data_json` (no sub-path).
    if parts.len() >= 2 && matches!(*parts.last().unwrap(), "data" | "data_json") {
        let prefix = parts[..parts.len() - 1].join(".");
        let json_str = values
            .get(&format!("{prefix}.data_json"))
            .or_else(|| values.get(&format!("{prefix}.data")))?;
        return serde_json::from_str(json_str).ok();
    }

    // Nested form: walk the path under the `.data`/`.data_json` anchor.
    let (prefix, path) = anchor_and_path(key)?;
    let json_str = values
        .get(&format!("{prefix}.data_json"))
        .or_else(|| values.get(&format!("{prefix}.data")))?;
    let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let mut current = &parsed;
    for segment in &path {
        current = if let Ok(idx) = segment.parse::<usize>() {
            current.as_array()?.get(idx)?
        } else {
            current.get(*segment)?
        };
    }
    Some(current.clone())
}

fn stringify_json_leaf(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        // Arrays / objects: pretty JSON. The operator usually consumes this
        // inside a markdown code block (gates) or downstream agent prompt
        // (which can re-parse). Compact form would be unreadable in gates.
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            serde_json::to_string_pretty(v).unwrap_or_default()
        }
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
    use crate::models::{StepOutputFormat, StepType};
    // Field group captures the immediate `<field>` after the step prefix.
    // For nested traversal (`steps.X.data.subtasks.0.title`) we still
    // anchor the validation on the top-level `data|data_json|summary|status`
    // — what matters at design time is that the producing step exposes a
    // structured envelope at all. Sub-path validity is best-effort
    // (resolved at run time, leaves placeholder if missing) since we don't
    // know the actual JSON shape ahead of execution.
    static RE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(
            r"\{\{\s*(?:steps\.([A-Za-z0-9_\-]+)\.(data|summary|status|data_json|output)((?:\.[A-Za-z0-9_\-]+)*)|previous_step\.(data|summary|status|data_json|output)((?:\.[A-Za-z0-9_\-]+)*))\s*\}\}"
        ).unwrap()
    });

    /// Le step émet-il une envelope Structured exploitable par
    /// `{{steps.X.data|summary|status|data_json}}` ? Pour l'Agent, ça
    /// dépend de son `output_format` (l'utilisateur peut choisir FreeText).
    /// Pour tous les autres step types, le runner émet TOUJOURS une
    /// envelope Structured peu importe le champ `output_format` du step
    /// (cf. notify_step.rs / api_call_executor.rs / json_data_step.rs / etc.) —
    /// donc on les considère comme producteurs Structured even si le
    /// wizard n'a pas explicitement set le field.
    fn produces_structured(step: &crate::models::WorkflowStep) -> bool {
        match step.step_type {
            StepType::Agent => matches!(
                step.output_format,
                StepOutputFormat::Structured | StepOutputFormat::TypedSchema { .. }
            ),
            // Les step types mécaniques émettent toujours une envelope
            // Structured. C'est leur contrat invariant.
            StepType::ApiCall
            | StepType::Notify
            | StepType::Gate
            | StepType::Exec
            | StepType::BatchApiCall
            | StepType::BatchQuickPrompt
            | StepType::JsonData
            // SubWorkflow's output is the child run's final envelope
            // (standardised) → `{{steps.<subwf>.data}}` is valid.
            | StepType::SubWorkflow => true,
        }
    }

    let mut errors = Vec::new();
    for (idx, step) in steps.iter().enumerate() {
        for caps in RE.captures_iter(&step.prompt_template) {
            if let (Some(name), Some(field)) = (caps.get(1), caps.get(2)) {
                let target_name = name.as_str();
                let target_field = field.as_str();
                // `.output` is raw text — a nested subpath can never resolve
                // and would ship the literal placeholder into the prompt.
                if target_field == "output" && caps.get(3).map(|m| !m.as_str().is_empty()).unwrap_or(false) {
                    errors.push(format!(
                        "Étape '{}' référence {{{{steps.{}.output{}}}}} — `.output` est du texte brut, il n'a pas de sous-chemin. Utilise {{{{steps.{}.output}}}} tel quel, ou passe par .data pour du JSON structuré.",
                        step.name, target_name, caps.get(3).map(|m| m.as_str()).unwrap_or(""), target_name
                    ));
                    continue;
                }
                // Look only in strictly-upstream steps (can't read self or future)
                let upstream = steps.iter().take(idx).find(|s| s.name == target_name);
                match upstream {
                    // `.output` is the RAW text — any upstream format is fine;
                    // only existence/ordering matter (a typo'd name used to
                    // ship the literal placeholder into the prompt, silently).
                    Some(_) if target_field == "output" => {}
                    Some(target) if produces_structured(target) => {}
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
            } else if let Some(field) = caps.get(4) {
                let target_field = field.as_str();
                if target_field == "output" && caps.get(5).map(|m| !m.as_str().is_empty()).unwrap_or(false) {
                    errors.push(format!(
                        "Étape '{}' référence {{{{previous_step.output{}}}}} — `.output` est du texte brut, il n'a pas de sous-chemin.",
                        step.name, caps.get(5).map(|m| m.as_str()).unwrap_or("")
                    ));
                    continue;
                }
                if idx == 0 {
                    errors.push(format!(
                        "Étape '{}' utilise {{{{previous_step.{}}}}} mais c'est la première étape du workflow — il n'y a pas de précédente.",
                        step.name, target_field
                    ));
                } else {
                    let prev = &steps[idx - 1];
                    if target_field != "output" && !produces_structured(prev) {
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
        // Matches `{{steps.NAME.<field>}}` and `{{previous_step.<field>}}`
        // where `<field>` is `data|summary|status|data_json` optionally
        // followed by a nested path (e.g. `data.subtasks.0.title`). Tolerates
        // whitespace inside the braces the way TemplateContext::render does.
        regex_lite::Regex::new(
            r"\{\{\s*(steps\.[A-Za-z0-9_\-]+\.(?:data|summary|status|data_json)(?:\.[A-Za-z0-9_\-]+)*|previous_step\.(?:data|summary|status|data_json)(?:\.[A-Za-z0-9_\-]+)*)\s*\}\}"
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

// ─── Artifacts (0.7.0 Phase 3) ───────────────────────────────────────────────

/// Extract every `---ARTIFACT:<name>---\n<content>\n---END_ARTIFACT---`
/// block from an agent's response. Returns a name → raw content map.
///
/// **Why blocks instead of a tool** : the same plain-text capture pattern
/// already powers `extract_step_envelope`, so the agent doesn't need a
/// new tool to interact with the runner — it just emits the right marker.
/// Works on every agent type without per-CLI plumbing.
///
/// **Trailing newline trimming** : we trim trailing `\n` so a re-rendered
/// artifact in `{{artifacts.X}}` doesn't add a blank line every time it's
/// referenced. Leading whitespace is preserved (Markdown indentation
/// matters).
pub fn extract_artifacts(text: &str) -> ::std::collections::HashMap<String, String> {
    use ::std::collections::HashMap;
    let mut out = HashMap::new();
    let mut cursor = 0usize;
    while let Some(start_rel) = text[cursor..].find("---ARTIFACT:") {
        let header_start = cursor + start_rel;
        // Header line: `---ARTIFACT:<name>---\n`. Find the closing `---`
        // on the same line.
        let after_prefix = &text[header_start + "---ARTIFACT:".len()..];
        let header_end_rel = match after_prefix.find("---") {
            Some(i) => i,
            None => break,
        };
        let name = after_prefix[..header_end_rel].trim().to_string();
        if name.is_empty() {
            cursor = header_start + 1;
            continue;
        }
        // Content starts after the header's closing `---` (skip optional `\n`).
        let content_start = header_start + "---ARTIFACT:".len() + header_end_rel + "---".len();
        let content_after = &text[content_start..];
        let content_after = content_after.strip_prefix('\n').unwrap_or(content_after);
        // Find the matching `---END_ARTIFACT---`.
        let end_rel = match content_after.find("---END_ARTIFACT---") {
            Some(i) => i,
            None => break,
        };
        let raw = &content_after[..end_rel];
        // Trim a single trailing newline (common LLM emission style) but
        // preserve everything else.
        let trimmed = raw.strip_suffix('\n').unwrap_or(raw).to_string();
        out.insert(name, trimmed);
        cursor = content_start + end_rel + "---END_ARTIFACT---".len();
    }
    out
}

// ─── State map (0.7.0 Phase 6) ───────────────────────────────────────────────

/// Extract every `---STATE:<key>=<value>---` line from an agent's
/// response. Returns a key → value map.
///
/// Sister of [`extract_artifacts`]: same plain-text capture pattern, but
/// single-line (a state entry is just a small string, no body block).
/// Multiple writes to the same key in one response → last write wins
/// (mirrors normal hash-map semantics, no surprise).
///
/// Format constraints:
/// - Header on its own line, no leading whitespace before `---STATE:`
/// - `<key>` is everything up to the first `=` (trimmed)
/// - `<value>` is everything after `=` up to `---` on the same line (trimmed)
/// - Empty key → entry skipped (defensive — no anonymous state)
/// - Empty value → kept (clearing a counter back to "" is legitimate)
///
/// Examples that match:
///   `---STATE:retry_count=3---`
///   `---STATE:last_verdict=approved---`
///   `---STATE:notes=---`              (empty value, key "notes" set to "")
pub fn extract_state(text: &str) -> ::std::collections::HashMap<String, String> {
    use ::std::collections::HashMap;
    let mut out = HashMap::new();
    let prefix = "---STATE:";
    let suffix = "---";
    let mut cursor = 0usize;
    while let Some(start_rel) = text[cursor..].find(prefix) {
        let header_start = cursor + start_rel;
        let after_prefix = &text[header_start + prefix.len()..];
        // Each entry is a single line — anything past `\n` ends the search.
        // Find the closing `---` BEFORE any newline.
        let line_end = after_prefix.find('\n').unwrap_or(after_prefix.len());
        let line_slice = &after_prefix[..line_end];
        let close_rel = match line_slice.find(suffix) {
            Some(i) => i,
            None => {
                cursor = header_start + prefix.len();
                continue;
            }
        };
        let body = &line_slice[..close_rel];
        // Split on the first `=`. Missing `=` → skip the entry (malformed).
        if let Some(eq_idx) = body.find('=') {
            let key = body[..eq_idx].trim().to_string();
            let value = body[eq_idx + 1..].trim().to_string();
            if !key.is_empty() {
                out.insert(key, value);
            }
        }
        cursor = header_start + prefix.len() + close_rel + suffix.len();
    }
    out
}

// ─── TypedSchema (0.7.0 Phase 2) ─────────────────────────────────────────────

/// Build the prompt instruction for an Agent step whose output_format is
/// `TypedSchema { schema }`. Reuses the `---STEP_OUTPUT---` envelope
/// contract (so downstream `{{previous_step.data}}` / `{{previous_step.status}}`
/// resolution stays uniform with vanilla `Structured`) but adds the
/// schema spec so the LLM produces conforming `data`.
pub fn build_typed_schema_instruction(schema: &serde_json::Value) -> String {
    let pretty = serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string());
    format!(
        "\n\n\
---\n\
You must structure your response as follows:\n\n\
1. First, do your analysis/reasoning in plain text.\n\n\
2. Then, end your response with EXACTLY this format:\n\n\
---STEP_OUTPUT---\n\
{{\"data\": <object conforming to the JSON schema below>, \"status\": \"OK\", \"summary\": \"<one sentence>\"}}\n\
---END_STEP_OUTPUT---\n\n\
The `data` field MUST validate against this JSON Schema:\n\n\
```json\n{}\n```\n\n\
Rules:\n\
- status must be one of: OK, NO_RESULTS, ERROR\n\
- summary must be a single sentence describing what you found/did\n\
- data MUST satisfy every field, type, and constraint in the schema above\n\
- The ---STEP_OUTPUT--- block must be the LAST thing in your response\n\
- Do NOT wrap the JSON in markdown code fences inside the delimiters",
        pretty
    )
}

/// Build a repair prompt that tells the LLM exactly what went wrong:
/// either "missing envelope" or "schema validation failed: <error>".
/// Phase-1 we only had `REPAIR_PROMPT_TEMPLATE`; that one stays valid
/// for vanilla `Structured` — the new builder routes per-format.
pub fn build_repair_prompt(
    previous_output: &str,
    output_format: &crate::models::StepOutputFormat,
    schema_error: Option<&str>,
) -> String {
    use crate::models::StepOutputFormat;
    match output_format {
        StepOutputFormat::TypedSchema { schema, .. } => {
            let pretty = serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string());
            let problem = schema_error
                .map(|e| format!("Your previous response failed: schema validation failed: {}\n\n", e))
                .unwrap_or_else(|| "Your previous response did not include the required output format.\n\n".into());
            format!(
                "{}\
Here is what you wrote:\n---\n{}\n---\n\n\
Now rewrite ONLY the structured output block. Do not repeat your analysis.\n\n\
The `data` field MUST validate against:\n\n```json\n{}\n```\n\n\
---STEP_OUTPUT---\n\
{{\"data\": <object conforming to the schema above>, \"status\": \"OK\", \"summary\": \"<one sentence>\"}}\n\
---END_STEP_OUTPUT---",
                problem, previous_output, pretty
            )
        }
        _ => REPAIR_PROMPT_TEMPLATE.replace("{PREVIOUS_OUTPUT}", previous_output),
    }
}

/// Validate the envelope's `data_json` (raw JSON string from the
/// `---STEP_OUTPUT---` block's `data` field) against a JSON-Schema subset.
///
/// **Subset supported** (deliberately tight — wide enough for Auto-Dev's
/// `validate_ticket` schema, narrow enough to keep the validator
/// understandable without pulling in the full `jsonschema` crate):
///
/// - `type`: `"string"` | `"integer"` | `"number"` | `"boolean"` |
///   `"array"` | `"object"` | `"null"`. Type mismatches reject.
/// - `enum`: array of allowed values. Anything not in the list rejects.
/// - `minimum` / `maximum`: numeric bounds (inclusive). Out-of-range rejects.
/// - `minLength` / `maxLength`: string length bounds. Out-of-range rejects.
/// - `properties`: per-field sub-schemas (recursive). Unknown fields are
///   tolerated by default (matches JSON Schema's `additionalProperties: true`).
/// - `required`: array of property names that must be present.
/// - `items`: sub-schema applied to every element of an array. The
///   first failing item is reported.
///
/// **Not supported** (deferred — no Auto-Dev step needs them yet):
///   `pattern`, `format`, `oneOf`, `anyOf`, `$ref`, `$defs`, `additionalProperties: false`.
pub fn validate_envelope_against_schema(
    data_json: &str,
    schema: &serde_json::Value,
) -> Result<(), String> {
    let data: serde_json::Value = serde_json::from_str(data_json)
        .map_err(|e| format!("data field is not valid JSON: {}", e))?;
    validate_value(&data, schema, "$")
}

fn validate_value(
    value: &serde_json::Value,
    schema: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    let schema_obj = match schema.as_object() {
        Some(o) => o,
        None => return Ok(()), // Empty/non-object schema accepts anything.
    };

    // type
    if let Some(ty) = schema_obj.get("type").and_then(|v| v.as_str()) {
        let actual = value_type_name(value);
        let ok = match ty {
            "number" => matches!(value, serde_json::Value::Number(_)),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            other => actual == other,
        };
        if !ok {
            return Err(format!("{}: expected type {}, got {}", path, ty, actual));
        }
    }

    // enum
    if let Some(allowed) = schema_obj.get("enum").and_then(|v| v.as_array()) {
        if !allowed.iter().any(|a| a == value) {
            return Err(format!(
                "{}: value {} is not in allowed enum {:?}",
                path, value, allowed
            ));
        }
    }

    // min/max numeric
    if let Some(n) = value.as_f64() {
        if let Some(min) = schema_obj.get("minimum").and_then(|v| v.as_f64()) {
            if n < min { return Err(format!("{}: {} < minimum {}", path, n, min)); }
        }
        if let Some(max) = schema_obj.get("maximum").and_then(|v| v.as_f64()) {
            if n > max { return Err(format!("{}: {} > maximum {}", path, n, max)); }
        }
    }

    // string length
    if let Some(s) = value.as_str() {
        if let Some(min) = schema_obj.get("minLength").and_then(|v| v.as_u64()) {
            if (s.chars().count() as u64) < min {
                return Err(format!("{}: length {} < minLength {}", path, s.chars().count(), min));
            }
        }
        if let Some(max) = schema_obj.get("maxLength").and_then(|v| v.as_u64()) {
            if (s.chars().count() as u64) > max {
                return Err(format!("{}: length {} > maxLength {}", path, s.chars().count(), max));
            }
        }
    }

    // properties + required (object case)
    if let Some(obj) = value.as_object() {
        if let Some(required) = schema_obj.get("required").and_then(|v| v.as_array()) {
            for req in required {
                if let Some(name) = req.as_str() {
                    if !obj.contains_key(name) {
                        return Err(format!("{}: missing required property '{}'", path, name));
                    }
                }
            }
        }
        if let Some(props) = schema_obj.get("properties").and_then(|v| v.as_object()) {
            for (name, sub_schema) in props {
                if let Some(child) = obj.get(name) {
                    let child_path = format!("{}.{}", path, name);
                    validate_value(child, sub_schema, &child_path)?;
                }
                // Missing optional property — covered by `required` above.
            }
        }
    }

    // items (array case)
    if let Some(arr) = value.as_array() {
        if let Some(item_schema) = schema_obj.get("items") {
            for (i, item) in arr.iter().enumerate() {
                let child_path = format!("{}[{}]", path, i);
                validate_value(item, item_schema, &child_path)?;
            }
        }
    }

    Ok(())
}

fn value_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Try to extract a `---STEP_OUTPUT--- ... ---END_STEP_OUTPUT---` envelope from raw text.
pub fn extract_step_envelope(text: &str) -> Option<StepEnvelope> {
    // Strategy 1: delimited block. When markers are PRESENT they are
    // authoritative — a malformed block returns None (→ repair) instead of
    // falling through to strategy 2, which could adopt a quoted example
    // from the agent's own reasoning.
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
        return None;
    }

    // Strategy 2 (legacy, marker-less — Batch* structured output): the LAST
    // top-level parsable JSON object of the text must ITSELF be a valid
    // envelope. No walking back to an earlier envelope-like object.
    let mut last_json: Option<serde_json::Value> = None;
    let mut i = 0;
    while let Some(rel) = text[i..].find('{') {
        let start = i + rel;
        let mut depth = 0i32;
        let mut end = None;
        // String-aware balancing: braces inside JSON strings don't count
        // (`{"data": "brace } inside", ...}` must balance at the real end).
        let mut in_string = false;
        let mut escaped = false;
        for (j, ch) in text[start..].char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' if in_string => escaped = true,
                '"' => in_string = !in_string,
                '{' if !in_string => depth += 1,
                '}' if !in_string => {
                    depth -= 1;
                    if depth == 0 { end = Some(start + j + 1); break; }
                }
                _ => {}
            }
        }
        match end {
            Some(e) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text[start..e]) {
                    last_json = Some(v);
                    i = e; // nested braces of a parsed object are not top-level candidates
                } else {
                    i = start + 1;
                }
            }
            None => break,
        }
    }
    last_json
        .filter(|v| v.get("data").is_some() && v.get("status").is_some())
        .as_ref()
        .and_then(envelope_from_json)
}

fn envelope_from_json(val: &serde_json::Value) -> Option<StepEnvelope> {
    let data = val.get("data")?;
    // The contract says `status` is a string. A present-but-non-string value
    // must NOT read as OK: `"status": null` is a model failing mid-envelope —
    // coercing that to success inverted the failure direction. Numbers keep
    // their text form (`200` → "200") so contains-rules still match them.
    let status = match val.get("status")? {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => "ERROR".to_string(),
    };
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
        status,
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

    // ── Nested-path tests (regression: 0.7.0 Ticket Autopilot gate
    //    recap displayed `{{steps.analyze.data.subtasks}}` literally
    //    because the templater only resolved top-level fields). ──

    /// Helper: feed a structured envelope to the context and let the regular
    /// `set_step_output` machinery populate `steps.X.data` + `steps.X.data_json`.
    fn ctx_with_envelope(step: &str, data_json_obj: &str, summary: &str) -> TemplateContext {
        let mut ctx = TemplateContext::new();
        ctx.set_step_output(
            step,
            &format!(
                "preamble\n---STEP_OUTPUT---\n{{\"data\": {}, \"status\": \"OK\", \"summary\": \"{}\"}}\n---END_STEP_OUTPUT---",
                data_json_obj, summary
            ),
        );
        ctx
    }

    #[test]
    fn nested_object_field_resolves() {
        // `{{steps.analyze.data.subtasks}}` should pretty-print the array
        // (the operator usually drops it inside a markdown code block).
        let ctx = ctx_with_envelope(
            "analyze",
            r#"{"subtasks": [{"id": 1, "title": "Setup CI"}], "test_strategy": "TDD"}"#,
            "Plan en 1 sous-tâche",
        );
        let rendered = ctx.render("{{steps.analyze.data.test_strategy}}").unwrap();
        assert_eq!(rendered, "TDD");
    }

    #[test]
    fn resolve_value_returns_typed_array_not_string() {
        // The api_body injection contract: a nested array stays a REAL array.
        let ctx = ctx_with_envelope(
            "review",
            r#"{"verdict": "APPROVE", "inlineComments": [{"path": "a.rs", "line": 4, "body": "x"}]}"#,
            "Reviewed",
        );
        let v = ctx.resolve_value("steps.review.data.inlineComments").unwrap();
        assert!(v.is_array(), "must stay a JSON array, got: {v}");
        assert_eq!(v[0]["line"], 4);
        // scalar field stays a (typed) scalar string
        assert_eq!(ctx.resolve_value("steps.review.data.verdict").unwrap(), "APPROVE");
    }

    #[test]
    fn resolve_value_anchors_on_data_json_alias_too() {
        // `data_json.<field>` must resolve identically to `data.<field>`
        // (the bug: only `data` was anchored, so `data_json.x` stayed literal).
        let ctx = ctx_with_envelope("review", r#"{"comments": [1, 2]}"#, "ok");
        let viz = ctx.render("{{steps.review.data_json.comments}}").unwrap();
        assert!(viz.contains('1') && viz.contains('2'), "data_json nested must render, got: {viz}");
        let typed = ctx.resolve_value("steps.review.data_json.comments").unwrap();
        assert!(typed.is_array());
    }

    #[test]
    fn resolve_value_whole_data_returns_full_payload() {
        let ctx = ctx_with_envelope("seed", r#"{"a": 1, "b": [2, 3]}"#, "ok");
        let v = ctx.resolve_value("steps.seed.data").unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"][1], 3);
    }

    #[test]
    fn nested_array_with_index_resolves() {
        let ctx = ctx_with_envelope(
            "analyze",
            r#"{"subtasks": [{"id": 1, "title": "Setup CI"}, {"id": 2, "title": "Wire DB"}]}"#,
            "Plan",
        );
        let rendered = ctx
            .render("{{steps.analyze.data.subtasks.1.title}}")
            .unwrap();
        assert_eq!(rendered, "Wire DB");
    }

    #[test]
    fn nested_object_pretty_prints() {
        // Final value being an object → pretty-printed JSON. Lets the
        // operator paste it into a markdown code block.
        let ctx = ctx_with_envelope(
            "analyze",
            r#"{"subtasks": [{"id": 1, "title": "Setup"}]}"#,
            "Plan",
        );
        let rendered = ctx.render("{{steps.analyze.data.subtasks}}").unwrap();
        assert!(rendered.contains("\"id\": 1"), "got: {}", rendered);
        assert!(rendered.contains("\"title\": \"Setup\""), "got: {}", rendered);
    }

    #[test]
    fn nested_exec_envelope_exit_code() {
        // Mirrors the Exec step contract: `{{steps.run_tests.data.exit_code}}`
        // is the canonical way for downstream agents to read the test result.
        let ctx = ctx_with_envelope(
            "run_tests",
            r#"{"exit_code": 0, "stdout": "all green", "stderr": "", "duration_ms": 1234}"#,
            "exit 0",
        );
        assert_eq!(
            ctx.render("{{steps.run_tests.data.exit_code}}").unwrap(),
            "0"
        );
        assert_eq!(
            ctx.render("{{steps.run_tests.data.stdout}}").unwrap(),
            "all green"
        );
    }

    #[test]
    fn nested_previous_step_path_resolves() {
        let ctx = ctx_with_envelope(
            "fetch",
            r#"{"key": "EW-42", "title": "Login bug"}"#,
            "Loaded",
        );
        assert_eq!(
            ctx.render("{{previous_step.data.key}}").unwrap(),
            "EW-42"
        );
    }

    #[test]
    fn missing_nested_field_leaves_placeholder() {
        // Broken refs stay visible so the operator notices — silent
        // empty-string substitution would mask bugs in gate recaps.
        let ctx = ctx_with_envelope("analyze", r#"{"a": 1}"#, "ok");
        let rendered = ctx.render("Result: {{steps.analyze.data.missing}}").unwrap();
        assert_eq!(rendered, "Result: {{steps.analyze.data.missing}}");
    }

    #[test]
    fn nested_path_on_non_data_prefix_is_left_alone() {
        // `state.X.foo` shouldn't accidentally match the nested resolver —
        // state values are flat strings, not JSON.
        let mut ctx = TemplateContext::new();
        ctx.values.insert("state.last_review".into(), "needs work".into());
        let rendered = ctx.render("{{state.last_review.foo}}").unwrap();
        assert_eq!(rendered, "{{state.last_review.foo}}");
    }

    #[test]
    fn flat_data_still_resolves_after_nested_addition() {
        // Regression guard: bare `{{steps.X.data}}` must still resolve via
        // the direct lookup path — the nested resolver is only a fallback.
        let ctx = ctx_with_envelope(
            "analyze",
            r#"{"subtasks": [{"id": 1}]}"#,
            "Plan",
        );
        let rendered = ctx.render("{{steps.analyze.data}}").unwrap();
        // `.data` is the unwrapped form (compact JSON since `data` is an
        // object, not a plain string).
        assert!(rendered.starts_with("{\"subtasks\""), "got: {}", rendered);
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
            on_timeout: None,
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
            batch_concurrent_limit: None,
            quick_api_id: None,
            notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
            api_method: None,
            api_path_params: None,
            api_query: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            gate_checkpoint_before: None,
            gate_auto_approve_after_secs: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            exec_setup_command: None,
            exec_setup_args: vec![],
            exec_stdin: None,
            quick_prompt_id: None,
            json_data_payload: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
        }
    }

    #[test]
    fn strategy2_only_accepts_the_last_toplevel_json_as_envelope() {
        // Codex acceptance matrix for the bounded strategy-2 fallback.

        // 1. Legacy bare envelope as the final JSON (Batch* shape) → OK.
        let legacy = r#"some preamble {"data": [1, 2], "status": "OK", "summary": "s"}"#;
        let env = extract_step_envelope(legacy).expect("legacy final envelope accepted");
        assert_eq!(env.status, "OK");

        // 2. Markers present but malformed block → None, even with a valid
        //    bare envelope elsewhere (markers are authoritative → repair).
        let marked_bad = r#"quoted example: {"data": "x", "status": "OK"}
---STEP_OUTPUT---
{"data": [1,], "status": "OK"
---END_STEP_OUTPUT---"#;
        assert!(extract_step_envelope(marked_bad).is_none(), "malformed marked block must not fall back");

        // 3. Earlier envelope-like JSON + LATER non-envelope JSON → None
        //    (the earlier one may be a quoted example from the reasoning).
        let example_then_other = r#"I will emit {"data": [], "status": "OK"} at the end. Result: {"count": 3}"#;
        assert!(extract_step_envelope(example_then_other).is_none(), "must not walk back past the last JSON");

        // 4. Earlier non-envelope JSON + FINAL valid envelope → OK.
        let other_then_envelope = r#"stats: {"count": 3} then {"data": {"items": [1]}, "status": "OK"}"#;
        let env = extract_step_envelope(other_then_envelope).expect("final envelope accepted");
        assert_eq!(env.status, "OK");

        // 5. Codex durcissement: braces inside JSON strings must not break
        //    the balancing (string-aware scanner).
        let brace_in_string = r#"done: {"data": "brace } inside string", "status": "OK"}"#;
        let env = extract_step_envelope(brace_in_string).expect("string-aware balancing");
        assert_eq!(env.data, "brace } inside string");
        let escaped_quote = r#"x: {"data": "quote \" then } brace", "status": "OK"}"#;
        assert!(extract_step_envelope(escaped_quote).is_some(), "escaped quotes handled");
    }

    #[test]
    fn validate_output_refs_check_existence_and_ordering_only() {
        use crate::models::StepOutputFormat::FreeText;
        // A typo'd `.output` used to pass BOTH validation layers and ship
        // the literal placeholder into the agent prompt.

        // Valid upstream .output on a FreeText producer → OK (raw-text contract).
        let ok = vec![step("a", "do things", FreeText), step("b", "resume: {{steps.a.output}}", FreeText)];
        assert!(validate_step_references(&ok).is_ok());
        // previous_step.output on a FreeText predecessor → OK too.
        let ok2 = vec![step("a", "do", FreeText), step("b", "resume: {{previous_step.output}}", FreeText)];
        assert!(validate_step_references(&ok2).is_ok());

        // Typo'd step name → save-time error.
        let typo = vec![step("a", "do", FreeText), step("b", "resume: {{steps.typo.output}}", FreeText)];
        let errs = validate_step_references(&typo).unwrap_err();
        assert!(errs[0].contains("aucune étape ne porte le nom"), "{errs:?}");

        // Self-reference → error.
        let selfref = vec![step("a", "loop: {{steps.a.output}}", FreeText)];
        assert!(validate_step_references(&selfref).is_err());

        // Forward reference → error.
        let fwd = vec![step("a", "peek: {{steps.b.output}}", FreeText), step("b", "do", FreeText)];
        let errs = validate_step_references(&fwd).unwrap_err();
        assert!(errs[0].contains("pas exécutée avant"), "{errs:?}");

        // previous_step.output on the FIRST step → error.
        let first = vec![step("a", "{{previous_step.output}}", FreeText)];
        assert!(validate_step_references(&first).is_err());

        // Codex blocker: a nested subpath on raw .output can never resolve.
        let nested = vec![step("a", "do", FreeText), step("b", "x: {{steps.a.output.foo}}", FreeText)];
        let errs = validate_step_references(&nested).unwrap_err();
        assert!(errs[0].contains("texte brut"), "{errs:?}");
        let nested_prev = vec![step("a", "do", FreeText), step("b", "x: {{previous_step.output.foo}}", FreeText)];
        let errs = validate_step_references(&nested_prev).unwrap_err();
        assert!(errs[0].contains("texte brut"), "{errs:?}");
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

    // ─── TypedSchema (0.7.0 Phase 2) ──────────────────────────────────────────

    #[test]
    fn typed_schema_instruction_includes_schema_text() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "score": { "type": "integer", "minimum": 0, "maximum": 10 }
            },
            "required": ["score"]
        });
        let instr = build_typed_schema_instruction(&schema);
        // The schema must appear verbatim (or pretty-printed) so the LLM
        // sees the contract — not just a hand-wavy description.
        assert!(instr.contains("score"));
        assert!(instr.contains("minimum"));
        assert!(instr.contains("---STEP_OUTPUT---"));
    }

    #[test]
    fn validator_accepts_conforming_object() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "score": { "type": "integer", "minimum": 0, "maximum": 10 },
                "status": { "type": "string", "enum": ["READY", "INCOMPLETE", "AMBIGUOUS"] }
            },
            "required": ["score", "status"]
        });
        let data = r#"{"score": 7, "status": "READY"}"#;
        assert!(validate_envelope_against_schema(data, &schema).is_ok());
    }

    #[test]
    fn validator_rejects_missing_required_property() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "score": { "type": "integer" } },
            "required": ["score"]
        });
        let data = r#"{"other": 7}"#;
        let err = validate_envelope_against_schema(data, &schema).unwrap_err();
        assert!(err.contains("missing required property 'score'"), "got: {}", err);
    }

    #[test]
    fn validator_rejects_out_of_range_number() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "score": { "type": "integer", "minimum": 0, "maximum": 10 } },
            "required": ["score"]
        });
        let data = r#"{"score": 42}"#;
        let err = validate_envelope_against_schema(data, &schema).unwrap_err();
        assert!(err.contains("> maximum"), "got: {}", err);
    }

    #[test]
    fn validator_rejects_value_not_in_enum() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "status": { "type": "string", "enum": ["READY", "DONE"] } },
            "required": ["status"]
        });
        let data = r#"{"status": "MAYBE"}"#;
        let err = validate_envelope_against_schema(data, &schema).unwrap_err();
        assert!(err.contains("not in allowed enum"), "got: {}", err);
    }

    #[test]
    fn validator_rejects_wrong_type() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "score": { "type": "integer" } },
            "required": ["score"]
        });
        let data = r#"{"score": "not a number"}"#;
        let err = validate_envelope_against_schema(data, &schema).unwrap_err();
        assert!(err.contains("expected type integer"), "got: {}", err);
    }

    #[test]
    fn validator_validates_array_items_recursively() {
        let schema = serde_json::json!({
            "type": "array",
            "items": { "type": "string", "minLength": 3 }
        });
        // Valid: all items >= 3 chars
        let ok = r#"["hello", "world"]"#;
        assert!(validate_envelope_against_schema(ok, &schema).is_ok());
        // Invalid: second item too short
        let bad = r#"["hello", "x"]"#;
        let err = validate_envelope_against_schema(bad, &schema).unwrap_err();
        assert!(err.contains("[1]"), "error path should point at item 1: {}", err);
        assert!(err.contains("minLength"), "got: {}", err);
    }

    #[test]
    fn validator_tolerates_unknown_properties() {
        // additionalProperties defaults to "true" in our subset — extra
        // fields are allowed, mirroring JSON Schema's default behavior.
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "score": { "type": "integer" } },
            "required": ["score"]
        });
        let data = r#"{"score": 7, "extra": "ignored"}"#;
        assert!(validate_envelope_against_schema(data, &schema).is_ok());
    }

    #[test]
    fn validator_handles_invalid_json_in_data() {
        let schema = serde_json::json!({});
        let bad = r#"{not json"#;
        let err = validate_envelope_against_schema(bad, &schema).unwrap_err();
        assert!(err.contains("not valid JSON"), "got: {}", err);
    }

    #[test]
    fn repair_prompt_for_typed_schema_includes_validation_error() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "score": { "type": "integer" } }
        });
        let fmt = crate::models::StepOutputFormat::TypedSchema { schema, on_invalid: Default::default() };
        let prompt = build_repair_prompt(
            "previous output here",
            &fmt,
            Some("$.score: expected type integer, got string"),
        );
        assert!(prompt.contains("schema validation failed"));
        assert!(prompt.contains("$.score"));
        assert!(prompt.contains("previous output here"));
    }

    #[test]
    fn repair_prompt_for_vanilla_structured_keeps_legacy_template() {
        let prompt = build_repair_prompt(
            "previous",
            &crate::models::StepOutputFormat::Structured,
            None,
        );
        // Legacy `REPAIR_PROMPT_TEMPLATE` content
        assert!(prompt.contains("Now rewrite ONLY the structured output block"));
        assert!(prompt.contains("previous"));
    }

    // ─── Artifacts (0.7.0 Phase 3) ────────────────────────────────────────────

    #[test]
    fn extract_artifacts_handles_single_block() {
        let output = "Reasoning prose.\n\
            ---ARTIFACT:plan---\n\
            # Plan\n\
            1. Step A\n\
            2. Step B\n\
            ---END_ARTIFACT---\n\
            More prose.";
        let arts = extract_artifacts(output);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts.get("plan").unwrap(), "# Plan\n1. Step A\n2. Step B");
    }

    #[test]
    fn extract_artifacts_handles_multiple_blocks() {
        let output = "\
            ---ARTIFACT:plan---\nplan body\n---END_ARTIFACT---\n\
            ---ARTIFACT:review---\nreview body\n---END_ARTIFACT---\n";
        let arts = extract_artifacts(output);
        assert_eq!(arts.len(), 2);
        assert_eq!(arts.get("plan").unwrap(), "plan body");
        assert_eq!(arts.get("review").unwrap(), "review body");
    }

    #[test]
    fn extract_artifacts_returns_empty_when_no_blocks() {
        let output = "Just prose, no markers.\n---STEP_OUTPUT---\n{}\n---END_STEP_OUTPUT---";
        let arts = extract_artifacts(output);
        assert!(arts.is_empty());
    }

    #[test]
    fn extract_artifacts_skips_unclosed_block() {
        let output = "---ARTIFACT:lonely---\nno end marker ever";
        let arts = extract_artifacts(output);
        assert!(arts.is_empty(), "unclosed block should be skipped, not panic");
    }

    #[test]
    fn extract_artifacts_skips_empty_name() {
        let output = "---ARTIFACT:---\nbody\n---END_ARTIFACT---";
        let arts = extract_artifacts(output);
        assert!(arts.is_empty(), "empty artifact name must be rejected");
    }

    #[test]
    fn extract_artifacts_preserves_leading_whitespace_strips_one_trailing_newline() {
        let output = "---ARTIFACT:md---\n  indented line\n  another\n---END_ARTIFACT---";
        let arts = extract_artifacts(output);
        // Leading whitespace preserved (Markdown matters); a single
        // trailing `\n` was stripped (LLM emission convention).
        assert_eq!(arts.get("md").unwrap(), "  indented line\n  another");
    }

    #[test]
    fn template_context_renders_artifact_after_step_output() {
        let mut ctx = TemplateContext::new();
        let step_output = "Did the work.\n\
            ---ARTIFACT:plan---\nstep 1\nstep 2\n---END_ARTIFACT---";
        ctx.set_step_output("plan_work", step_output);
        let rendered = ctx.render("Plan was: {{artifacts.plan}}").unwrap();
        assert_eq!(rendered, "Plan was: step 1\nstep 2");
    }

    #[test]
    fn template_context_seed_artifacts_pre_first_step() {
        // Use case: round 2+ of an Auto-Dev loop. The previous run
        // produced `review` already; the new run should see it before
        // the first step executes.
        let mut ctx = TemplateContext::new();
        let mut seed = ::std::collections::HashMap::new();
        seed.insert("review".to_string(), "feedback from round 1".to_string());
        ctx.seed_artifacts(&seed);
        let rendered = ctx.render("Review: {{artifacts.review}}").unwrap();
        assert_eq!(rendered, "Review: feedback from round 1");
    }

    #[test]
    fn missing_artifact_renders_when_seeded_via_workflow_declaration() {
        // The runner pre-seeds every declared artifact with "" before
        // step 1 executes, so referencing an artifact that hasn't been
        // produced yet renders empty. Test simulates that seeding here:
        // a TemplateContext with a manually-seeded empty entry.
        let mut ctx = TemplateContext::new();
        ctx.set("artifacts.review", "");
        let rendered = ctx.render("Review: '{{artifacts.review}}'").unwrap();
        assert_eq!(rendered, "Review: ''");
    }

    #[test]
    fn artifact_overwritten_on_later_step() {
        // implement → review → implement loop : the second `review`
        // emission should replace the first.
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("review_v1", "---ARTIFACT:review---\nv1\n---END_ARTIFACT---");
        ctx.set_step_output("review_v2", "---ARTIFACT:review---\nv2 with more\n---END_ARTIFACT---");
        let rendered = ctx.render("{{artifacts.review}}").unwrap();
        assert_eq!(rendered, "v2 with more");
    }

    // ─── State (0.7.0 Phase 6) ────────────────────────────────────────────────

    #[test]
    fn extract_state_handles_single_entry() {
        let output = "Reasoning prose.\n---STATE:retry_count=3---\nMore prose.";
        let st = extract_state(output);
        assert_eq!(st.len(), 1);
        assert_eq!(st.get("retry_count").unwrap(), "3");
    }

    #[test]
    fn extract_state_handles_multiple_entries() {
        let output = "---STATE:counter=5---\n\
            ---STATE:last_verdict=approved---\n\
            ---STATE:notes=long form value with spaces---";
        let st = extract_state(output);
        assert_eq!(st.len(), 3);
        assert_eq!(st.get("counter").unwrap(), "5");
        assert_eq!(st.get("last_verdict").unwrap(), "approved");
        assert_eq!(st.get("notes").unwrap(), "long form value with spaces");
    }

    #[test]
    fn extract_state_returns_empty_when_no_entries() {
        let output = "Just prose, no markers.\n---STEP_OUTPUT---\n{}\n---END_STEP_OUTPUT---";
        let st = extract_state(output);
        assert!(st.is_empty());
    }

    #[test]
    fn extract_state_skips_malformed_no_equals() {
        let output = "---STATE:lonely---";
        let st = extract_state(output);
        assert!(st.is_empty(), "entry without '=' must be skipped");
    }

    #[test]
    fn extract_state_skips_empty_key() {
        let output = "---STATE:=value---";
        let st = extract_state(output);
        assert!(st.is_empty(), "empty key must be rejected");
    }

    #[test]
    fn extract_state_keeps_empty_value() {
        // Clearing a counter to "" is legitimate — distinguish from
        // "key never set". The map gets an entry with an empty string.
        let output = "---STATE:notes=---";
        let st = extract_state(output);
        assert_eq!(st.len(), 1);
        assert_eq!(st.get("notes").unwrap(), "");
    }

    #[test]
    fn extract_state_last_write_wins_within_one_response() {
        // Two writes to the same key in one agent response: standard
        // hash-map semantics — second overwrites first. Documented
        // behaviour, not undefined.
        let output = "---STATE:counter=1---\n---STATE:counter=2---";
        let st = extract_state(output);
        assert_eq!(st.get("counter").unwrap(), "2");
    }

    #[test]
    fn extract_state_does_not_span_multiple_lines() {
        // Defensive: a `---STATE:` without closing `---` on the same
        // line is malformed and skipped. Without this rule, a long
        // narrative paragraph would silently capture a state value
        // ending wherever the first random `---` happens to land.
        let output = "---STATE:counter=3\nsome unrelated content\n---END_THING---";
        let st = extract_state(output);
        assert!(st.is_empty(), "open-ended STATE without same-line close must be skipped");
    }

    #[test]
    fn template_context_renders_state_after_step_output() {
        let mut ctx = TemplateContext::new();
        let step_output = "Did the work.\n---STATE:retry_count=1---";
        ctx.set_step_output("review", step_output);
        let rendered = ctx.render("Retry #{{state.retry_count}}").unwrap();
        assert_eq!(rendered, "Retry #1");
    }

    #[test]
    fn template_context_seed_state_pre_first_step() {
        // Use case: resume after Gate pause / daemon restart. The run
        // row carries `state.counter=2`; the next step's prompt must
        // see it BEFORE the step executes.
        let mut ctx = TemplateContext::new();
        let mut seed = ::std::collections::HashMap::new();
        seed.insert("counter".to_string(), "2".to_string());
        ctx.seed_state(&seed);
        let rendered = ctx.render("Iter {{state.counter}}").unwrap();
        assert_eq!(rendered, "Iter 2");
    }

    #[test]
    fn state_overwritten_on_later_step() {
        // implement → review → implement: the second `last_verdict`
        // emission overwrites the first in the template ctx.
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("r1", "---STATE:last_verdict=needs_changes---");
        ctx.set_step_output("r2", "---STATE:last_verdict=approved---");
        let rendered = ctx.render("{{state.last_verdict}}").unwrap();
        assert_eq!(rendered, "approved");
    }

    // ════════════════════════════════════════════════════════════════════
    // 0.8.5 — Cross-step output transmission matrix
    //
    // CRITICAL plumbing layer. Every step type publishes its output as a
    // string to `set_step_output(name, output)`. Downstream steps read
    // it via templates like `{{steps.X.data}}`, `{{steps.X.summary}}`,
    // `{{steps.X.status}}`, `{{steps.X.data.<nested>}}`. The pin below
    // captures, for EACH step type, a representative output sample
    // (mirroring exactly what each step implementation emits today) and
    // verifies the envelope extraction + the four canonical access
    // patterns. Any regression in a step's output shape — or in the
    // envelope extractor — fails one localised test instead of
    // silently breaking every workflow that consumes it.
    //
    // Source-of-truth samples (kept verbatim with the producing impl):
    //   - JsonData        → `json_data_step.rs::execute_json_data_step`
    //   - ApiCall         → `api_call_executor.rs::execute_api_call_step_core`
    //   - Notify          → `notify_step.rs::execute_notify_step`
    //   - Exec            → `exec_step.rs::execute_exec_step`
    //   - Agent (Struct.) → runner.rs emits `---STEP_OUTPUT---` envelope
    //   - Agent (FreeText)→ raw text, no envelope (consumers can only
    //                       read `.output`)
    //   - Gate            → raw rendered gate_message, no envelope
    //   - BatchApiCall    → `build_structured_output` (strategy-2 JSON)
    //   - BatchQuickPrompt→ `build_structured_output` (strategy-2 JSON)
    //
    // If you add a new step type, add a sample + assertion block below.
    // ════════════════════════════════════════════════════════════════════
    mod cross_step_transmission {
        use super::*;

        // ── JsonData ────────────────────────────────────────────────────
        fn sample_json_data_output() -> String {
            // 0.8.5 — JsonData now emits the canonical Kronn envelope
            // (markers + signal) via `format_step_output_simple`. The
            // legacy bare-JSON shape is covered by the dedicated
            // backward-compat test further down.
            crate::workflows::step_output_format::format_step_output_simple(
                serde_json::json!({ "key": "DEMO-1", "body": "Refactor login button" }),
                "OK",
                "JSON data (1 object, 2 field(s))",
            )
        }

        #[test]
        fn json_data_exposes_data_summary_status_and_nested_fields() {
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("fetch", &sample_json_data_output());

            // Top-level envelope fields land in ctx.
            assert_eq!(
                ctx.render("{{steps.fetch.status}}").unwrap(),
                "OK",
            );
            assert_eq!(
                ctx.render("{{steps.fetch.summary}}").unwrap(),
                "JSON data (1 object, 2 field(s))",
            );
            // `data` (object) renders as compact JSON via stringify path.
            assert!(ctx.render("{{steps.fetch.data}}").unwrap().contains("DEMO-1"));
            // Nested traversal works (resolve_nested_path via data_json).
            assert_eq!(
                ctx.render("{{steps.fetch.data.key}}").unwrap(),
                "DEMO-1",
            );
            assert_eq!(
                ctx.render("{{steps.fetch.data.body}}").unwrap(),
                "Refactor login button",
            );
            // `previous_step.*` aliases mirror the named ones.
            assert_eq!(
                ctx.render("{{previous_step.status}}").unwrap(),
                "OK",
            );
        }

        // ── ApiCall (Jira-shaped data) ──────────────────────────────────
        fn sample_apicall_jira_output() -> String {
            // 0.8.5 — ApiCall now goes through `format_step_output` like
            // every other step type. The Jira-shaped data inside is the
            // realistic AutoCode `fetch_issue` payload — what consumers
            // see when navigating `{{steps.fetch_issue.data.<path>}}`.
            crate::workflows::step_output_format::format_step_output(
                serde_json::json!({
                    "key": "EW-7247",
                    "fields": {
                        "summary": "Africanews → Euronews migration",
                        "description": { "content": [], "type": "doc" },
                    },
                    "renderedFields": {
                        "description": "<p>Port Africanews onto Euronews…</p>",
                    },
                }),
                "OK",
                "GET /rest/api/3/issue/EW-7247 → 1 item",
                None,
                &["OK"],
            )
        }

        #[test]
        fn apicall_exposes_nested_path_into_real_jira_payload() {
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("fetch_issue", &sample_apicall_jira_output());

            // The fix that unblocked AutoCode EW-7247 pinned: data.key reachable.
            assert_eq!(
                ctx.render("{{steps.fetch_issue.data.key}}").unwrap(),
                "EW-7247",
            );
            // Deep nested traversal — `data.fields.summary`.
            assert_eq!(
                ctx.render("{{steps.fetch_issue.data.fields.summary}}").unwrap(),
                "Africanews → Euronews migration",
            );
            // `data.renderedFields.description` (Jira HTML body).
            assert_eq!(
                ctx.render("{{steps.fetch_issue.data.renderedFields.description}}").unwrap(),
                "<p>Port Africanews onto Euronews…</p>",
            );
            // Bare `.data` returns compact JSON (downstream agent can navigate).
            let bare = ctx.render("{{steps.fetch_issue.data}}").unwrap();
            assert!(bare.contains("EW-7247"));
            assert!(bare.contains("Africanews"));
        }

        // ── Notify ──────────────────────────────────────────────────────
        fn sample_notify_output() -> String {
            // 0.8.5 — Notify now emits the canonical envelope + signal.
            crate::workflows::step_output_format::format_step_output(
                serde_json::json!({
                    "http_status": 200,
                    "response_excerpt": "{\"ok\": true}",
                    "url": "https://hooks.example/abc",
                    "method": "POST",
                }),
                "OK",
                "POST https://hooks.example/abc → 200",
                None,
                &["OK"],
            )
        }

        #[test]
        fn notify_exposes_http_metadata_to_downstream_steps() {
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("alert", &sample_notify_output());
            assert_eq!(
                ctx.render("{{steps.alert.status}}").unwrap(),
                "OK",
            );
            assert_eq!(
                ctx.render("{{steps.alert.data.http_status}}").unwrap(),
                "200",
            );
            assert_eq!(
                ctx.render("{{steps.alert.data.url}}").unwrap(),
                "https://hooks.example/abc",
            );
        }

        // ── Exec ────────────────────────────────────────────────────────
        fn sample_exec_output(exit_code: i32, stdout: &str) -> String {
            // Mirrors exec_step.rs:324-334 — Strategy-1 (`---STEP_OUTPUT---`)
            // wrapped JSON envelope, plus trailing `[SIGNAL: …]` lines.
            let env = serde_json::json!({
                "data": {
                    "exit_code": exit_code,
                    "stdout_excerpt": stdout,
                    "stderr_excerpt": "",
                    "killed": false,
                },
                "status": if exit_code == 0 { "OK" } else { "ERROR" },
                "summary": format!("exec exit {}", exit_code),
            });
            let signal_generic = if exit_code == 0 { "[SIGNAL: OK]" } else { "[SIGNAL: ERROR]" };
            format!(
                "summary line\n\n---STEP_OUTPUT---\n{}\n---END_STEP_OUTPUT---\n{}\n[SIGNAL: exit_{}]",
                env, signal_generic, exit_code,
            )
        }

        #[test]
        fn exec_exposes_exit_code_and_stdout_excerpt() {
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("tests", &sample_exec_output(0, "test passed"));
            assert_eq!(
                ctx.render("{{steps.tests.status}}").unwrap(),
                "OK",
            );
            assert_eq!(
                ctx.render("{{steps.tests.data.exit_code}}").unwrap(),
                "0",
            );
            assert_eq!(
                ctx.render("{{steps.tests.data.stdout_excerpt}}").unwrap(),
                "test passed",
            );
        }

        #[test]
        fn exec_failure_envelope_still_extracts_data() {
            // Regression: even on Failed status, set_step_output must
            // populate ctx so downstream conditional steps can branch.
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("tests", &sample_exec_output(2, "compile error"));
            assert_eq!(
                ctx.render("{{steps.tests.status}}").unwrap(),
                "ERROR",
            );
            assert_eq!(
                ctx.render("{{steps.tests.data.exit_code}}").unwrap(),
                "2",
            );
        }

        // ── Agent (Structured) ──────────────────────────────────────────
        fn sample_agent_structured_output(data: serde_json::Value, summary: &str) -> String {
            // Mirrors what the runner expects an Agent in `output_format:
            // Structured` to emit — a `---STEP_OUTPUT---` block at the
            // end of the response. Strategy-1 parseable.
            format!(
                "Here is my analysis.\n\n---STEP_OUTPUT---\n{}\n---END_STEP_OUTPUT---",
                serde_json::json!({ "data": data, "status": "OK", "summary": summary }),
            )
        }

        #[test]
        fn agent_structured_exposes_typed_manifest_fields() {
            // The AutoCode triage step emits a typed manifest. Pin the
            // contract that `implement` can read each branch array.
            let manifest = serde_json::json!({
                "clear": ["sub-1", "sub-2"],
                "decided": [{ "id": "d1", "chosen": "A" }],
                "mocked": [],
                "blocked": [{ "id": "b1", "needed_from": "PM" }],
                "files_touched": ["src/foo.rs", "src/bar.rs"],
            });
            let mut ctx = TemplateContext::new();
            ctx.set_step_output(
                "triage",
                &sample_agent_structured_output(manifest, "5 entries triaged"),
            );

            // Nested arrays render as pretty JSON (operator-friendly).
            let clear = ctx.render("{{steps.triage.data.clear}}").unwrap();
            assert!(clear.contains("sub-1"));
            assert!(clear.contains("sub-2"));
            // Nested object inside an array.
            let decided = ctx.render("{{steps.triage.data.decided}}").unwrap();
            assert!(decided.contains("\"id\": \"d1\""));
            // `summary` and `status` reachable in the same envelope.
            assert_eq!(
                ctx.render("{{steps.triage.summary}}").unwrap(),
                "5 entries triaged",
            );
            assert_eq!(
                ctx.render("{{steps.triage.status}}").unwrap(),
                "OK",
            );
            // Empty array still renders.
            assert_eq!(
                ctx.render("{{steps.triage.data.mocked}}").unwrap(),
                "[]",
            );
        }

        // ── Agent (FreeText) ────────────────────────────────────────────
        // FreeText output has NO envelope. Downstream consumers can only
        // read `{{steps.X.output}}` — `.data` / `.summary` / `.status`
        // remain unresolved (intentional: validate_step_references catches
        // mismatches at save time, find_unresolved_critical_refs at run
        // time).
        #[test]
        fn agent_freetext_exposes_only_output_no_data_envelope() {
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("brainstorm", "Three ideas:\n- A\n- B\n- C");

            // `.output` works.
            let raw = ctx.render("{{steps.brainstorm.output}}").unwrap();
            assert!(raw.contains("Three ideas"));
            // `.data` does NOT resolve → placeholder kept literal.
            assert_eq!(
                ctx.render("{{steps.brainstorm.data}}").unwrap(),
                "{{steps.brainstorm.data}}",
            );
        }

        // ── Gate ────────────────────────────────────────────────────────
        // Gate steps emit only the rendered `gate_message` as `output` —
        // no envelope. Downstream steps that need a structured "decision"
        // should read from a sibling Agent step, not from Gate.
        #[test]
        fn gate_exposes_only_output_no_envelope() {
            let mut ctx = TemplateContext::new();
            let gate_msg = "Review the triage manifest:\n\nApprove / Reject?";
            ctx.set_step_output("review_triage", gate_msg);

            assert_eq!(
                ctx.render("{{steps.review_triage.output}}").unwrap(),
                gate_msg,
            );
            // `.data` stays unresolved — by design.
            assert_eq!(
                ctx.render("{{steps.review_triage.data}}").unwrap(),
                "{{steps.review_triage.data}}",
            );
        }

        // ── BatchApiCall + BatchQuickPrompt ─────────────────────────────
        fn sample_batch_output() -> String {
            // 0.8.5 — batch fan-outs now emit the canonical envelope
            // with the status name doubling as the SIGNAL value so
            // `on_result.contains` rules can branch on PARTIAL / ERROR.
            crate::workflows::step_output_format::format_step_output(
                serde_json::json!({
                    "batch_run_id": "br-1",
                    "total": 3,
                    "completed": 3,
                    "failed": 0,
                    "discussion_ids": ["d1", "d2", "d3"],
                }),
                "OK",
                "Batch 3/3 completed",
                None,
                &["OK"],
            )
        }

        #[test]
        fn batch_exposes_counters_and_discussion_ids() {
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("triage_batch", &sample_batch_output());

            assert_eq!(
                ctx.render("{{steps.triage_batch.status}}").unwrap(),
                "OK",
            );
            assert_eq!(
                ctx.render("{{steps.triage_batch.data.completed}}").unwrap(),
                "3",
            );
            assert_eq!(
                ctx.render("{{steps.triage_batch.data.failed}}").unwrap(),
                "0",
            );
            // Nested array index access.
            assert_eq!(
                ctx.render("{{steps.triage_batch.data.discussion_ids.0}}").unwrap(),
                "d1",
            );
        }

        // ── Chained source → consumer pairs ─────────────────────────────
        //
        // The matrix above pins each source in isolation. These tests
        // pin canonical SOURCE → CONSUMER pairs in the order they're
        // composed in real workflows — guards against a regression that
        // breaks the "obvious" wiring (ApiCall→Agent, JsonData→Batch, …)
        // even when individual envelope extractions still pass.

        #[test]
        fn pair_jsondata_to_agent_data_passes_through() {
            // JsonData fixture → Agent reads `{{steps.fetch.data.body}}`.
            // This is the contract of the feasibility-autopilot preset
            // in its original (JsonData) form.
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("fetch", &sample_json_data_output());
            let agent_prompt = "Triage this: {{steps.fetch.data.body}}";
            let rendered = ctx.render(agent_prompt).unwrap();
            assert_eq!(rendered, "Triage this: Refactor login button");
        }

        #[test]
        fn pair_apicall_to_agent_full_data_passes_through() {
            // ApiCall (Jira) → Agent reads `{{steps.fetch_issue.data}}` —
            // the AutoCode EW-7247 path after the 0.8.5 prompt fix.
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("fetch_issue", &sample_apicall_jira_output());
            let agent_prompt = "Triage: {{steps.fetch_issue.data}}";
            let rendered = ctx.render(agent_prompt).unwrap();
            assert!(rendered.contains("EW-7247"));
            assert!(rendered.contains("Africanews"));
        }

        #[test]
        fn pair_agent_to_exec_signal_only() {
            // Agent emits the typed manifest; downstream Exec doesn't
            // read structured data (Exec is a shell command) but its
            // `command_template` may reference `{{steps.triage.summary}}`
            // for logging / branching.
            let mut ctx = TemplateContext::new();
            ctx.set_step_output(
                "triage",
                &sample_agent_structured_output(
                    serde_json::json!({ "files_touched": ["src/x.rs"] }),
                    "1 file",
                ),
            );
            let exec_log_template = "echo 'triage said: {{steps.triage.summary}}'";
            assert_eq!(
                ctx.render(exec_log_template).unwrap(),
                "echo 'triage said: 1 file'",
            );
        }

        #[test]
        fn pair_exec_to_agent_exit_code_branching() {
            // Real-world: run_tests Exec → pr_draft Agent reads exit code
            // to decide tone of PR description. Pin that exit_code is
            // reachable as a string.
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("run_tests", &sample_exec_output(0, "ok"));
            let pr_template =
                "Tests: {{steps.run_tests.status}} (exit {{steps.run_tests.data.exit_code}})";
            assert_eq!(
                ctx.render(pr_template).unwrap(),
                "Tests: OK (exit 0)",
            );
        }

        #[test]
        fn pair_apicall_to_notify_propagates_payload() {
            // ApiCall (fetch tickets) → Notify (Slack-style webhook) uses
            // `{{steps.fetch.data}}` as the JSON body. The data string is
            // available verbatim for inline substitution.
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("fetch", &sample_apicall_jira_output());
            let notify_body = "{{steps.fetch.summary}} — see {{steps.fetch.data.key}}";
            assert_eq!(
                ctx.render(notify_body).unwrap(),
                "GET /rest/api/3/issue/EW-7247 → 1 item — see EW-7247",
            );
        }

        #[test]
        fn pair_gate_to_following_step_only_output_visible() {
            // Gate's output is the gate_message verbatim. A step right
            // after a Gate (e.g. an Agent that reads "what did the gate
            // say?") can only consume `.output`, not `.data`. This
            // failure mode is silent (placeholder stays literal) — the
            // save-time `validate_step_references` is what catches the
            // mistake in the wizard.
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("review", "Approve / Reject?");
            assert_eq!(
                ctx.render("{{steps.review.output}}").unwrap(),
                "Approve / Reject?",
            );
            assert_eq!(
                ctx.render("{{steps.review.data}}").unwrap(),
                "{{steps.review.data}}",
            );
        }

        #[test]
        fn pair_batch_to_agent_aggregate_handover() {
            // BatchQuickPrompt / BatchApiCall → Agent summarisation step
            // reads the per-discussion ids + completion counters.
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("fan_out", &sample_batch_output());
            let summary_prompt =
                "Out of {{steps.fan_out.data.total}}, {{steps.fan_out.data.completed}} succeeded.";
            assert_eq!(
                ctx.render(summary_prompt).unwrap(),
                "Out of 3, 3 succeeded.",
            );
        }

        // ── Backwards-compat: extractor still reads pre-0.8.5 bare JSON ─
        //
        // Runs persisted in DB before the canonical-envelope refactor
        // hold the OLD shape (bare `{data,status,summary}` JSON + an
        // optional trailing `[SIGNAL: ...]`). The extractor must keep
        // parsing those so legacy run logs render correctly in the UI
        // and old discussion threads still resolve `{{steps.X.data}}`.
        // Loss of this fallback would be a silent data-loss regression
        // on existing customer DBs.
        #[test]
        fn legacy_bare_json_envelope_still_extracts_correctly() {
            let legacy = "{\"data\":{\"key\":\"EW-1\",\"body\":\"hi\"},\"status\":\"OK\",\"summary\":\"legacy run\"}\n[SIGNAL: OK]";
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("legacy", legacy);
            assert_eq!(ctx.render("{{steps.legacy.status}}").unwrap(), "OK");
            assert_eq!(ctx.render("{{steps.legacy.summary}}").unwrap(), "legacy run");
            assert_eq!(ctx.render("{{steps.legacy.data.key}}").unwrap(), "EW-1");
            assert_eq!(ctx.render("{{steps.legacy.data.body}}").unwrap(), "hi");
        }

        // ── Catch-all: every structured step type populates the four
        //    canonical keys (.output, .data, .summary, .status). Failure
        //    here means a step's output_format isn't strategy-1/2
        //    parseable — the single most damaging regression class
        //    because it silently breaks every downstream `{{steps.X.…}}`.
        #[test]
        fn canonical_keys_present_for_every_envelope_producing_step_type() {
            // Each tuple = (kind label, sample output string emitter).
            let cases: Vec<(&str, String)> = vec![
                ("JsonData", sample_json_data_output()),
                ("ApiCall", sample_apicall_jira_output()),
                ("Notify", sample_notify_output()),
                ("Exec(success)", sample_exec_output(0, "ok")),
                ("Exec(failure)", sample_exec_output(1, "bad")),
                (
                    "Agent(Structured)",
                    sample_agent_structured_output(
                        serde_json::json!({ "x": 1 }),
                        "agent summary",
                    ),
                ),
                ("BatchApiCall/BatchQuickPrompt", sample_batch_output()),
            ];

            for (label, output) in cases {
                let mut ctx = TemplateContext::new();
                ctx.set_step_output("src", &output);

                // All four canonical keys must resolve to non-placeholder.
                for field in ["output", "data", "summary", "status"] {
                    let placeholder = format!("{{{{steps.src.{}}}}}", field);
                    let rendered = ctx.render(&placeholder).unwrap();
                    assert_ne!(
                        rendered, placeholder,
                        "{label}: `{field}` did not resolve (envelope extraction broken)",
                    );
                    assert!(
                        !rendered.is_empty(),
                        "{label}: `{field}` resolved to empty (envelope shape regressed)",
                    );
                }
            }
        }
    }
}
