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
}
