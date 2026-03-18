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
    pub fn set_step_output(&mut self, step_name: &str, output: &str) {
        self.values.insert(format!("steps.{}.output", step_name), output.into());
        // Also set `previous_step.output` (overwritten each step)
        self.values.insert("previous_step.output".into(), output.into());
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
}
