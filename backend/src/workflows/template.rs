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
    /// For structured outputs, also sets `.data` and `.summary` variables.
    pub fn set_step_output(&mut self, step_name: &str, output: &str) {
        self.values.insert(format!("steps.{}.output", step_name), output.into());
        self.values.insert("previous_step.output".into(), output.into());

        // Try to extract structured envelope
        if let Some(envelope) = extract_step_envelope(output) {
            self.values.insert(format!("steps.{}.data", step_name), envelope.data.clone());
            self.values.insert(format!("steps.{}.summary", step_name), envelope.summary.clone());
            self.values.insert(format!("steps.{}.status", step_name), envelope.status.clone());
            self.values.insert("previous_step.data".into(), envelope.data);
            self.values.insert("previous_step.summary".into(), envelope.summary);
            self.values.insert("previous_step.status".into(), envelope.status);
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

// ── Structured output extraction ────────────────────────────────────────────

/// Extracted envelope from a step's output.
#[derive(Debug, Clone)]
pub struct StepEnvelope {
    pub data: String,
    pub status: String,
    pub summary: String,
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

    // Serialize data back to string — if it's a string, use it directly
    let data_str = if let Some(s) = data.as_str() {
        s.to_string()
    } else {
        serde_json::to_string(data).unwrap_or_default()
    };

    Some(StepEnvelope {
        data: data_str,
        status: status.to_string(),
        summary: summary.to_string(),
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
