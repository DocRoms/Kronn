//! Keep literal credentials out of exported JSON bundles.
//!
//! Kronn injects authentication from saved connections, but a header, query
//! value or command argument can still hold a token typed by hand. An export
//! file is meant to travel, so such values are replaced before serialisation
//! and the bundle lists which fields were masked, never their content.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::{QuickApi, QuickExec, Workflow};

/// Written in place of a masked value; readable enough to say what to do.
pub const REDACTED: &str = "[redacted by Kronn export: set it again or use a connection]";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RedactedField {
    /// `quick_api`, `quick_exec` or `workflow_step`.
    pub kind: String,
    pub resource_id: String,
    pub name: String,
    /// Dotted location, e.g. `api_headers.Authorization` or `args.3`.
    pub field: String,
}

const SECRET_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "x-api-key",
    "api-key",
    "x-auth-token",
];

/// Words that make a parameter name a secret on their own (`token`, `db_password`).
const SECRET_WORDS: &[&str] = &[
    "token",
    "password",
    "passwd",
    "pwd",
    "passphrase",
    "secret",
    "apikey",
    "bearer",
    "credential",
    "credentials",
    "auth",
];

/// Word pairs that name a key (`api_key`, `privateKey`), unlike `sort_key`.
const SECRET_KEY_PREFIXES: &[&str] = &[
    "api",
    "access",
    "private",
    "secret",
    "signing",
    "encryption",
    "client",
];

/// Whether a parameter, header or flag name designates a credential. Names are
/// compared word by word so `max_tokens` or `tokenizer` stay ordinary.
fn secret_name(name: &str) -> bool {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for ch in name.trim_start_matches('-').chars() {
        if !ch.is_ascii_alphanumeric() {
            words.extend((!current.is_empty()).then(|| std::mem::take(&mut current)));
            previous_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && previous_lower {
            words.push(std::mem::take(&mut current));
        }
        previous_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        current.push(ch.to_ascii_lowercase());
    }
    words.extend((!current.is_empty()).then_some(current));
    words
        .iter()
        .any(|word| SECRET_WORDS.contains(&word.as_str()))
        || words
            .windows(2)
            .any(|pair| pair[1] == "key" && SECRET_KEY_PREFIXES.contains(&pair[0].as_str()))
}

/// The value without its `{{…}}` placeholders: only this part can hold a
/// literal secret, since placeholders are resolved at run time.
fn literal_part(value: &str) -> String {
    let mut literal = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("{{") {
        literal.push_str(&rest[..start]);
        match rest[start..].find("}}") {
            Some(end) => rest = &rest[start + end + 2..],
            None => {
                rest = &rest[start..];
                break;
            }
        }
    }
    literal.push_str(rest);
    literal.trim().to_owned()
}

/// An auth scheme keyword alone (`Bearer {{token}}`) carries no secret.
fn only_scheme(literal: &str) -> bool {
    literal.split_whitespace().all(|word| {
        matches!(
            word.to_ascii_lowercase().as_str(),
            "bearer" | "basic" | "token" | "apikey" | "digest"
        )
    })
}

fn suspicious(context: &str) -> bool {
    crate::core::redact::looks_like_secret(context)
        || crate::core::redact::redact_for_audit_artifact(context).1 > 0
}

struct Scope<'a> {
    kind: &'static str,
    resource_id: &'a str,
    name: &'a str,
    found: &'a mut Vec<RedactedField>,
}

impl Scope<'_> {
    fn mask(&mut self, value: &mut String, field: String) {
        *value = REDACTED.to_owned();
        self.found.push(RedactedField {
            kind: self.kind.to_owned(),
            resource_id: self.resource_id.to_owned(),
            name: self.name.to_owned(),
            field,
        });
    }

    fn headers(&mut self, prefix: &str, headers: &mut Option<HashMap<String, String>>) {
        for (key, value) in headers.iter_mut().flatten() {
            let literal = literal_part(value);
            if literal.is_empty() {
                continue;
            }
            if ((SECRET_HEADERS.contains(&key.to_ascii_lowercase().as_str()) || secret_name(key))
                && !only_scheme(&literal))
                || suspicious(&format!("{key}: {literal}"))
            {
                self.mask(value, format!("{prefix}.{key}"));
            }
        }
    }

    fn pairs(&mut self, prefix: &str, pairs: &mut Option<HashMap<String, String>>) {
        for (key, value) in pairs.iter_mut().flatten() {
            let literal = literal_part(value);
            if literal.is_empty() {
                continue;
            }
            if (secret_name(key) && !only_scheme(&literal))
                || suspicious(&format!("{key}={literal}"))
                || suspicious(&literal)
            {
                self.mask(value, format!("{prefix}.{key}"));
            }
        }
    }

    fn json(&mut self, path: String, key: Option<&str>, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => {
                let literal = literal_part(text);
                if literal.is_empty() {
                    return;
                }
                let keyed = key.is_some_and(|key| {
                    (secret_name(key) && !only_scheme(&literal))
                        || suspicious(&format!("\"{key}\": \"{literal}\""))
                });
                if keyed || suspicious(&literal) {
                    self.mask(text, path);
                }
            }
            serde_json::Value::Array(items) => {
                for (index, item) in items.iter_mut().enumerate() {
                    self.json(format!("{path}.{index}"), None, item);
                }
            }
            serde_json::Value::Object(map) => {
                for (child, item) in map.iter_mut() {
                    self.json(format!("{path}.{child}"), Some(child), item);
                }
            }
            _ => {}
        }
    }
}

pub fn redact_quick_api(api: &mut QuickApi, found: &mut Vec<RedactedField>) {
    let (id, name) = (api.id.clone(), api.name.clone());
    let mut scope = Scope {
        kind: "quick_api",
        resource_id: &id,
        name: &name,
        found,
    };
    scope.headers("api_headers", &mut api.api_headers);
    scope.pairs("api_query", &mut api.api_query);
    scope.pairs("api_path_params", &mut api.api_path_params);
    if let Some(body) = api.api_body.as_mut() {
        scope.json("api_body".into(), None, body);
    }
}

pub fn redact_quick_exec(exec: &mut QuickExec, found: &mut Vec<RedactedField>) {
    let (id, name) = (exec.id.clone(), exec.name.clone());
    let mut scope = Scope {
        kind: "quick_exec",
        resource_id: &id,
        name: &name,
        found,
    };
    if suspicious(&literal_part(&exec.command)) {
        scope.mask(&mut exec.command, "command".into());
    }
    let mut after_secret_flag = false;
    for (index, arg) in exec.args.iter_mut().enumerate() {
        // A value may start with a single dash; only a long option ends the value.
        let flag_value = after_secret_flag && !arg.starts_with("--");
        // `--token=value` and `TOKEN=value` carry the value in the same argument.
        let (name, value) = match arg.split_once('=') {
            Some((name, value)) if !name.is_empty() && !name.contains(char::is_whitespace) => {
                (name, Some(value))
            }
            _ => (arg.as_str(), None),
        };
        after_secret_flag = arg.starts_with('-') && value.is_none() && secret_name(name);
        let literal = literal_part(value.unwrap_or(arg));
        if literal.is_empty() {
            continue;
        }
        let named = value.is_some() && secret_name(name) && !only_scheme(&literal);
        if flag_value || named || suspicious(&literal_part(arg)) {
            scope.mask(arg, format!("args.{index}"));
        }
    }
}

pub fn redact_workflow(workflow: &mut Workflow, found: &mut Vec<RedactedField>) {
    let (id, name) = (workflow.id.clone(), workflow.name.clone());
    redact_steps(
        &id,
        &name,
        workflow
            .steps
            .iter_mut()
            .chain(workflow.on_failure.iter_mut()),
        found,
    );
}

fn redact_steps<'a>(
    workflow_id: &str,
    workflow_name: &str,
    steps: impl Iterator<Item = &'a mut crate::models::WorkflowStep>,
    found: &mut Vec<RedactedField>,
) {
    for step in steps {
        let name = format!("{workflow_name} › {}", step.name);
        let mut scope = Scope {
            kind: "workflow_step",
            resource_id: workflow_id,
            name: &name,
            found,
        };
        scope.headers("api_headers", &mut step.api_headers);
        scope.pairs("api_query", &mut step.api_query);
        scope.pairs("api_path_params", &mut step.api_path_params);
        if let Some(body) = step.api_body.as_mut() {
            scope.json("api_body".into(), None, body);
        }
    }
}

#[cfg(test)]
#[path = "export_secrets_test.rs"]
mod tests;
