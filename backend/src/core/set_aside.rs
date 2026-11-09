//! The `config.toml.corrupt.<ts>` files Kronn keeps when it cannot read a
//! `config.toml` (KT-1594). They may hold credentials, and the key, in clear.
//!
//! Kronn never deletes one by itself. The owner may delete one here once
//! every credential it carries is verified: the same value is in
//! `stored_credentials` and decrypts with the key in use. A file that cannot
//! be read, one whose key still opens rows the key in use does not, or one
//! that holds the only copy of the key in use, is not deletable: any of those
//! could be the last copy of something.
//!
//! Values never leave this module: the API only gets names and verdicts.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};

use crate::core::{config, crypto};
use crate::db::stored_credentials::{self as rows, KIND_AUTH_TOKEN, KIND_PROVIDER_KEY};
use crate::db::Database;

/// The prefix of a kept `config.toml`.
pub const PREFIX: &str = "config.toml.corrupt.";

/// A credential a config text carries in clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearCredential {
    /// `provider_key` or `auth_token`, as in `stored_credentials`.
    pub kind: &'static str,
    pub name: String,
    pub provider: String,
    pub value: String,
}

/// Why a set-aside file may not be deleted. The code is what the UI maps to a
/// sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// The file cannot be read (not text, or an I/O error).
    Unreadable,
    /// No key in use: nothing can be verified.
    KeyLocked,
    /// This many credentials are not in `stored_credentials`.
    Unverified(usize),
    /// The file holds a key that still opens this many rows the key in use
    /// does not.
    KeyNeeded(usize),
    /// The file holds the only copy of the key in use.
    OnlyKeyCopy,
}

impl Blocker {
    pub fn code(&self) -> &'static str {
        match self {
            Blocker::Unreadable => "unreadable",
            Blocker::KeyLocked => "key_locked",
            Blocker::Unverified(_) => "unverified",
            Blocker::KeyNeeded(_) => "key_needed",
            Blocker::OnlyKeyCopy => "only_key_copy",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Blocker::Unreadable => "The file cannot be read, so its credentials cannot be \
                                    verified."
                .into(),
            Blocker::KeyLocked => "No encryption key is in use: restore it first, nothing can be \
                                   verified while it is locked."
                .into(),
            Blocker::Unverified(n) => format!(
                "{n} credential(s) in the file are not in the encrypted store (or differ from \
                 it): re-enter them in Kronn first."
            ),
            Blocker::KeyNeeded(n) => format!(
                "The file holds an encryption key that still opens {n} stored row(s) the key in \
                 use does not: restore or re-encrypt them first."
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

/// The credentials a config.toml text carries in clear. A text that is not
/// valid TOML gives what its readable sections hold, plus every line that
/// looks like a credential on its own (such a line is listed even when it
/// is not one: a file is only deletable when no value of it is missing).
pub fn clear_credentials_in(text: &str) -> Vec<ClearCredential> {
    let strict = text.parse::<toml::Table>().ok();
    let valid = strict.is_some();
    let table = strict.or_else(|| {
        crate::core::config_salvage::salvage_sections(text).and_then(|t| t.parse().ok())
    });
    let mut out: Vec<ClearCredential> = Vec::new();
    let mut push = |cred: ClearCredential| {
        if !cred.value.is_empty()
            && !out
                .iter()
                .any(|c| c.kind == cred.kind && c.value == cred.value)
        {
            out.push(cred);
        }
    };
    let text_of = |v: Option<&toml::Value>| {
        v.and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default()
    };
    if let Some(table) = &table {
        push(ClearCredential {
            kind: KIND_AUTH_TOKEN,
            name: String::new(),
            provider: String::new(),
            value: text_of(table.get("server").and_then(|s| s.get("auth_token"))),
        });
        let tokens = table.get("tokens");
        for key in tokens
            .and_then(|t| t.get("keys"))
            .and_then(|k| k.as_array())
            .into_iter()
            .flatten()
        {
            push(ClearCredential {
                kind: KIND_PROVIDER_KEY,
                name: text_of(key.get("name")),
                provider: text_of(key.get("provider")),
                value: text_of(key.get("value")),
            });
        }
        // The single-key fields of before the key list.
        for provider in ["anthropic", "openai", "google"] {
            push(ClearCredential {
                kind: KIND_PROVIDER_KEY,
                name: String::new(),
                provider: provider.into(),
                value: text_of(tokens.and_then(|t| t.get(provider))),
            });
        }
    }
    if !valid {
        for line in text.lines() {
            let Ok(one) = line.trim().parse::<toml::Table>() else {
                continue;
            };
            for field in ["auth_token", "value", "anthropic", "openai", "google"] {
                push(ClearCredential {
                    kind: if field == "auth_token" {
                        KIND_AUTH_TOKEN
                    } else {
                        KIND_PROVIDER_KEY
                    },
                    name: String::new(),
                    provider: if matches!(field, "anthropic" | "openai" | "google") {
                        field.into()
                    } else {
                        String::new()
                    },
                    value: text_of(one.get(field)),
                });
            }
        }
    }
    out
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
/// no key in use).
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

/// Read a set-aside file of `dir` and decide whether it may be deleted.
/// `key_copy_elsewhere`: a vault or config.toml keeps the key in use.
pub async fn assess(
    db: &Database,
    dir: &Path,
    key: Option<&str>,
    name: &str,
    key_copy_elsewhere: bool,
) -> Result<Assessment> {
    anyhow::ensure!(valid_name(name), "not a set-aside config.toml: {name}");
    let path = dir.join(name);
    let meta = std::fs::symlink_metadata(&path).context("read the set-aside file")?;
    anyhow::ensure!(meta.is_file(), "not a regular file: {name}");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Assessment {
            credentials: Vec::new(),
            holds_key: false,
            blocker: Some(Blocker::Unreadable),
        });
    };
    let credentials = verify(db, key, clear_credentials_in(&text)).await?;
    let file_key = key_in(&text);
    let unverified = credentials.iter().filter(|(_, ok)| !ok).count();
    let blocker = match key {
        None => Some(Blocker::KeyLocked),
        Some(_) if unverified > 0 => Some(Blocker::Unverified(unverified)),
        Some(key) => match file_key.as_deref() {
            None => None,
            Some(k) if crate::core::keyvault::same_key(k, key) => {
                (!key_copy_elsewhere).then_some(Blocker::OnlyKeyCopy)
            }
            Some(k) => match crate::core::keystore::rows_only_this_key_opens(db, key, k).await? {
                0 => None,
                n => Some(Blocker::KeyNeeded(n)),
            },
        },
    };
    Ok(Assessment {
        credentials,
        holds_key: file_key.is_some(),
        blocker,
    })
}

/// Delete a set-aside file, after checking it again: the verdict the screen
/// showed may be stale.
pub async fn delete(
    db: &Database,
    dir: &Path,
    key: Option<&str>,
    name: &str,
    key_copy_elsewhere: bool,
) -> Result<()> {
    let verdict = assess(db, dir, key, name, key_copy_elsewhere).await?;
    if let Some(blocker) = verdict.blocker {
        anyhow::bail!("{}", blocker.message());
    }
    std::fs::remove_file(dir.join(name)).context("delete the set-aside file")?;
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

    fn key() -> String {
        crypto::generate_secret()
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
        let name = "config.toml.corrupt.1";
        file(dir.path(), name, &valid_config());

        // Nothing stored: both are unverified.
        let a = assess(&db, dir.path(), Some(&k), name, true).await.unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unverified(2)));
        assert!(!a.deletable());
        assert!(delete(&db, dir.path(), Some(&k), name, true).await.is_err());
        assert!(dir.path().join(name).exists());

        // One of the two stored: still refused.
        store(&db, vec![row(KIND_PROVIDER_KEY, "a", FILE_KEY, &k)]).await;
        let a = assess(&db, dir.path(), Some(&k), name, true).await.unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unverified(1)));
        assert_eq!(a.credentials.iter().filter(|(_, ok)| *ok).count(), 1);

        // Stored, but under another key (or another value): not verified.
        store(&db, vec![row(KIND_AUTH_TOKEN, "server", TOKEN, &key())]).await;
        let a = assess(&db, dir.path(), Some(&k), name, true).await.unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unverified(1)));
        store(&db, vec![row(KIND_AUTH_TOKEN, "server", OTHER, &k)]).await;
        let a = assess(&db, dir.path(), Some(&k), name, true).await.unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unverified(1)));

        // Every credential verified: it goes.
        store(&db, vec![row(KIND_AUTH_TOKEN, "server", TOKEN, &k)]).await;
        let a = assess(&db, dir.path(), Some(&k), name, true).await.unwrap();
        assert!(a.deletable(), "{:?}", a.blocker);
        delete(&db, dir.path(), Some(&k), name, true).await.unwrap();
        assert!(!dir.path().join(name).exists());
    }

    #[tokio::test]
    async fn a_locked_key_verifies_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let name = "config.toml.corrupt.1";
        file(dir.path(), name, "language = \"fr\"\n");
        let a = assess(&db, dir.path(), None, name, true).await.unwrap();
        assert_eq!(a.blocker, Some(Blocker::KeyLocked));
        assert!(delete(&db, dir.path(), None, name, true).await.is_err());
        assert!(dir.path().join(name).exists());
    }

    #[tokio::test]
    async fn an_unreadable_file_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let name = "config.toml.corrupt.1";
        std::fs::write(dir.path().join(name), [0xff, 0xfe, 0x00, 0x9f]).unwrap();
        let a = assess(&db, dir.path(), Some(&key()), name, true)
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::Unreadable));
    }

    #[tokio::test]
    async fn the_key_it_holds_is_not_lost_with_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let (in_use, old) = (key(), key());
        let name = "config.toml.corrupt.1";
        file(
            dir.path(),
            name,
            &format!("encryption_secret = \"{old}\"\nlanguage = \"fr\"\n"),
        );

        // A row only the old key opens: the file is the way back to it.
        store(&db, vec![row(KIND_PROVIDER_KEY, "x", "locked", &old)]).await;
        let a = assess(&db, dir.path(), Some(&in_use), name, true)
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::KeyNeeded(1)));
        assert!(a.holds_key);

        // No row needs it any more.
        db.with_conn(rows::delete_all).await.unwrap();
        let a = assess(&db, dir.path(), Some(&in_use), name, true)
            .await
            .unwrap();
        assert!(a.deletable());
    }

    #[tokio::test]
    async fn the_only_copy_of_the_key_in_use_stays() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let k = key();
        let name = "config.toml.corrupt.1";
        file(dir.path(), name, &format!("encryption_secret = \"{k}\"\n"));
        let a = assess(&db, dir.path(), Some(&k), name, false)
            .await
            .unwrap();
        assert_eq!(a.blocker, Some(Blocker::OnlyKeyCopy));
        let a = assess(&db, dir.path(), Some(&k), name, true).await.unwrap();
        assert!(a.deletable());
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
}
