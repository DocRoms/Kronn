//! The `config.toml.corrupt.<ts>` files Kronn keeps when it cannot read a
//! `config.toml` (KT-1594). They may hold credentials, and the key, in clear.
//!
//! Kronn never deletes one by itself. The owner may delete one here only when
//! Kronn can prove that nothing in it is lost. Anything unknown, unparsable
//! or sealed blocks the delete, with a reason the screen shows:
//!
//! - a sealed copy (`.enc`) is never deleted here: it may be the only copy;
//! - every leaf value of the file is either a credential verified in
//!   `stored_credentials` (it decrypts with the key in use and equals the
//!   value of the file) or sits at a place a closed list, from the config
//!   model, knows to hold no secret (ports, flags, names, language) with the
//!   type that place has. Nothing is judged by what a field is called: an
//!   unknown field, a non-text value at a credential place, a webhook URL, a
//!   theme secret or an invalid `encryption_secret` blocks;
//! - a file that is not valid TOML is read line by line, under the section it
//!   sat in: a line that does not parse (a header, a `key = value` with no
//!   quote, a cut string) blocks;
//! - the key the file holds must not be the only way back to a database row,
//!   a `locked-secrets-*.json` or an `.enc` file, and must not be the only
//!   copy of the key in use.
//!
//! The file is read and unlinked through the data directory without following
//! a link, and unlinked only if its name still names the file that was read.
//! Values never leave this module: the API only gets names and verdicts.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};

use crate::core::keyvault::KeyStore;
use crate::core::{config, crypto, keystore, rooted_io};
use crate::db::stored_credentials::{self as rows, KIND_AUTH_TOKEN, KIND_PROVIDER_KEY};
use crate::db::Database;

/// The prefix of a kept `config.toml`.
pub const PREFIX: &str = "config.toml.corrupt.";

/// A secret in a place `stored_credentials` has no row for (the webhook URL,
/// the theme secrets, a field this build does not know). Never verified.
pub const KIND_CONFIG_SECRET: &str = "config_secret";

/// The most a set-aside `config.toml` may weigh to be assessed.
const MAX_FILE: u64 = 4 * 1024 * 1024;
/// The most a key-bound file may weigh to be read.
const MAX_BOUND_FILE: u64 = 64 * 1024 * 1024;

/// A credential a config text carries in clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearCredential {
    /// `provider_key` or `auth_token`, as in `stored_credentials`, or
    /// [`KIND_CONFIG_SECRET`].
    pub kind: &'static str,
    /// For [`KIND_CONFIG_SECRET`], the place it sits in (never the value).
    pub name: String,
    pub provider: String,
    pub value: String,
}

/// Why a set-aside file may not be deleted. The code is what the UI maps to a
/// sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// The file cannot be read (not text, a link, too big, an I/O error).
    Unreadable,
    /// A sealed (encrypted) copy: never deleted here.
    Sealed,
    /// No key in use: nothing can be verified.
    KeyLocked,
    /// This many lines cannot be read and could hide a secret.
    Unparsed(usize),
    /// This many values sit at a place the config model does not know as
    /// non-secret (an unknown field, a value of the wrong type).
    UnknownValue(usize),
    /// This many credentials are not in `stored_credentials`.
    Unverified(usize),
    /// The file holds a key that still opens this many rows the key in use
    /// does not.
    KeyNeeded(usize),
    /// The file holds a key that still opens this many key-bound files (or
    /// their rows) the key in use does not.
    KeyBoundFiles(usize),
    /// The key-bound files (or the data directory) could not be read or parsed:
    /// nothing proves that none of them needs the key of this file.
    KeyCheckUnavailable,
    /// The file holds the only copy of the key in use.
    OnlyKeyCopy,
}

impl Blocker {
    pub fn code(&self) -> &'static str {
        match self {
            Blocker::Unreadable => "unreadable",
            Blocker::Sealed => "sealed",
            Blocker::KeyLocked => "key_locked",
            Blocker::Unparsed(_) => "unparsed",
            Blocker::UnknownValue(_) => "unknown_value",
            Blocker::Unverified(_) => "unverified",
            Blocker::KeyNeeded(_) => "key_needed",
            Blocker::KeyBoundFiles(_) => "key_bound_files",
            Blocker::KeyCheckUnavailable => "key_check_unavailable",
            Blocker::OnlyKeyCopy => "only_key_copy",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Blocker::Unreadable => "The file cannot be read, so its credentials cannot be \
                                    verified."
                .into(),
            Blocker::Sealed => "A sealed copy is never deleted here: it may be the only copy of \
                                what it holds."
                .into(),
            Blocker::KeyLocked => "No encryption key is in use: restore it first, nothing can be \
                                   verified while it is locked."
                .into(),
            Blocker::Unparsed(n) => format!(
                "{n} line(s) of the file cannot be read and could hide a credential: it cannot \
                 be proven that nothing is lost."
            ),
            Blocker::UnknownValue(n) => format!(
                "{n} value(s) of the file are at a place this version of Kronn does not know as \
                 harmless (an unknown field, or a value of the wrong type): it cannot be proven \
                 that nothing is lost."
            ),
            Blocker::Unverified(n) => format!(
                "{n} credential(s) in the file are not in the encrypted store (or differ from \
                 it): re-enter them in Kronn first."
            ),
            Blocker::KeyNeeded(n) => format!(
                "The file holds an encryption key that still opens {n} stored row(s) the key in \
                 use does not: restore or re-encrypt them first."
            ),
            Blocker::KeyBoundFiles(n) => format!(
                "The file holds an encryption key that still opens {n} other file(s) or \
                 row(s) kept aside (locked secrets, sealed or preserved copies) the key in use \
                 does not: restore or re-encrypt them first."
            ),
            Blocker::KeyCheckUnavailable => "The files kept aside next to it could not all be \
                                             read, so it cannot be proven that none of them \
                                             needs the key this file holds: try again."
                .into(),
            Blocker::OnlyKeyCopy => "The file holds the only copy of the key in use: set a \
                                     recovery passphrase or KRONN_ENCRYPTION_KEK first."
                .into(),
        }
    }
}

/// What a set-aside file holds and whether it may go.
#[derive(Debug, Clone)]
pub struct Assessment {
    /// Each credential of the file, and whether it is verified.
    pub credentials: Vec<(ClearCredential, bool)>,
    /// The file carries an encryption key.
    pub holds_key: bool,
    pub blocker: Option<Blocker>,
}

impl Assessment {
    pub fn deletable(&self) -> bool {
        self.blocker.is_none()
    }

    fn blocked(blocker: Blocker) -> Self {
        Self {
            credentials: Vec::new(),
            holds_key: false,
            blocker: Some(blocker),
        }
    }
}

/// The names of the set-aside files of `dir`, oldest first.
pub fn files(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| valid_name(n))
        .collect();
    names.sort();
    names
}

/// A set-aside file name and nothing else: no separator, no `..`.
fn valid_name(name: &str) -> bool {
    name.strip_prefix(PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    })
}

/// A sealed copy: encrypted, whatever it is a copy of.
fn sealed(name: &str) -> bool {
    name.ends_with(".enc")
}

/// What a config text carries in clear.
#[derive(Debug, Default)]
struct Scan {
    /// Credentials the store can hold, and the secrets of places it cannot.
    credentials: Vec<ClearCredential>,
    /// Lines of a file that is not valid TOML that cannot be read on their own
    /// (an unreadable header, an unterminated quote, a line of a multi-line
    /// string): a secret could sit in them.
    unparsed: usize,
    /// Values at a place the config model does not know as non-secret: an
    /// unknown field, a value of the wrong type for its field.
    unknown: usize,
    /// The key the file holds, the only `encryption_secret` that is not a
    /// value to verify: the key checks of [`assess_identified`] handle it.
    file_key: Option<String>,
    /// The `name` and `provider` of the `tokens.keys` entry being walked.
    entry: (String, String),
    /// A header of the file could not be read: the lines under it sit in an
    /// unknown section, and a bare credential name is read as the field it
    /// usually is.
    lost: bool,
}

/// The agents of `[agents]` and `[agents.model_tiers]`.
const AGENTS: [&str; 10] = [
    "claude_code",
    "codex",
    "open_code",
    "gemini_cli",
    "kiro",
    "vibe",
    "copilot_cli",
    "ollama",
    "lite_llm",
    "nvidia",
];

/// What a non-secret setting holds.
#[derive(Clone, Copy)]
enum Shape {
    Flag,
    Number,
    Text,
    /// A text that is an address: with no user info, query or fragment, where
    /// a credential would travel.
    Url,
}

impl Shape {
    fn fits(self, value: &toml::Value) -> bool {
        match (self, value) {
            (Shape::Flag, toml::Value::Boolean(_)) => true,
            (Shape::Number, toml::Value::Integer(_) | toml::Value::Float(_)) => true,
            (Shape::Text, toml::Value::String(_)) => true,
            (Shape::Url, toml::Value::String(text)) => !text.contains(['@', '?', '#']),
            _ => false,
        }
    }
}

/// The closed list of the places of a `config.toml` that hold no secret, from
/// the config model (`AppConfig` and what it holds). `[]` stands for an array
/// item. A place that is not here is unknown: it blocks the delete, whatever
/// its name says. The secrets of the model (`encryption_secret`,
/// `server.auth_token`, `server.failure_notify_url`, `secret_themes`, the
/// provider keys) are not here either: they are verified or block.
fn non_secret_shape(path: &[&str]) -> Option<Shape> {
    use Shape::*;
    Some(match path {
        ["language" | "ui_language" | "stt_model" | "audit_reverify_cron"] => Text,
        ["audit_briefing_docs"] => Flag,
        ["tts_voices", _] => Text,
        ["disabled_agents" | "unlocked_profiles" | "disabled_auto_skills", "[]"] => Text,
        ["embed_allowed_origins", "[]"] => Url,
        ["workers", "default_max_review_rounds" | "max_concurrent_task_workers" | "heavy_output_kb"] => {
            Number
        }
        ["workers", "review_rounds_unlimited"] => Flag,
        ["workers", "background_priority"] => Flag,
        ["tokens", "disabled_overrides", "[]"] => Text,
        ["tokens", "keys", "[]", "id" | "name" | "provider"] => Text,
        ["tokens", "keys", "[]", "active"] => Flag,
        ["tokens", "keys", "[]", "position"] => Number,
        ["scan", "paths" | "ignore", "[]"] => Text,
        ["scan", "scan_depth"] => Number,
        ["scan", "default_docs_dir"] => Text,
        ["comfyui", "endpoint"] => Url,
        ["comfyui", "image_model" | "video_model" | "image_family" | "install_dir"] => Text,
        ["comfyui", "model_families", _] => Text,
        ["comfyui", "image_settings" | "video_settings", "steps" | "cfg" | "denoise"] => Number,
        ["comfyui", "image_settings" | "video_settings", "sampler" | "scheduler"] => Text,
        ["media_reference_limits", "[]", "connection_id" | "workflow" | "model" | "modality"] => {
            Text
        }
        ["media_reference_limits", "[]", "endpoint"] => Url,
        ["media_reference_limits", "[]", "max_input_references"] => Number,
        ["agents", agent, field] if AGENTS.contains(agent) => match *field {
            "path" | "version" | "mention_color" => Text,
            "installed" | "full_access" => Flag,
            "concurrency" => Number,
            "base_url" => Url,
            _ => return None,
        },
        ["agents", "model_tiers", agent, field]
            if AGENTS.contains(agent)
                && matches!(
                    *field,
                    "economy"
                        | "default"
                        | "reasoning"
                        | "decision"
                        | "economy_effort"
                        | "default_effort"
                        | "reasoning_effort"
                ) =>
        {
            Text
        }
        ["server", field] => match *field {
            "host"
            | "domain"
            | "pseudo"
            | "avatar_email"
            | "bio"
            | "global_context"
            | "global_context_mode"
            | "anti_hallucination_mode"
            | "default_model_tier"
            | "default_summary_strategy"
            | "timezone" => Text,
            "auth_enabled"
            | "auth_strict_localhost"
            | "protect_vault_from_agents"
            | "lane_seeds_copy_enabled"
            | "lane_seeds_adopt_suggested"
            | "lane_seeds_strong_confirm"
            | "tls_terminated_by_proxy"
            | "p2p_enabled"
            | "continual_learning_enabled"
            | "discussion_notes_enabled"
            | "debug_mode"
            | "agent_handoffs_enabled"
            | "agent_handoff_paid_unlimited" => Flag,
            "port"
            | "run_retention_days"
            | "run_payload_retention_days"
            | "execution_variable_retention_days"
            | "interrupted_worktree_ttl_days"
            | "disk_critical_gib"
            | "disk_warning_gib"
            | "disk_launch_min_free_gib"
            | "disk_sweep_idle_hours"
            | "max_concurrent_agents"
            | "agent_stall_timeout_min"
            | "agent_global_timeout_min"
            | "local_agent_global_timeout_min"
            | "max_prompt_bytes"
            | "agent_handoff_paid_limit" => Number,
            _ => return None,
        },
        ["server", "frontend_origins", "[]"] => Url,
        ["server", "agent_handoff_blocked_agents", "[]"] => Text,
        ["server", "ollama_context_overrides", _] => Number,
        ["server", "discussion_weight", "enabled"] => Flag,
        ["server", "discussion_weight", "amber_bytes" | "red_bytes"] => Number,
        _ => return None,
    })
}

/// The `[a.b]` or `[[a.b]]` a header line names, with `[]` for the entry of an
/// array of tables; `None` if it names nothing.
fn header_path(header: &toml::Table) -> Option<Vec<String>> {
    let mut path = Vec::new();
    let mut here = header;
    loop {
        let mut entries = here.iter();
        let Some((key, value)) = entries.next() else {
            return Some(path);
        };
        if entries.next().is_some() {
            return None;
        }
        path.push(key.clone());
        match value {
            toml::Value::Table(table) => here = table,
            toml::Value::Array(items) => match items.as_slice() {
                [toml::Value::Table(table)] => {
                    path.push("[]".into());
                    here = table;
                }
                _ => return None,
            },
            _ => return None,
        }
    }
}

impl Scan {
    fn push(&mut self, cred: ClearCredential) {
        if !cred.value.is_empty()
            && !self
                .credentials
                .iter()
                .any(|c| c.kind == cred.kind && c.value == cred.value)
        {
            self.credentials.push(cred);
        }
    }

    fn secret(&mut self, kind: &'static str, name: &str, provider: &str, value: &str) {
        self.push(ClearCredential {
            kind,
            name: name.to_string(),
            provider: provider.to_string(),
            value: value.to_string(),
        });
    }

    /// Every leaf under `value` is judged by [`Scan::leaf`]; there is no place
    /// the walk skips.
    fn walk(&mut self, path: &mut Vec<String>, value: &toml::Value) {
        match value {
            toml::Value::Table(table) => {
                for (k, v) in table {
                    path.push(k.clone());
                    self.walk(path, v);
                    path.pop();
                }
            }
            toml::Value::Array(items) => {
                let keys = path.iter().map(String::as_str).eq(["tokens", "keys"]);
                for item in items {
                    let label = |k: &str| {
                        item.get(k)
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string()
                    };
                    self.entry = if keys {
                        (label("name"), label("provider"))
                    } else {
                        Default::default()
                    };
                    path.push("[]".into());
                    self.walk(path, item);
                    path.pop();
                }
                self.entry = Default::default();
            }
            _ => self.leaf(path, value),
        }
    }

    /// One value. It is a credential to verify, the key the file holds, a
    /// setting the config model knows holds no secret, or an unknown value.
    fn leaf(&mut self, path: &[String], value: &toml::Value) {
        let text = value.as_str();
        if text.is_some_and(|t| t.trim().is_empty()) {
            return;
        }
        let mut names: Vec<&str> = path.iter().map(String::as_str).collect();
        // Under an unreadable header a bare credential name is read as the
        // field it usually is (the place is unknown, the value is not lost).
        let bare = match names.as_slice() {
            ["auth_token"] if self.lost => Some(vec!["server", "auth_token"]),
            ["value"] if self.lost => Some(vec!["tokens", "keys", "[]", "value"]),
            [p @ ("anthropic" | "openai" | "google")] if self.lost => Some(vec!["tokens", *p]),
            _ => None,
        };
        if let Some(bare) = bare {
            names = bare;
        }
        match (names.as_slice(), text) {
            (["encryption_secret"], Some(t))
                if self.file_key.is_some() && crypto::canonical_secret(t).ok() == self.file_key =>
            {
                // The key the file holds: its fate is the key checks'.
            }
            (["server", "auth_token"], Some(t)) => self.secret(KIND_AUTH_TOKEN, "", "", t),
            (["tokens", p @ ("anthropic" | "openai" | "google")], Some(t)) => {
                self.secret(KIND_PROVIDER_KEY, "", p, t)
            }
            (["tokens", "keys", "[]", "value"], Some(t)) => {
                let (name, provider) = self.entry.clone();
                self.secret(KIND_PROVIDER_KEY, &name, &provider, t)
            }
            ([.., "failure_notify_url"] | ["secret_themes", ..], Some(t)) => {
                self.secret(KIND_CONFIG_SECRET, &names.join("."), "", t)
            }
            _ if non_secret_shape(&names).is_some_and(|s| s.fits(value)) => {}
            _ => self.unknown += 1,
        }
    }

    fn table(&mut self, base: &[String], table: &toml::Table) {
        let mut path = base.to_vec();
        for (k, v) in table {
            path.push(k.clone());
            self.walk(&mut path, v);
            path.pop();
        }
    }
}

/// Everything a config.toml text carries that is not proven harmless. A valid
/// text is walked leaf by leaf. A text that is not valid TOML is read line by
/// line, each under the section it sat in; a line that cannot be read, a
/// header included, is counted: it could hide a secret.
fn scan(text: &str) -> Scan {
    let mut out = Scan {
        file_key: key_in(text),
        ..Scan::default()
    };
    if let Ok(table) = text.parse::<toml::Table>() {
        out.table(&[], &table);
        return out;
    }
    let mut section: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            match line
                .parse::<toml::Table>()
                .ok()
                .as_ref()
                .and_then(header_path)
            {
                Some(path) => {
                    section = path;
                    out.lost = false;
                }
                None => {
                    out.unparsed += 1;
                    section.clear();
                    out.lost = true;
                }
            }
            continue;
        }
        match line.parse::<toml::Table>() {
            Ok(one) => out.table(&section, &one),
            Err(_) => out.unparsed += 1,
        }
    }
    out
}

/// The credentials the encrypted store can hold that a config.toml text
/// carries in clear (the provider keys and the auth token); see [`scan`] for
/// the stricter reading the set-aside files get.
pub fn clear_credentials_in(text: &str) -> Vec<ClearCredential> {
    scan(text)
        .credentials
        .into_iter()
        .filter(|c| c.kind != KIND_CONFIG_SECRET)
        .collect()
}

/// The canonical `encryption_secret` a config text carries, if it is a key.
fn key_in(text: &str) -> Option<String> {
    text.parse::<toml::Table>()
        .ok()
        .and_then(|t| {
            t.get("encryption_secret")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .or_else(|| config::salvage_key_line(text))
        .and_then(|k| crypto::canonical_secret(&k).ok())
}

/// The `(kind, value)` pairs `stored_credentials` holds and `key` decrypts.
pub async fn stored_values(db: &Database, key: &str) -> Result<HashSet<(String, String)>> {
    let key = crypto::parse_secret(key).map_err(anyhow::Error::msg)?;
    let stored = db.with_conn(rows::list).await?;
    Ok(stored
        .into_iter()
        .filter(|r| r.kind == KIND_PROVIDER_KEY || r.kind == KIND_AUTH_TOKEN)
        .filter_map(|r| {
            crypto::decrypt(&r.value_encrypted, &key)
                .ok()
                .map(|v| (r.kind, v))
        })
        .collect())
}

/// Each credential of `text` and whether the store holds it (all `false` with
/// no key in use, and for [`KIND_CONFIG_SECRET`]: the store has no row for it).
pub async fn verify(
    db: &Database,
    key: Option<&str>,
    creds: Vec<ClearCredential>,
) -> Result<Vec<(ClearCredential, bool)>> {
    let stored = match key {
        Some(key) => stored_values(db, key).await?,
        None => HashSet::new(),
    };
    Ok(creds
        .into_iter()
        .map(|c| {
            let ok = stored.contains(&(c.kind.to_string(), c.value.clone()));
            (c, ok)
        })
        .collect())
}

/// A vault, or the config.toml in use, still keeps `key`: computed now, from
/// the disk and the vaults (a rekey since the boot is seen).
pub fn key_kept_elsewhere(dir: &Path, store: &KeyStore, key: &str) -> bool {
    store.copies_of(key) >= 1
        || rooted_io::read_capped(dir, Path::new("config.toml"), MAX_FILE)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| key_in(&text))
            .is_some_and(|kept| crate::core::keyvault::same_key(&kept, key))
}

/// How many key-bound things only `other` opens, not `key`: the rows of
/// `locked-secrets-*.json`, and each `.enc` file of the data directory (the
/// config backup, the preserved values, sealed copies). `Err` when anything
/// could not be read or parsed (the directory, an entry, a file, a key): then
/// nothing proves that none of them is bound to `other`.
fn key_bound_needing(dir: &Path, key: &str, other: &str) -> Result<usize, ()> {
    let (Ok(key), Ok(other)) = (crypto::parse_secret(key), crypto::parse_secret(other)) else {
        return Err(());
    };
    let opens = |cipher: &str| {
        let cipher = cipher.trim();
        crypto::decrypt(cipher, &other).is_ok() && crypto::decrypt(cipher, &key).is_err()
    };
    let mut needing = 0;
    for entry in std::fs::read_dir(dir).map_err(|_| ())? {
        let name = entry
            .map_err(|_| ())?
            .file_name()
            .to_string_lossy()
            .into_owned();
        let locked = name.starts_with(keystore::LOCKED_SECRETS_PREFIX) && name.ends_with(".json");
        if !locked && !sealed(&name) {
            continue;
        }
        let text = rooted_io::read_capped(dir, Path::new(&name), MAX_BOUND_FILE)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or(())?;
        if !locked {
            needing += usize::from(opens(&text));
            continue;
        }
        let items = serde_json::from_str::<Vec<serde_json::Value>>(&text).map_err(|_| ())?;
        needing += items
            .iter()
            .filter(|item| {
                keystore::ENCRYPTED_COLUMNS
                    .iter()
                    .filter(|c| item["table"].as_str() == Some(c.table))
                    .filter_map(|c| item["row"][c.column].as_str())
                    .any(opens)
            })
            .count();
    }
    Ok(needing)
}

/// Read a set-aside file of `dir` and decide whether it may be deleted; the
/// identity of the file that was read comes with the verdict.
async fn assess_identified(
    db: &Database,
    dir: &Path,
    key: Option<&str>,
    name: &str,
    store: &KeyStore,
) -> Result<(Assessment, Option<rooted_io::Identity>)> {
    anyhow::ensure!(valid_name(name), "not a set-aside config.toml: {name}");
    if sealed(name) {
        return Ok((Assessment::blocked(Blocker::Sealed), None));
    }
    let read = rooted_io::read_identified(dir, Path::new(name), MAX_FILE);
    let (bytes, identity) = match read {
        Ok(read) => read,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(e).context("read the set-aside file")
        }
        Err(_) => return Ok((Assessment::blocked(Blocker::Unreadable), None)),
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok((Assessment::blocked(Blocker::Unreadable), Some(identity)));
    };
    let scanned = scan(&text);
    let (unparsed, unknown) = (scanned.unparsed, scanned.unknown);
    let credentials = verify(db, key, scanned.credentials).await?;
    let file_key = key_in(&text);
    let unverified = credentials.iter().filter(|(_, ok)| !ok).count();
    let blocker = match key {
        None => Some(Blocker::KeyLocked),
        Some(_) if unparsed > 0 => Some(Blocker::Unparsed(unparsed)),
        Some(_) if unknown > 0 => Some(Blocker::UnknownValue(unknown)),
        Some(_) if unverified > 0 => Some(Blocker::Unverified(unverified)),
        Some(key) => match file_key.as_deref() {
            None => None,
            Some(k) if crate::core::keyvault::same_key(k, key) => {
                (!key_kept_elsewhere(dir, store, key)).then_some(Blocker::OnlyKeyCopy)
            }
            Some(k) => match keystore::rows_only_this_key_opens(db, key, k).await? {
                0 => match key_bound_needing(dir, key, k) {
                    Ok(0) => None,
                    Ok(n) => Some(Blocker::KeyBoundFiles(n)),
                    Err(()) => Some(Blocker::KeyCheckUnavailable),
                },
                n => Some(Blocker::KeyNeeded(n)),
            },
        },
    };
    let assessment = Assessment {
        credentials,
        holds_key: file_key.is_some(),
        blocker,
    };
    Ok((assessment, Some(identity)))
}

/// Read a set-aside file of `dir` and decide whether it may be deleted.
pub async fn assess(
    db: &Database,
    dir: &Path,
    key: Option<&str>,
    name: &str,
    store: &KeyStore,
) -> Result<Assessment> {
    Ok(assess_identified(db, dir, key, name, store).await?.0)
}

/// Delete a set-aside file, after checking it again: the verdict the screen
/// showed may be stale. What is checked is what was read through the data
/// directory, and it is unlinked only if its name still names that file.
pub async fn delete(
    db: &Database,
    dir: &Path,
    key: Option<&str>,
    name: &str,
    store: &KeyStore,
) -> Result<()> {
    let (verdict, identity) = assess_identified(db, dir, key, name, store).await?;
    if let Some(blocker) = verdict.blocker {
        anyhow::bail!("{}", blocker.message());
    }
    let identity = identity.context("the set-aside file was not identified")?;
    rooted_io::remove_if_same(dir, Path::new(name), &identity)
        .context("delete the set-aside file")?;
    crate::core::keyvault::sync_dir(dir);
    config::forget_set_aside(dir, name);
    tracing::warn!("{name} deleted by the owner: every credential in it was verified");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::stored_credentials::StoredCredential;

    const FILE_KEY: &str = "sk-ant-fixture-in-the-file-0001";
    const OTHER: &str = "sk-openai-fixture-not-stored-02";
    const TOKEN: &str = "auth-token-fixture-0003";
    const NAME: &str = "config.toml.corrupt.1";

    fn key() -> String {
        crypto::generate_secret()
    }

    /// No vault keeps anything: the key in use has no copy outside the files.
    fn no_vault() -> KeyStore {
        KeyStore::from_vaults(Vec::new())
    }

    fn row(kind: &str, id: &str, value: &str, key: &str) -> StoredCredential {
        StoredCredential {
            kind: kind.into(),
            id: id.into(),
            name: String::new(),
            provider: String::new(),
            active: true,
            position: 0,
            value_encrypted: crypto::encrypt(value, &crypto::parse_secret(key).unwrap()).unwrap(),
        }
    }

    async fn store(db: &Database, rows_: Vec<StoredCredential>) {
        db.with_conn(move |c| rows::upsert_all(c, &rows_))
            .await
            .unwrap();
    }

    fn file(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn valid_config() -> String {
        format!(
            "[server]\nauth_token = \"{TOKEN}\"\n\n[[tokens.keys]]\nid = \"a\"\nname = \"Anthropic\"\n\
             provider = \"anthropic\"\nvalue = \"{FILE_KEY}\"\nactive = true\n"
        )
    }

    /// The credentials of `valid_config` are in the store, under `k`.
    async fn store_valid_config(db: &Database, k: &str) {
        store(
            db,
            vec![
                row(KIND_PROVIDER_KEY, "a", FILE_KEY, k),
                row(KIND_AUTH_TOKEN, "server", TOKEN, k),
            ],
        )
        .await;
    }

    /// A database whose store holds `valid_config`, a key in use, a data dir.
    async fn verified() -> (tempfile::TempDir, Database, String) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let k = key();
        store_valid_config(&db, &k).await;
        (dir, db, k)
    }

    async fn blocker_of(
        dir: &Path,
        db: &Database,
        k: &str,
        text: &str,
    ) -> (Option<Blocker>, Vec<(ClearCredential, bool)>) {
        file(dir, NAME, text);
        let a = assess(db, dir, Some(k), NAME, &no_vault()).await.unwrap();
        (a.blocker, a.credentials)
    }

    #[test]
    fn credentials_of_a_valid_config_are_listed_by_name_and_kind() {
        let found = clear_credentials_in(&valid_config());
        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .any(|c| c.kind == KIND_AUTH_TOKEN && c.value == TOKEN));
        assert!(found.iter().any(|c| c.kind == KIND_PROVIDER_KEY
            && c.name == "Anthropic"
            && c.provider == "anthropic"
            && c.value == FILE_KEY));
        assert!(clear_credentials_in("language = \"fr\"\n").is_empty());
    }

    #[test]
    fn a_broken_file_still_lists_its_credentials() {
        let text = format!(
            "[server\nauth_token = \"{TOKEN}\"\n[[tokens.keys\nvalue = \"{FILE_KEY}\"\nnot toml at all\n"
        );
        assert!(toml::from_str::<toml::Table>(&text).is_err());
        let found = clear_credentials_in(&text);
        assert!(found.iter().any(|c| c.value == TOKEN), "{found:?}");
        assert!(found.iter().any(|c| c.value == FILE_KEY), "{found:?}");
    }

    #[test]
    fn only_set_aside_names_are_accepted() {
        assert!(valid_name("config.toml.corrupt.20261001T000000.000001Z"));
        for bad in [
            "config.toml",
            "config.toml.corrupt.",
            "config.toml.corrupt.../../x",
            "config.toml.corrupt.a/b",
            "../config.toml.corrupt.1",
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_file_is_deletable_only_once_every_credential_is_stored() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let k = key();
        let vault = no_vault();
        file(dir.path(), NAME, &valid_config());

        // Nothing stored: both are unverified.
        let a = assess(&db, dir.path(), Some(&k), NAME, &vault)
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unverified(2)));
        assert!(!a.deletable());
        assert!(delete(&db, dir.path(), Some(&k), NAME, &vault)
            .await
            .is_err());
        assert!(dir.path().join(NAME).exists());

        // One of the two stored: still refused.
        store(&db, vec![row(KIND_PROVIDER_KEY, "a", FILE_KEY, &k)]).await;
        let a = assess(&db, dir.path(), Some(&k), NAME, &vault)
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unverified(1)));
        assert_eq!(a.credentials.iter().filter(|(_, ok)| *ok).count(), 1);

        // Stored, but under another key (or another value): not verified.
        store(&db, vec![row(KIND_AUTH_TOKEN, "server", TOKEN, &key())]).await;
        let a = assess(&db, dir.path(), Some(&k), NAME, &vault)
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unverified(1)));
        store(&db, vec![row(KIND_AUTH_TOKEN, "server", OTHER, &k)]).await;
        let a = assess(&db, dir.path(), Some(&k), NAME, &vault)
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unverified(1)));

        // Every credential verified: it goes.
        store(&db, vec![row(KIND_AUTH_TOKEN, "server", TOKEN, &k)]).await;
        let a = assess(&db, dir.path(), Some(&k), NAME, &vault)
            .await
            .unwrap();
        assert!(a.deletable(), "{:?}", a.blocker);
        delete(&db, dir.path(), Some(&k), NAME, &vault)
            .await
            .unwrap();
        assert!(!dir.path().join(NAME).exists());
    }

    #[tokio::test]
    async fn a_locked_key_verifies_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        file(dir.path(), NAME, "language = \"fr\"\n");
        let a = assess(&db, dir.path(), None, NAME, &no_vault())
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::KeyLocked));
        assert!(delete(&db, dir.path(), None, NAME, &no_vault())
            .await
            .is_err());
        assert!(dir.path().join(NAME).exists());
    }

    #[tokio::test]
    async fn an_unreadable_file_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        std::fs::write(dir.path().join(NAME), [0xff, 0xfe, 0x00, 0x9f]).unwrap();
        let a = assess(&db, dir.path(), Some(&key()), NAME, &no_vault())
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unreadable));
    }

    #[tokio::test]
    async fn the_key_it_holds_is_not_lost_with_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let (in_use, old) = (key(), key());
        file(
            dir.path(),
            NAME,
            &format!("encryption_secret = \"{old}\"\nlanguage = \"fr\"\n"),
        );

        // A row only the old key opens: the file is the way back to it.
        store(&db, vec![row(KIND_PROVIDER_KEY, "x", "locked", &old)]).await;
        let a = assess(&db, dir.path(), Some(&in_use), NAME, &no_vault())
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::KeyNeeded(1)));
        assert!(a.holds_key);

        // No row needs it any more.
        db.with_conn(rows::delete_all).await.unwrap();
        let a = assess(&db, dir.path(), Some(&in_use), NAME, &no_vault())
            .await
            .unwrap();
        assert!(a.deletable());
    }

    #[tokio::test]
    async fn the_only_copy_of_the_key_in_use_stays() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let k = key();
        file(dir.path(), NAME, &format!("encryption_secret = \"{k}\"\n"));
        let a = assess(&db, dir.path(), Some(&k), NAME, &no_vault())
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::OnlyKeyCopy));

        // The live config.toml keeps it, read now from the disk: deletable.
        file(
            dir.path(),
            "config.toml",
            &format!("encryption_secret = \"{k}\"\n"),
        );
        let a = assess(&db, dir.path(), Some(&k), NAME, &no_vault())
            .await
            .unwrap();
        assert!(a.deletable(), "{:?}", a.blocker);

        // The key changed since: the copy is gone, the file is needed again.
        file(
            dir.path(),
            "config.toml",
            &format!("encryption_secret = \"{}\"\n", key()),
        );
        let a = assess(&db, dir.path(), Some(&k), NAME, &no_vault())
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::OnlyKeyCopy));
        assert!(delete(&db, dir.path(), Some(&k), NAME, &no_vault())
            .await
            .is_err());
        assert!(dir.path().join(NAME).exists());
    }

    #[test]
    fn files_lists_only_regular_set_aside_files() {
        let dir = tempfile::tempdir().unwrap();
        file(dir.path(), "config.toml.corrupt.2", "");
        file(dir.path(), "config.toml.corrupt.1", "");
        file(dir.path(), "config.toml", "");
        file(dir.path(), "config.toml.retired-key.1", "");
        std::fs::create_dir(dir.path().join("config.toml.corrupt.3")).unwrap();
        assert_eq!(
            files(dir.path()),
            vec!["config.toml.corrupt.1", "config.toml.corrupt.2"]
        );
    }

    #[tokio::test]
    async fn a_sealed_copy_is_never_deletable_here() {
        let (dir, db, k) = verified().await;
        // The sealed naming of a config copy: ciphertext, no credential, no key
        // to see in it, and possibly the only copy of what it seals.
        let sealed_name = "config.toml.corrupt.20261001T000000.000001Z.enc";
        let sealed_text =
            crypto::encrypt(&valid_config(), &crypto::parse_secret(&k).unwrap()).unwrap();
        file(dir.path(), sealed_name, &sealed_text);
        assert!(files(dir.path()).contains(&sealed_name.to_string()));
        let a = assess(&db, dir.path(), Some(&k), sealed_name, &no_vault())
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::Sealed));
        assert!(delete(&db, dir.path(), Some(&k), sealed_name, &no_vault())
            .await
            .is_err());
        assert!(dir.path().join(sealed_name).exists());
        assert_eq!(Blocker::Sealed.code(), "sealed");
    }

    /// `valid_config` with `line` as the first line of its `[server]` table.
    fn in_server(line: &str) -> String {
        valid_config().replace("[server]\n", &format!("[server]\n{line}\n"))
    }

    /// `valid_config` with `extra` before it (top-level lines, or tables of
    /// their own).
    fn with(extra: &str) -> String {
        format!("{extra}\n{}", valid_config())
    }

    #[tokio::test]
    async fn a_secret_of_a_place_the_store_cannot_hold_is_never_verified() {
        let (dir, db, k) = verified().await;
        // The base is deletable.
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &valid_config()).await;
        assert_eq!(blocker, None);

        let cases = [
            (
                "server.failure_notify_url",
                in_server("failure_notify_url = \"https://hooks.example/T/B/s3cr3t\""),
            ),
            (
                "secret_themes.matrix",
                with("[secret_themes]\nmatrix = \"theme-secret-0007\""),
            ),
        ];
        for (place, text) in cases {
            let (blocker, creds) = blocker_of(dir.path(), &db, &k, &text).await;
            assert_eq!(blocker, Some(Blocker::Unverified(1)), "{place}");
            let unverified: Vec<_> = creds.iter().filter(|(_, ok)| !ok).collect();
            assert_eq!(unverified.len(), 1, "{place}");
            assert_eq!(unverified[0].0.kind, KIND_CONFIG_SECRET, "{place}");
            // The place is named, the value is never carried by the name.
            assert_eq!(unverified[0].0.name, place);
        }
    }

    #[tokio::test]
    async fn a_value_at_an_unknown_place_blocks_whatever_it_is_called() {
        let (dir, db, k) = verified().await;
        // Settings the config model knows hold no secret do not block.
        let text = format!(
            "language = \"fr\"\n{}",
            in_server("port = 8080\ndebug_mode = true\nhost = \"127.0.0.1\"")
        );
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, None);

        // The three of the review, and what else a name cannot tell.
        let cases = [
            with("slack_webhook = \"https://hooks.slack.com/services/T0/B0/abcdef\""),
            with("database_url = \"postgres://u:pw@h/db\""),
            with("GH_PAT = \"ghp_0123456789abcdef\""),
            with("future_flag = true"),
            with("future_count = 7"),
            with("[extras]\nlabel = \"harmless-looking\""),
            with("[deep.nested]\npassword = \"hunter2-0010\""),
            with("[webhook.headers]\nAuthorization = \"Bearer abc123\""),
            with("[tokens]\nfuture = \"sk-future-0009\""),
            with("[tokens]\nanthropic = [\"sk-in-an-array-0011\"]"),
            in_server("api_key = \"sk-in-the-server-0012\""),
            valid_config().replace("active = true", "refresh_token = \"r-0013\""),
        ];
        for (n, text) in cases.iter().enumerate() {
            assert!(text.parse::<toml::Table>().is_ok(), "{n}: {text}");
            let (blocker, creds) = blocker_of(dir.path(), &db, &k, text).await;
            assert_eq!(blocker, Some(Blocker::UnknownValue(1)), "{n}: {text}");
            // The known credentials of the file are verified: only the
            // unknown place blocks.
            assert!(creds.iter().all(|(_, ok)| *ok), "{n}");
        }
        assert_eq!(Blocker::UnknownValue(1).code(), "unknown_value");
    }

    #[tokio::test]
    async fn a_value_of_the_wrong_type_blocks() {
        let (dir, db, k) = verified().await;
        let cases = [
            // A text where the model has a number or a flag.
            in_server("port = \"sk-live-abc\""),
            in_server("debug_mode = \"sk-live-abc\""),
            in_server("max_concurrent_agents = [1]"),
            // A number where the model has a text.
            with("language = 42"),
            // A non-text value at a credential place.
            valid_config().replace(&format!("auth_token = \"{TOKEN}\""), "auth_token = 12345"),
            valid_config().replace(&format!("value = \"{FILE_KEY}\""), "value = 42"),
            valid_config().replace(
                &format!("value = \"{FILE_KEY}\""),
                "value = [\"sk-in-an-array\"]",
            ),
            with("[tokens]\nopenai = true"),
            // An address that carries user info, a query or a fragment.
            with("[comfyui]\nendpoint = \"https://user:pw@127.0.0.1:8188\""),
            with("[agents.ollama]\nbase_url = \"https://h.example/v1?api_key=abc\""),
            with("embed_allowed_origins = [\"https://h.example/#token=abc\"]"),
        ];
        for (n, text) in cases.iter().enumerate() {
            assert!(text.parse::<toml::Table>().is_ok(), "{n}: {text}");
            let (blocker, _) = blocker_of(dir.path(), &db, &k, text).await;
            assert_eq!(blocker, Some(Blocker::UnknownValue(1)), "{n}: {text}");
        }
        // A plain address is a setting.
        let text = with("[comfyui]\nendpoint = \"http://127.0.0.1:8188\"");
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, None);
    }

    #[tokio::test]
    async fn an_encryption_secret_is_skipped_only_when_it_is_the_key_the_file_holds() {
        let (dir, db, k) = verified().await;
        let old = key();

        // A valid key is the key checks' business: nothing needs it, it goes.
        let (blocker, _) = blocker_of(
            dir.path(),
            &db,
            &k,
            &with(&format!("encryption_secret = \"{old}\"")),
        )
        .await;
        assert_eq!(blocker, None);

        // What is not a valid key is a value no check covers.
        let cases = [
            "encryption_secret = \"correct horse battery staple\"".to_string(),
            "encryption_secret = 12345".to_string(),
            // A damaged key: one digit short.
            format!("encryption_secret = \"{}\"", &old[..old.len() - 1]),
            format!("encryption_secret = \"{old}zz\""),
        ];
        for extra in cases {
            let (blocker, _) = blocker_of(dir.path(), &db, &k, &with(&extra)).await;
            assert_eq!(blocker, Some(Blocker::UnknownValue(1)), "{extra}");
        }

        // A key somewhere else than the top of the file is not the file's key.
        let text = in_server(&format!("encryption_secret = \"{old}\""));
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, Some(Blocker::UnknownValue(1)));

        // Two lines of a broken file: only the one the key checks read is
        // skipped, the other one is a secret nothing covers.
        let second = key();
        let text = format!(
            "encryption_secret = \"{old}\"\nencryption_secret = \"{second}\"\n{}",
            valid_config()
        );
        assert!(text.parse::<toml::Table>().is_err());
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, Some(Blocker::UnknownValue(1)));
    }

    #[test]
    fn the_live_list_keeps_only_what_the_store_can_hold() {
        let text = in_server("failure_notify_url = \"https://hooks.example/x\"");
        assert!(scan(&text)
            .credentials
            .iter()
            .any(|c| c.kind == KIND_CONFIG_SECRET));
        assert_eq!(clear_credentials_in(&text).len(), 2);
    }

    /// The closed list follows the config model: everything a fully set
    /// `AppConfig` writes is known, but its credentials.
    #[test]
    fn the_closed_list_covers_the_config_model() {
        use crate::models::{
            AgentType, ApiKey, ComfyUiConfig, MediaModality, MediaReferenceLimit, ModelTierConfig,
        };
        let mut cfg = config::default_config();
        let some = |s: &str| Some(s.to_string());
        cfg.server.domain = some("kronn.local");
        cfg.server.pseudo = some("owner");
        cfg.server.avatar_email = some("owner@example.test");
        cfg.server.bio = some("a bio");
        cfg.server.global_context = some("a context");
        cfg.server.timezone = some("Europe/Paris");
        cfg.server.frontend_origins = vec!["http://kronn.lan:5173".into()];
        cfg.server
            .ollama_context_overrides
            .insert("qwen3.8:27b-mlx".into(), 32768);
        cfg.server.agent_handoff_blocked_agents = vec![AgentType::Codex];
        cfg.stt_model = some("onnx-community/whisper-tiny");
        cfg.tts_voices.insert("fr".into(), "voice-fr".into());
        cfg.disabled_agents = vec![AgentType::Kiro];
        cfg.unlocked_profiles = vec!["batman".into()];
        cfg.disabled_auto_skills = vec!["a-skill".into()];
        cfg.embed_allowed_origins = vec!["https://embed.example".into()];
        cfg.audit_reverify_cron = some("0 3 * * *");
        cfg.audit_briefing_docs = true;
        cfg.scan.default_docs_dir = some("docs");
        cfg.agents.claude_code.path = some("/usr/bin/claude");
        cfg.agents.claude_code.version = some("1.2.3");
        cfg.agents.claude_code.concurrency = Some(2);
        cfg.agents.claude_code.mention_color = some("#112233");
        cfg.agents.lite_llm.base_url = some("http://127.0.0.1:4000");
        cfg.agents.model_tiers.codex = ModelTierConfig {
            economy: some("m1"),
            default: some("m2"),
            reasoning: some("m3"),
            decision: some("m4"),
            economy_effort: some("low"),
            default_effort: some("medium"),
            reasoning_effort: some("high"),
        };
        cfg.comfyui = ComfyUiConfig {
            endpoint: some("http://127.0.0.1:8188"),
            image_model: some("a.safetensors"),
            video_model: some("b.safetensors"),
            image_family: Some(crate::core::comfyui::ComfyFamily::Sdxl),
            install_dir: some("/opt/comfy"),
            model_families: [(
                "checkpoints/a.safetensors".to_string(),
                crate::core::comfyui::ComfyFamily::Sdxl,
            )]
            .into(),
            image_settings: crate::core::comfyui::ComfyGenSettings {
                steps: Some(20),
                cfg: Some(4.5),
                sampler: some("euler"),
                scheduler: some("simple"),
                denoise: Some(0.5),
            },
            video_settings: crate::core::comfyui::ComfyGenSettings {
                steps: Some(10),
                ..Default::default()
            },
        };
        cfg.media_reference_limits = vec![MediaReferenceLimit {
            connection_id: "c".into(),
            endpoint: "http://127.0.0.1:8188".into(),
            workflow: some("w"),
            model: "m".into(),
            modality: MediaModality::Image,
            max_input_references: 3,
        }];
        cfg.tokens.disabled_overrides = vec!["anthropic".into()];
        cfg.tokens.keys = vec![ApiKey {
            id: "a".into(),
            name: "Anthropic".into(),
            provider: "anthropic".into(),
            value: FILE_KEY.into(),
            active: true,
        }];

        let text = toml::to_string_pretty(&cfg).unwrap();
        let scanned = scan(&text);
        assert_eq!(scanned.unparsed, 0);
        // The only credential a default config carries is the provider key.
        assert_eq!(scanned.credentials.len(), 1, "{:?}", scanned.credentials);

        // Name every place the list does not know, for the failure message.
        fn leaves(path: &mut Vec<String>, v: &toml::Value, out: &mut Vec<String>) {
            match v {
                toml::Value::Table(t) => t.iter().for_each(|(k, v)| {
                    path.push(k.clone());
                    leaves(path, v, out);
                    path.pop();
                }),
                toml::Value::Array(items) => items.iter().for_each(|v| {
                    path.push("[]".into());
                    leaves(path, v, out);
                    path.pop();
                }),
                leaf => {
                    let names: Vec<&str> = path.iter().map(String::as_str).collect();
                    let value_of_a_key = names == ["tokens", "keys", "[]", "value"];
                    if !value_of_a_key && !non_secret_shape(&names).is_some_and(|s| s.fits(leaf)) {
                        out.push(path.join("."));
                    }
                }
            }
        }
        let mut unknown = Vec::new();
        let table: toml::Table = text.parse().unwrap();
        leaves(&mut Vec::new(), &toml::Value::Table(table), &mut unknown);
        assert!(
            unknown.is_empty(),
            "places the closed list lacks: {unknown:?}"
        );
        assert_eq!(scanned.unknown, 0);
    }

    #[tokio::test]
    async fn a_file_that_is_not_valid_toml_is_read_line_by_line() {
        let (dir, db, k) = verified().await;

        // Tables defined twice: invalid TOML, but every line reads on its own,
        // under the section it sat in.
        let twice = format!("{}\n[server]\nport = 8080\n", valid_config());
        assert!(toml::from_str::<toml::Table>(&twice).is_err());
        let (blocker, creds) = blocker_of(dir.path(), &db, &k, &twice).await;
        assert_eq!(blocker, None, "{creds:?}");
        assert_eq!(creds.len(), 2);

        // A secret at an unknown place is judged by the place, not the name.
        let text = format!("{twice}api_key = \"sk-line-0012\"\n");
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, Some(Blocker::UnknownValue(1)));
        let text = format!("{twice}slack_webhook = \"https://hooks.example/T/B/s\"\n");
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, Some(Blocker::UnknownValue(1)));
        // The webhook URL is a secret the store cannot hold.
        let text = format!("{twice}failure_notify_url = \"https://hooks.example/T/B/s\"\n");
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, Some(Blocker::Unverified(1)));

        // A line that does not parse blocks, quote or no quote.
        let unreadable = [
            // A value with no quote: no parser reads it, a secret sits in it.
            "api_key = sk-live-abc",
            "[broken\nlanguage = \"fr\"",
            // A header with no quote.
            "[extras",
            "[[tokens.keys",
            "not toml at all",
            // An unterminated quote.
            "password = \"never closed",
            // The lines of a multi-line string and of a multi-line array.
            "notes = \"\"\"\nsk-hidden-0013\n\"\"\"",
            "keys = [\n  \"sk-hidden-0014\",\n]",
        ];
        for extra in unreadable {
            let text = format!("{twice}{extra}\n");
            let (blocker, _) = blocker_of(dir.path(), &db, &k, &text).await;
            assert!(
                matches!(blocker, Some(Blocker::Unparsed(n)) if n >= 1),
                "{extra}: {blocker:?}"
            );
            assert!(delete(&db, dir.path(), Some(&k), NAME, &no_vault())
                .await
                .is_err());
            assert!(dir.path().join(NAME).exists());
        }

        // The credentials of a broken file are still listed, and verified: the
        // lines that do not read are what blocks.
        let text = format!(
            "[server\nauth_token = \"{TOKEN}\"\n[[tokens.keys\nvalue = \"{FILE_KEY}\"\nnot toml at all\n"
        );
        let (blocker, creds) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, Some(Blocker::Unparsed(3)));
        assert_eq!(creds.len(), 2);
        assert!(creds.iter().all(|(_, ok)| *ok));
    }

    /// An `.enc` file or locked rows only `old` opens: deleting the file that
    /// holds `old` would lose them.
    #[tokio::test]
    async fn a_key_still_needed_by_a_file_kept_aside_blocks_the_delete() {
        let (dir, db, in_use) = verified().await;
        let old = key();
        let with_old_key = format!("encryption_secret = \"{old}\"\n{}", valid_config());
        let (blocker, _) = blocker_of(dir.path(), &db, &in_use, &with_old_key).await;
        assert_eq!(blocker, None, "nothing needs the old key yet");

        let sealed_under =
            |k: &str, text: &str| crypto::encrypt(text, &crypto::parse_secret(k).unwrap()).unwrap();

        // Each key-bound file of the data dir, under the old key only.
        let cases: [(&str, String); 3] = [
            (
                crate::core::credential_store::BACKUP_FILENAME,
                sealed_under(&old, "config backup"),
            ),
            (
                crate::core::config_salvage::SHELTER_FILENAME,
                sealed_under(&old, "preserved values"),
            ),
            (
                "config.toml.corrupt.1.enc",
                sealed_under(&old, "a sealed copy"),
            ),
        ];
        for (name, content) in cases {
            file(dir.path(), name, &content);
            let a = assess(&db, dir.path(), Some(&in_use), NAME, &no_vault())
                .await
                .unwrap();
            assert_eq!(a.blocker, Some(Blocker::KeyBoundFiles(1)), "{name}");
            assert!(delete(&db, dir.path(), Some(&in_use), NAME, &no_vault())
                .await
                .is_err());
            assert!(dir.path().join(NAME).exists(), "{name}");
            // The same file under the key in use does not need the old key.
            file(dir.path(), name, &sealed_under(&in_use, "moved"));
            let a = assess(&db, dir.path(), Some(&in_use), NAME, &no_vault())
                .await
                .unwrap();
            assert!(a.deletable(), "{name}: {:?}", a.blocker);
            std::fs::remove_file(dir.path().join(name)).unwrap();
        }

        // Rows waiting in a locked-secrets file.
        let locked = |k: &str| {
            serde_json::to_string(&serde_json::json!([{
                "table": "stored_credentials",
                "column": "value_encrypted",
                "row": { "value_encrypted": sealed_under(k, "waiting") }
            }]))
            .unwrap()
        };
        let locked_name = format!("{}20261001T000000Z.json", keystore::LOCKED_SECRETS_PREFIX);
        file(dir.path(), &locked_name, &locked(&old));
        let a = assess(&db, dir.path(), Some(&in_use), NAME, &no_vault())
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::KeyBoundFiles(1)));
        file(dir.path(), &locked_name, &locked(&in_use));
        let a = assess(&db, dir.path(), Some(&in_use), NAME, &no_vault())
            .await
            .unwrap();
        assert!(a.deletable(), "{:?}", a.blocker);

        // A key-bound file that cannot be parsed is not proven harmless.
        file(dir.path(), &locked_name, "not json at all");
        let a = assess(&db, dir.path(), Some(&in_use), NAME, &no_vault())
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::KeyCheckUnavailable));
        assert!(delete(&db, dir.path(), Some(&in_use), NAME, &no_vault())
            .await
            .is_err());
        assert!(dir.path().join(NAME).exists());
    }

    /// A key-bound entry that cannot be read, or a data dir that cannot be
    /// listed, fails the check closed.
    #[test]
    fn an_unreadable_key_bound_entry_or_dir_fails_closed() {
        let (key, other) = (key(), key());
        // A missing data dir cannot be listed.
        let gone = tempfile::tempdir().unwrap();
        assert_eq!(
            key_bound_needing(&gone.path().join("nope"), &key, &other),
            Err(())
        );

        // An `.enc` entry that is not a readable file (a directory here).
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(key_bound_needing(dir.path(), &key, &other), Ok(0));
        std::fs::create_dir(dir.path().join("config.toml.corrupt.9.enc")).unwrap();
        assert_eq!(key_bound_needing(dir.path(), &key, &other), Err(()));

        // A file that is not UTF-8.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml.corrupt.9.enc"), [0xff, 0xfe]).unwrap();
        assert_eq!(key_bound_needing(dir.path(), &key, &other), Err(()));

        // A key that does not parse.
        assert_eq!(key_bound_needing(dir.path(), "not a key", &other), Err(()));
    }

    #[cfg(unix)] // asserts POSIX mode bits, which Windows does not have
    #[test]
    fn an_enc_file_without_read_permission_fails_closed() {
        use std::os::unix::fs::PermissionsExt;
        let (key, other) = (key(), key());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml.corrupt.9.enc");
        std::fs::write(&path, "x").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read(&path).is_ok(); // root ignores modes
        let got = key_bound_needing(dir.path(), &key, &other);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        if !readable {
            assert_eq!(got, Err(()));
        }
    }

    #[test]
    fn the_new_blocker_has_a_code_and_a_message() {
        assert_eq!(Blocker::KeyCheckUnavailable.code(), "key_check_unavailable");
        assert!(!Blocker::KeyCheckUnavailable.message().is_empty());
    }

    #[cfg(unix)] // creates symlinks through std::os::unix; makes hard links and reads unix inode metadata
    #[tokio::test]
    async fn a_linked_name_is_never_read_or_unlinked() {
        let (dir, db, k) = verified().await;
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("elsewhere.toml");
        std::fs::write(&target, valid_config()).unwrap();

        // A symlink under the set-aside name: the target is not read.
        std::os::unix::fs::symlink(&target, dir.path().join(NAME)).unwrap();
        let a = assess(&db, dir.path(), Some(&k), NAME, &no_vault())
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unreadable));
        assert!(a.credentials.is_empty());
        assert!(delete(&db, dir.path(), Some(&k), NAME, &no_vault())
            .await
            .is_err());
        assert!(target.exists());
        assert!(std::fs::symlink_metadata(dir.path().join(NAME)).is_ok());
        std::fs::remove_file(dir.path().join(NAME)).unwrap();

        // A hard link: the file is also reachable from elsewhere.
        file(dir.path(), NAME, &valid_config());
        std::fs::hard_link(dir.path().join(NAME), outside.path().join("alias")).unwrap();
        let a = assess(&db, dir.path(), Some(&k), NAME, &no_vault())
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unreadable));
    }
}
