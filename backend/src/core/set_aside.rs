//! The `config.toml.corrupt.<ts>` files Kronn keeps when it cannot read a
//! `config.toml` (KT-1594). They may hold credentials, and the key, in clear.
//!
//! Kronn never deletes one by itself. The owner may delete one here only when
//! Kronn can prove that nothing in it is lost. Anything unknown, unparsable
//! or sealed blocks the delete, with a reason the screen shows:
//!
//! - a sealed copy (`.enc`) is never deleted here: it may be the only copy;
//! - every credential-looking value of the file (the known fields, and any
//!   place [`config_salvage`](crate::core::config_salvage) would treat as a
//!   credential, plus the webhook URL and the theme secrets) must be in
//!   `stored_credentials` and decrypt with the key in use; the places the
//!   encrypted store cannot hold are never verified;
//! - in a file that is not valid TOML, a line the parser cannot read and that
//!   holds a quote could hide a secret: it blocks;
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

use crate::core::config_salvage::whole_credential;
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
    /// This many credentials are not in `stored_credentials`.
    Unverified(usize),
    /// The file holds a key that still opens this many rows the key in use
    /// does not.
    KeyNeeded(usize),
    /// The file holds a key that still opens this many key-bound files (or
    /// their rows) the key in use does not.
    KeyBoundFiles(usize),
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
            Blocker::Unverified(_) => "unverified",
            Blocker::KeyNeeded(_) => "key_needed",
            Blocker::KeyBoundFiles(_) => "key_bound_files",
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
    credentials: Vec<ClearCredential>,
    /// Lines of a file that is not valid TOML that cannot be read on their own
    /// and hold a quote: a secret could sit in them.
    unparsed: usize,
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

    /// Every text under `value` is a credential of the place `name`.
    fn leaves(&mut self, name: &str, value: &toml::Value) {
        match value {
            toml::Value::String(text) => self.push(ClearCredential {
                kind: KIND_CONFIG_SECRET,
                name: name.to_string(),
                provider: String::new(),
                value: text.clone(),
            }),
            toml::Value::Array(items) => items.iter().for_each(|v| self.leaves(name, v)),
            toml::Value::Table(table) => table
                .iter()
                .for_each(|(k, v)| self.leaves(&format!("{name}.{k}"), v)),
            _ => {}
        }
    }

    /// The value at `path`: the known fields are read by [`known_fields`], any
    /// other place [`whole_credential`] judges a credential, the webhook URL
    /// and the theme secrets are config secrets; the rest is walked.
    fn walk(&mut self, path: &mut Vec<String>, value: &toml::Value) {
        let names: Vec<&str> = path.iter().map(String::as_str).collect();
        match names.as_slice() {
            ["server", "auth_token"]
            | ["tokens", "anthropic" | "openai" | "google"]
            | ["encryption_secret"] => return,
            _ => {}
        }
        let here = path.join(".");
        if names == ["tokens", "keys"] {
            self.token_keys(&here, value);
            return;
        }
        let tokens_root = names == ["tokens"];
        let named =
            names.last() == Some(&"failure_notify_url") || names.first() == Some(&"secret_themes");
        if tokens_root && !matches!(value, toml::Value::Table(_)) {
            self.leaves(&here, value);
            return;
        }
        if !tokens_root && (named || whole_credential(path, value)) {
            self.leaves(&here, value);
            return;
        }
        match value {
            toml::Value::Table(table) => {
                for (k, v) in table {
                    path.push(k.clone());
                    self.walk(path, v);
                    path.pop();
                }
            }
            toml::Value::Array(items) => {
                for v in items {
                    path.push("[]".into());
                    self.walk(path, v);
                    path.pop();
                }
            }
            _ => {}
        }
    }

    /// `tokens.keys`: the `value` of each entry is a provider key; any other
    /// text of an entry that is not a label is a config secret.
    fn token_keys(&mut self, here: &str, value: &toml::Value) {
        let toml::Value::Array(items) = value else {
            self.leaves(here, value);
            return;
        };
        for item in items {
            let toml::Value::Table(entry) = item else {
                self.leaves(here, item);
                continue;
            };
            let text = |k: &str| {
                entry
                    .get(k)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_default()
            };
            self.push(ClearCredential {
                kind: KIND_PROVIDER_KEY,
                name: text("name"),
                provider: text("provider"),
                value: text("value"),
            });
            for (k, v) in entry {
                if !matches!(
                    k.as_str(),
                    "id" | "name" | "provider" | "value" | "active" | "position"
                ) {
                    let mut path =
                        vec!["tokens".to_string(), "keys".into(), "[]".into(), k.clone()];
                    self.walk(&mut path, v);
                }
            }
        }
    }

    /// The known single fields of a config table.
    fn known_fields(&mut self, table: &toml::Table) {
        let text = |v: Option<&toml::Value>| {
            v.and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_default()
        };
        self.push(ClearCredential {
            kind: KIND_AUTH_TOKEN,
            name: String::new(),
            provider: String::new(),
            value: text(table.get("server").and_then(|s| s.get("auth_token"))),
        });
        let tokens = table.get("tokens");
        // The single-key fields of before the key list.
        for provider in ["anthropic", "openai", "google"] {
            self.push(ClearCredential {
                kind: KIND_PROVIDER_KEY,
                name: String::new(),
                provider: provider.into(),
                value: text(tokens.and_then(|t| t.get(provider))),
            });
        }
    }

    fn table(&mut self, table: &toml::Table) {
        self.known_fields(table);
        for (k, v) in table {
            self.walk(&mut vec![k.clone()], v);
        }
    }

    /// One line of a file that is not valid TOML, read on its own, outside
    /// the section it sat in: a field that could be a credential is listed.
    fn line(&mut self, one: &toml::Table) {
        for (k, v) in one {
            let known = match k.as_str() {
                "auth_token" => Some(KIND_AUTH_TOKEN),
                "value" | "anthropic" | "openai" | "google" => Some(KIND_PROVIDER_KEY),
                _ => None,
            };
            match (known, v.as_str()) {
                (Some(kind), Some(text)) => self.push(ClearCredential {
                    kind,
                    name: String::new(),
                    provider: if matches!(k.as_str(), "anthropic" | "openai" | "google") {
                        k.clone()
                    } else {
                        String::new()
                    },
                    value: text.to_string(),
                }),
                _ => self.walk(&mut vec![k.clone()], v),
            }
        }
    }
}

/// Everything credential-looking in a config.toml text, and what could not be
/// read. A text that is not valid TOML gives what its readable sections hold,
/// plus every line that parses on its own (a field listed there may not be a
/// credential: a file is only deletable when no value of it can be missing).
fn scan(text: &str) -> Scan {
    let mut out = Scan::default();
    if let Ok(table) = text.parse::<toml::Table>() {
        out.table(&table);
        return out;
    }
    if let Some(table) =
        crate::core::config_salvage::salvage_sections(text).and_then(|t| t.parse().ok())
    {
        out.table(&table);
    }
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.parse::<toml::Table>() {
            Ok(one) => out.line(&one),
            // `[server`, `not toml at all`: no text to hide a secret in.
            Err(_) if !line.contains(['"', '\'']) => {}
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
/// config backup, the preserved values, sealed copies). One that cannot be
/// read counts: nothing proves it is not bound to `other`.
fn key_bound_needing(dir: &Path, key: &str, other: &str) -> usize {
    let (Ok(key), Ok(other)) = (crypto::parse_secret(key), crypto::parse_secret(other)) else {
        return 0;
    };
    let opens = |cipher: &str| {
        let cipher = cipher.trim();
        crypto::decrypt(cipher, &other).is_ok() && crypto::decrypt(cipher, &key).is_err()
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut needing = 0;
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let locked = name.starts_with(keystore::LOCKED_SECRETS_PREFIX) && name.ends_with(".json");
        if !locked && !sealed(&name) {
            continue;
        }
        let Some(text) = rooted_io::read_capped(dir, Path::new(&name), MAX_BOUND_FILE)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
        else {
            needing += 1;
            continue;
        };
        if !locked {
            needing += usize::from(opens(&text));
            continue;
        }
        let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
            needing += 1;
            continue;
        };
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
    needing
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
    let unparsed = scanned.unparsed;
    let credentials = verify(db, key, scanned.credentials).await?;
    let file_key = key_in(&text);
    let unverified = credentials.iter().filter(|(_, ok)| !ok).count();
    let blocker = match key {
        None => Some(Blocker::KeyLocked),
        Some(_) if unparsed > 0 => Some(Blocker::Unparsed(unparsed)),
        Some(_) if unverified > 0 => Some(Blocker::Unverified(unverified)),
        Some(key) => match file_key.as_deref() {
            None => None,
            Some(k) if crate::core::keyvault::same_key(k, key) => {
                (!key_kept_elsewhere(dir, store, key)).then_some(Blocker::OnlyKeyCopy)
            }
            Some(k) => match keystore::rows_only_this_key_opens(db, key, k).await? {
                0 => match key_bound_needing(dir, key, k) {
                    0 => None,
                    n => Some(Blocker::KeyBoundFiles(n)),
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

    #[tokio::test]
    async fn a_secret_the_store_cannot_hold_blocks_the_delete() {
        let (dir, db, k) = verified().await;
        // The base is deletable.
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &valid_config()).await;
        assert_eq!(blocker, None);

        let cases = [
            (
                "failure_notify_url",
                "[server]\nfailure_notify_url = \"https://hooks.example/T/B/s3cr3t\"\n",
            ),
            (
                "secret_themes.matrix",
                "[secret_themes]\nmatrix = \"theme-secret-0007\"\n",
            ),
            (
                "extras.api_secret",
                "[extras]\napi_secret = \"future-secret-0008\"\n",
            ),
            (
                "webhook.headers.authorization",
                "[webhook.headers]\nAuthorization = \"Bearer abc123\"\n",
            ),
            ("tokens.future", "[tokens]\nfuture = \"sk-future-0009\"\n"),
            (
                "deep.nested.password",
                "[deep.nested]\npassword = \"hunter2-0010\"\n",
            ),
        ];
        for (place, extra) in cases {
            let text = format!("{}\n{extra}", valid_config());
            let (blocker, creds) = blocker_of(dir.path(), &db, &k, &text).await;
            assert_eq!(blocker, Some(Blocker::Unverified(1)), "{place}");
            let unverified: Vec<_> = creds.iter().filter(|(_, ok)| !ok).collect();
            assert_eq!(unverified.len(), 1, "{place}");
            assert_eq!(unverified[0].0.kind, KIND_CONFIG_SECRET, "{place}");
            // The place is named, the value is never carried by the name.
            assert!(
                unverified[0].0.name.eq_ignore_ascii_case(place),
                "{place}: {}",
                unverified[0].0.name
            );
        }

        // A plain setting is not a credential.
        let text = format!("language = \"fr\"\nmax_tokens = 100\n{}", valid_config());
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, None);

        // An extra text in a provider key entry that is not a label.
        let text = valid_config().replace("active = true", "refresh_token = \"r-0011\"");
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, Some(Blocker::Unverified(1)));
    }

    #[test]
    fn the_live_list_keeps_only_what_the_store_can_hold() {
        let text = format!(
            "{}\n[server]\nfailure_notify_url = \"https://hooks.example/x\"\n",
            valid_config()
        );
        assert!(scan(&text)
            .credentials
            .iter()
            .any(|c| c.kind == KIND_CONFIG_SECRET));
        assert_eq!(clear_credentials_in(&text).len(), 2);
    }

    #[tokio::test]
    async fn a_broken_file_is_read_line_by_line_and_what_it_cannot_read_blocks() {
        let (dir, db, k) = verified().await;

        // Credentials of a broken file, verified: deletable. Lines with no
        // text in them (a cut header, prose) hide nothing.
        let text = format!("[server\nauth_token = \"{TOKEN}\"\n[[tokens.keys\nvalue = \"{FILE_KEY}\"\nnot toml at all\n");
        let (blocker, creds) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, None, "{creds:?}");
        assert_eq!(creds.len(), 2);

        // A credential-looking line of a broken file, not stored.
        let text = format!("{text}failure_notify_url = \"https://hooks.example/T/B/s3cr3t\"\n");
        let (blocker, _) = blocker_of(dir.path(), &db, &k, &text).await;
        assert_eq!(blocker, Some(Blocker::Unverified(1)));
        let text = "[broken\napi_key = \"sk-line-0012\"\n";
        let (blocker, _) = blocker_of(dir.path(), &db, &k, text).await;
        assert_eq!(blocker, Some(Blocker::Unverified(1)));

        // An unterminated quote.
        let text = "[broken\npassword = \"never closed\n";
        let (blocker, _) = blocker_of(dir.path(), &db, &k, text).await;
        assert_eq!(blocker, Some(Blocker::Unparsed(1)));

        // A multi-line string: its opening line cannot be read alone.
        let text = "[broken\nnotes = \"\"\"\nsk-hidden-0013\n\"\"\"\n";
        let (blocker, _) = blocker_of(dir.path(), &db, &k, text).await;
        assert!(
            matches!(blocker, Some(Blocker::Unparsed(n)) if n >= 1),
            "{blocker:?}"
        );

        // A multi-line array: the elements are lines of their own.
        let text = "[broken\nkeys = [\n  \"sk-hidden-0014\",\n]\n";
        let (blocker, _) = blocker_of(dir.path(), &db, &k, text).await;
        assert!(
            matches!(blocker, Some(Blocker::Unparsed(n)) if n >= 1),
            "{blocker:?}"
        );
        assert!(delete(&db, dir.path(), Some(&k), NAME, &no_vault())
            .await
            .is_err());
        assert!(dir.path().join(NAME).exists());
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

        // A key-bound file that cannot be read is not proven harmless.
        file(dir.path(), &locked_name, "not json at all");
        let a = assess(&db, dir.path(), Some(&in_use), NAME, &no_vault())
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::KeyBoundFiles(1)));
    }

    #[cfg(unix)]
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
