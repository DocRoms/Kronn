//! Which files an ACP agent may read: versioned environment templates yes, real
//! secret files no.
//!
//! A `.env.dist` (like `.env.example`, `.env.sample`, `.env.template`) is a
//! template committed on purpose: variable names and placeholder values, no
//! secret. A `.env`, `.env.local` or a key file is the real thing. Both sit under
//! the same `*.env.*` glob, so a runtime that guards environment files by glob
//! alone refuses the two alike (OpenCode does: `read` on `*.env.*` is `ask`).
//!
//! Two places apply this rule, for two kinds of runtime:
//! - the broker ([`super::permission_broker`]) for a runtime that reports the
//!   path of a read it asks about;
//! - [`opencode_config_content`], handed to OpenCode at spawn, because OpenCode
//!   asks about a `read` WITHOUT its path — the broker sees "a read" and nothing
//!   else, so the decision has to be made inside OpenCode, by pattern.

use std::path::{Component, Path};

/// Suffixes that make an environment file a template.
pub(super) const TEMPLATE_SUFFIXES: [&str; 4] = ["dist", "example", "sample", "template"];

/// What an environment file name says about its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvFile {
    /// A versioned template: names and placeholders, no secret.
    Template,
    /// The real file, or anything Kronn cannot prove is a template.
    Secret,
}

/// Classify a bare file name. `None` when it is not an environment file at all.
pub fn classify_env_file(name: &str) -> Option<EnvFile> {
    let name = name.to_ascii_lowercase();
    // `.envrc` (direnv) and `.env.*` both start with `.env`; `prod.env` only ends
    // with it. All of them hold variables.
    if !(name.starts_with(".env") || name.ends_with(".env") || name.contains(".env.")) {
        return None;
    }
    let template = name.contains(".env.")
        && name
            .rsplit('.')
            .next()
            .is_some_and(|suffix| TEMPLATE_SUFFIXES.contains(&suffix));
    Some(if template {
        EnvFile::Template
    } else {
        EnvFile::Secret
    })
}

/// True when reading `path` could disclose a secret: a real environment file or a
/// key / credential file. A versioned environment template never is.
pub fn is_secret_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if let Some(kind) = classify_env_file(name) {
        return kind == EnvFile::Secret;
    }
    let name = name.to_ascii_lowercase();
    name.ends_with(".pem")
        || name.ends_with(".key")
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
        || name == "credentials"
        || name == "credentials.json"
        || KRONN_SECRET_NAMES.contains(&name.as_str())
        || (name == "config.toml" && in_kronn_data_dir(path))
        || path.components().any(|component| {
            matches!(component, Component::Normal(part) if part == ".ssh" || part == ".aws")
        })
        || path
            .components()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| {
                matches!(pair, [Component::Normal(a), Component::Normal(b)] if *a == ".kronn" && *b == "backups")
            })
}

/// Files Kronn itself writes that hold the encryption key, credentials or
/// decrypted MCP environments.
const KRONN_SECRET_NAMES: [&str; 8] = [
    "encryption_key",
    "human-admin-secret",
    "secrets.toml",
    ".mcp.json",
    "kronn.db",
    "kronn.db-wal",
    "kronn.db-shm",
    "kronn.db-journal",
];

/// `config.toml` is an ordinary project name; it is a secret only as Kronn's
/// own config (it holds provider keys), i.e. inside the Kronn data directory.
fn in_kronn_data_dir(path: &Path) -> bool {
    if let Ok(dir) = crate::core::config::config_dir() {
        if path.starts_with(&dir) {
            return true;
        }
    }
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "kronn" || name == "com.kronn.kronn")
}

/// The inline OpenCode configuration Kronn starts `opencode acp` with
/// (`OPENCODE_CONFIG_CONTENT`, merged after the user's and the project's own
/// configuration). It carries two things:
///
/// - `permission.read`: the real environment files are DENIED, the templates
///   ALLOWED. OpenCode evaluates the last matching rule, so the allows come after
///   the `*.env.*` deny. A denial is not a question put to Kronn: OpenCode hands
///   the agent an error as the tool's answer and the turn goes on.
/// - `experimental.continue_loop_on_deny`: a refusal the broker gives to a
///   question OpenCode DID ask (a path outside the project, say) used to end the
///   turn (`RejectedError` stops the session loop unless this is set), so one
///   refused read aborted the whole audit step. With it the agent reads the
///   refusal as the tool's answer and carries on.
pub fn opencode_config_content() -> String {
    let mut read = String::from(r#""*.env":"deny","*.env.*":"deny""#);
    for suffix in TEMPLATE_SUFFIXES {
        read.push_str(&format!(
            r#","*.env.{suffix}":"allow","*.env.*.{suffix}":"allow""#
        ));
    }
    format!(
        r#"{{"permission":{{"read":{{{read}}}}},"experimental":{{"continue_loop_on_deny":true}}}}"#
    )
}

/// OpenCode permission keys that act on the machine or reach out (1.18): asked,
/// so the broker decides, when the session has no full access. OpenCode's own
/// default for every key is `allow`.
const OPENCODE_ASKED: &[&str] = &[
    "bash",
    "doom_loop",
    "edit",
    "external_directory",
    "skill",
    "task",
    "webfetch",
    "websearch",
];

/// The inline configuration OpenCode starts with, or `None` to leave the
/// operator's own untouched (full access only).
///
/// Without full access every permission is `ask` (`"*"` first: OpenCode keeps
/// the last matching rule) except reading and listing, the todo list, and the
/// tools of Kronn's bridge (`bridge_tools`, OpenCode permission names). The
/// same ruleset is set on the `build` agent, made the default, because an
/// agent's own permissions override the top-level ones. An operator's
/// `OPENCODE_CONFIG_CONTENT` keeps its other settings; one that is not a JSON
/// object refuses the launch rather than run unrestricted.
pub fn opencode_launch_config(
    full_access: bool,
    bridge_tools: &[String],
    operator: Option<&str>,
) -> Result<Option<String>, String> {
    if full_access {
        return Ok(operator.is_none().then(opencode_config_content));
    }
    let mut config: serde_json::Value = match operator {
        Some(raw) => serde_json::from_str(raw)
            .ok()
            .filter(serde_json::Value::is_object)
            .ok_or("OPENCODE_CONFIG_CONTENT is not a JSON object: Kronn cannot apply its OpenCode permissions, so OpenCode is not started without full access")?,
        None => serde_json::json!({}),
    };
    let base: serde_json::Value =
        serde_json::from_str(&opencode_config_content()).expect("valid OpenCode config");
    let mut read = serde_json::Map::new();
    read.insert("*".into(), "allow".into());
    if let Some(rules) = base["permission"]["read"].as_object() {
        read.extend(rules.clone());
    }
    let mut permission = serde_json::Map::new();
    permission.insert("*".into(), "ask".into());
    permission.insert("read".into(), serde_json::Value::Object(read));
    for key in ["glob", "grep", "list", "lsp", "todowrite"] {
        permission.insert(key.into(), "allow".into());
    }
    for key in OPENCODE_ASKED {
        permission.insert((*key).into(), "ask".into());
    }
    permission.insert("question".into(), "deny".into());
    for tool in bridge_tools {
        permission.insert(tool.clone(), "allow".into());
    }
    let permission = serde_json::Value::Object(permission);
    let object = config.as_object_mut().expect("checked above");
    object.insert("permission".into(), permission.clone());
    object.insert("default_agent".into(), "build".into());
    let agent = object
        .entry("agent")
        .or_insert_with(|| serde_json::json!({}));
    if !agent.is_object() {
        *agent = serde_json::json!({});
    }
    let build = agent
        .as_object_mut()
        .expect("object")
        .entry("build")
        .or_insert_with(|| serde_json::json!({}));
    if !build.is_object() {
        *build = serde_json::json!({});
    }
    build
        .as_object_mut()
        .expect("object")
        .insert("permission".into(), permission);
    let experimental = object
        .entry("experimental")
        .or_insert_with(|| serde_json::json!({}));
    if !experimental.is_object() {
        *experimental = serde_json::json!({});
    }
    experimental
        .as_object_mut()
        .expect("object")
        .insert("continue_loop_on_deny".into(), true.into());
    Ok(Some(config.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn secret(path: &str) -> bool {
        is_secret_file(Path::new(path))
    }

    #[test]
    fn versioned_environment_templates_are_not_secrets() {
        for name in [
            ".env.dist",
            ".env.example",
            ".env.sample",
            ".env.template",
            ".env.local.dist",
            ".env.production.example",
            "config/.env.dist",
            ".ENV.DIST",
        ] {
            assert_eq!(
                classify_env_file(name.rsplit('/').next().unwrap()),
                Some(EnvFile::Template),
                "{name}"
            );
            assert!(!secret(name), "{name} is a template");
        }
    }

    #[test]
    fn real_environment_files_and_keys_are_secrets() {
        for name in [
            ".env",
            ".env.local",
            ".env.production",
            ".env.dist.local",
            "prod.env",
            ".envrc",
            "config/.env",
            "server.pem",
            "tls/private.key",
            "id_rsa",
            "id_ed25519.pub",
            "credentials.json",
            ".aws/config",
            "home/.ssh/known_hosts",
        ] {
            assert!(secret(name), "{name} must stay refused");
        }
    }

    #[test]
    fn kronn_key_config_db_and_mcp_files_are_secrets() {
        for name in [
            "/home/u/.config/kronn/encryption_key",
            "/home/u/.config/kronn/config.toml",
            "/Users/u/Library/Application Support/com.kronn.kronn/config.toml",
            "/data/human-admin-secret",
            "/data/secrets.toml",
            "/data/kronn.db",
            "/data/kronn.db-wal",
            "repo/.mcp.json",
            "repo/.kronn/backups/mcp-configs/x.backup",
        ] {
            assert!(secret(name), "{name} must be refused");
        }
    }

    #[test]
    fn an_ordinary_project_config_toml_stays_readable() {
        for name in [
            "repo/config.toml",
            "repo/.cargo/config.toml",
            "repo/backups/notes.md",
        ] {
            assert!(!secret(name), "{name}");
        }
    }

    #[test]
    fn ordinary_files_are_not_secrets() {
        for name in [
            "README.md",
            "src/main.rs",
            "docs/environment.md",
            "env.example",
            "keys.md",
        ] {
            assert!(!secret(name), "{name}");
        }
    }

    #[test]
    fn the_opencode_config_denies_real_env_files_and_allows_templates_in_that_order() {
        let content = opencode_config_content();
        let parsed: Value = serde_json::from_str(&content).expect("valid JSON");
        assert_eq!(parsed["experimental"]["continue_loop_on_deny"], true);
        assert_eq!(parsed["permission"]["read"]["*.env"], "deny");
        assert_eq!(parsed["permission"]["read"]["*.env.*"], "deny");
        for suffix in TEMPLATE_SUFFIXES {
            assert_eq!(
                parsed["permission"]["read"][format!("*.env.{suffix}")],
                "allow"
            );
            assert_eq!(
                parsed["permission"]["read"][format!("*.env.*.{suffix}")],
                "allow"
            );
        }
        // OpenCode honours the LAST matching rule, in the order written: every
        // allow has to come after the `*.env.*` deny it carves an exception from.
        let deny = content.find(r#""*.env.*":"deny""#).unwrap();
        for suffix in TEMPLATE_SUFFIXES {
            assert!(
                content
                    .find(&format!(r#""*.env.{suffix}":"allow""#))
                    .unwrap()
                    > deny
            );
            assert!(
                content
                    .find(&format!(r#""*.env.*.{suffix}":"allow""#))
                    .unwrap()
                    > deny
            );
        }
    }

    fn restricted(operator: Option<&str>) -> Value {
        let tools = vec!["kronn-internal-0123456789ab_*".to_owned()];
        serde_json::from_str(
            &opencode_launch_config(false, &tools, operator)
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn without_full_access_opencode_asks_before_acting_and_keeps_the_bridge() {
        let config = restricted(None);
        for permission in [
            &config["permission"],
            &config["agent"]["build"]["permission"],
        ] {
            assert_eq!(permission["*"], "ask");
            for key in OPENCODE_ASKED {
                assert_eq!(permission[*key], "ask", "{key}");
            }
            for key in ["glob", "grep", "list", "lsp", "todowrite"] {
                assert_eq!(permission[key], "allow", "{key}");
            }
            assert_eq!(permission["question"], "deny");
            assert_eq!(permission["read"]["*"], "allow");
            assert_eq!(permission["read"]["*.env"], "deny");
            assert_eq!(permission["kronn-internal-0123456789ab_*"], "allow");
        }
        assert_eq!(config["default_agent"], "build");
        assert_eq!(config["experimental"]["continue_loop_on_deny"], true);
        // OpenCode keeps the last matching rule: the catch-all comes first.
        let text = opencode_launch_config(false, &[], None).unwrap().unwrap();
        let at = |text: &str, needle: &str| text.find(needle).expect(needle);
        assert!(at(&text, r#""*":"ask""#) < at(&text, r#""bash":"ask""#));
        let read = &text[at(&text, r#""read":{"#)..];
        assert!(at(read, r#""*":"allow""#) < at(read, r#""*.env":"deny""#));
    }

    #[test]
    fn an_operator_config_keeps_its_settings_but_not_its_permissions() {
        let config = restricted(Some(
            r#"{"theme":"mine","default_agent":"yolo","permission":{"bash":"allow","*":"allow"},
                "agent":{"build":{"model":"m","permission":{"bash":"allow"}}}}"#,
        ));
        assert_eq!(config["theme"], "mine");
        assert_eq!(config["agent"]["build"]["model"], "m");
        assert_eq!(config["default_agent"], "build");
        assert_eq!(config["permission"]["bash"], "ask");
        assert_eq!(config["permission"]["*"], "ask");
        assert_eq!(config["agent"]["build"]["permission"]["bash"], "ask");
        for refused in ["not json", "[1]", "\"allow\""] {
            assert!(
                opencode_launch_config(false, &[], Some(refused)).is_err(),
                "{refused}"
            );
        }
        // Full access: the operator's configuration is theirs.
        assert_eq!(opencode_launch_config(true, &[], Some("{}")).unwrap(), None);
        assert_eq!(
            opencode_launch_config(true, &[], None).unwrap(),
            Some(opencode_config_content())
        );
    }
}
