//! Migration of a 0.14.2 `config.toml` into the encrypted credential store,
//! on real files in a temporary data directory.

use super::*;
use crate::core::keystore::{self, KeyOutcome};
use crate::core::keyvault::{KeyStore, SidecarFile};
use crate::core::{config, crypto};
use serial_test::serial;

const ANTHROPIC: &str = "sk-ant-api03-fixture-value-0001";
const CONNECTION: &str = "nvapi-fixture-connection-value-0002";
const AUTH_TOKEN: &str = "6f1c1d2e-auth-token-fixture-0003";
const MCP_SECRET: &str = "ghp_fixtureMcpToken0004";
const SNAPSHOT_SECRET: &str = "snapshot-fixture-value-0005";

/// Points `KRONN_DATA_DIR` at a fresh directory for one test and forgets the
/// process-wide store/key state for it afterwards.
struct DataDir {
    dir: tempfile::TempDir,
    previous: Option<String>,
}

impl DataDir {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var("KRONN_DATA_DIR").ok();
        std::env::set_var("KRONN_DATA_DIR", dir.path());
        std::env::remove_var(crate::core::keyvault::ENV_KEK);
        Self { dir, previous }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// What a restart forgets: the armed store and the retained disk key.
    fn restart(&self) {
        disarm(self.path());
        config::release_disk_key(self.path());
    }

    fn config_text(&self) -> String {
        std::fs::read_to_string(self.path().join("config.toml")).unwrap()
    }

    /// Two durable copies, as on macOS (keychain + sidecar): a second
    /// sidecar file stands in for the keychain.
    fn sidecar(&self) -> KeyStore {
        let keychain = self.path().join("keychain-standin");
        KeyStore::from_vaults(vec![
            Box::new(SidecarFile::in_dir(&keychain)),
            Box::new(SidecarFile::in_dir(self.path())),
        ])
    }

    /// Sidecar only (Linux, WSL, Docker, dev builds).
    fn sidecar_only(&self) -> KeyStore {
        KeyStore::from_vaults(vec![Box::new(SidecarFile::in_dir(self.path()))])
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        self.restart();
        match &self.previous {
            Some(v) => std::env::set_var("KRONN_DATA_DIR", v),
            None => std::env::remove_var("KRONN_DATA_DIR"),
        }
    }
}

/// A config.toml exactly as 0.14.2 wrote it: key, auth token and provider
/// keys (one of them an External API connection credential) in plaintext.
fn write_0142_config(dir: &Path, key: &str) -> String {
    let mut cfg = config::default_config();
    cfg.server.auth_token = Some(AUTH_TOKEN.into());
    cfg.server.auth_enabled = true;
    cfg.tokens.keys = vec![
        ApiKey {
            id: "key-anthropic".into(),
            name: "Personal API Key".into(),
            provider: "anthropic".into(),
            value: ANTHROPIC.into(),
            active: true,
        },
        ApiKey {
            id: "key-conn".into(),
            name: "NVIDIA API key".into(),
            provider: "conn-nvidia-1a2b3c4d".into(),
            value: CONNECTION.into(),
            active: true,
        },
    ];
    let text = format!(
        "encryption_secret = \"{key}\"\n{}",
        toml::to_string_pretty(&cfg).unwrap()
    );
    std::fs::write(dir.join("config.toml"), &text).unwrap();
    // The DB migration runner's copy: credentials already out, key still in.
    std::fs::write(
        dir.join("config.toml.backup"),
        without_credentials(&text).unwrap(),
    )
    .unwrap();
    text
}

/// Ciphertext in every pre-existing encrypted column.
async fn seed_ciphertext(db: &Database, key: &str) {
    let mut env = std::collections::HashMap::new();
    env.insert("GITHUB_TOKEN".to_string(), MCP_SECRET.to_string());
    let mcp = crate::db::mcps::encrypt_env(&env, key).unwrap();
    let parsed = crypto::parse_secret(key).unwrap();
    let snapshot = crypto::encrypt(SNAPSHOT_SECRET, &parsed).unwrap();
    let github =
        crate::core::github_connection::encrypt_stored_token("github_pat_fixture0006", key)
            .unwrap();
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) \
             VALUES ('p1', 'p1', '/nowhere', datetime('now'), datetime('now'))",
            [],
        )?;
        crate::db::github_connections::set_mode(
            conn,
            "p1",
            crate::models::GithubConnectionMode::StoredToken,
            Some(&github),
        )?;
        conn.execute(
            "INSERT INTO mcp_servers (id, name, transport) VALUES ('s1','github','stdio')",
            [],
        )?;
        conn.execute(
            "INSERT INTO mcp_configs (id, server_id, label, env_encrypted, env_keys_json) \
             VALUES ('c1','s1','github', ?1, '[\"GITHUB_TOKEN\"]')",
            [mcp],
        )?;
        conn.execute(
            "INSERT INTO execution_variable_snapshots (id, run_kind, run_id, environment_ref, \
             resolved_at, retention_days, values_encrypted, fingerprint, provenance_json) \
             VALUES ('snap1','workflow','run1','default','2026-10-01T00:00:00Z',30,?1,'fp','[]')",
            [snapshot],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

/// The standalone boot order: load, reconcile, credential store.
async fn boot_like_main(
    dir: &DataDir,
    db: &Arc<Database>,
) -> (AppConfig, KeyOutcome, Option<CredentialBoot>) {
    let mut cfg = config::load()
        .await
        .unwrap()
        .unwrap_or_else(config::default_config);
    let outcome = keystore::reconcile_with(&mut cfg, db, &dir.sidecar(), dir.path())
        .await
        .unwrap();
    let result = boot(&mut cfg, db.clone(), dir.path(), &outcome, None)
        .await
        .unwrap();
    (cfg, outcome, result)
}

fn assert_no_secret_in(text: &str, key: &str, what: &str) {
    for secret in [key, ANTHROPIC, CONNECTION, AUTH_TOKEN] {
        assert!(
            !text.contains(secret),
            "{what} still holds a secret: {text}"
        );
    }
}

async fn every_column_decrypts(db: &Database, key: &str) {
    let parsed = crypto::parse_secret(key).unwrap();
    let columns = db
        .with_conn(|conn| {
            let mut out = Vec::new();
            for col in keystore::ENCRYPTED_COLUMNS {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {} FROM {} WHERE {} IS NOT NULL AND {} != ''",
                    col.column, col.table, col.column, col.column
                ))?;
                let values = stmt
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                out.push((col.table, col.column, values));
            }
            Ok(out)
        })
        .await
        .unwrap();
    for (table, column, values) in columns {
        assert!(!values.is_empty(), "{table}.{column} holds no ciphertext");
        for v in values {
            assert!(
                crypto::decrypt(&v, &parsed).is_ok(),
                "{table}.{column} does not decrypt"
            );
        }
    }
}

#[tokio::test]
#[serial]
async fn a_0142_config_migrates_without_loss_and_keeps_no_secret() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    let original = write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;

    let (cfg, outcome, result) = boot_like_main(&dir, &db).await;
    assert_eq!(
        outcome,
        KeyOutcome::Resolved {
            source: "legacy-config"
        }
    );
    let result = result.expect("armed");
    assert_eq!(
        result.moved_from_config, 3,
        "two provider keys and the auth token"
    );
    assert!(!result.generated_auth_token);
    assert_eq!(cfg.tokens.active_key_for("anthropic"), Some(ANTHROPIC));
    assert_eq!(cfg.server.auth_token.as_deref(), Some(AUTH_TOKEN));

    // Nothing secret left in either config file; the key lives in the sidecar.
    assert_no_secret_in(&dir.config_text(), &key, "config.toml");
    assert_no_secret_in(
        &std::fs::read_to_string(dir.path().join("config.toml.backup")).unwrap(),
        &key,
        "config.toml.backup",
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(crate::core::keyvault::SIDECAR_FILENAME))
            .unwrap()
            .trim(),
        key
    );
    every_column_decrypts(&db, &key).await;

    // The encrypted backup is owner-only and restores the original file.
    let backup = result.backup.expect("backup");
    assert_eq!(read_backup(&backup, &key).unwrap(), original);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert_no_secret_in(
        &std::fs::read_to_string(&backup).unwrap(),
        &key,
        "backup file",
    );

    // A restart reads everything back from the store.
    dir.restart();
    let (again, _, second) = boot_like_main(&dir, &db).await;
    let second = second.expect("armed");
    assert_eq!(second.moved_from_config, 0);
    assert_eq!(
        format!("{:?}", again.tokens.keys),
        format!("{:?}", cfg.tokens.keys)
    );
    assert_eq!(again.server.auth_token.as_deref(), Some(AUTH_TOKEN));
    assert!(
        again.server.auth_enabled,
        "auth_enabled survives (not regenerated)"
    );
    assert_eq!(again.encryption_secret.as_deref(), Some(key.as_str()));
    assert_no_secret_in(&dir.config_text(), &key, "config.toml after restart");
}

#[tokio::test]
#[serial]
async fn rerunning_the_migration_changes_nothing() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;

    boot_like_main(&dir, &db).await;
    let rows_before = db.with_conn(rows::list).await.unwrap();
    let text_before = dir.config_text();
    let backup_before = std::fs::read(dir.path().join(BACKUP_FILENAME)).unwrap();

    boot_like_main(&dir, &db).await;
    dir.restart();
    boot_like_main(&dir, &db).await;

    assert_eq!(
        db.with_conn(rows::list).await.unwrap(),
        rows_before,
        "rows untouched"
    );
    assert_eq!(dir.config_text(), text_before);
    assert_eq!(
        std::fs::read(dir.path().join(BACKUP_FILENAME)).unwrap(),
        backup_before,
        "the first backup is kept"
    );
}

#[tokio::test]
#[serial]
async fn an_interrupted_migration_converges_on_rerun() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    let original = write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;

    // Crash after the rows were written, before config.toml was rewritten:
    // the store holds the values and the file still holds them too.
    boot_like_main(&dir, &db).await;
    std::fs::write(dir.path().join("config.toml"), &original).unwrap();
    std::fs::remove_file(dir.path().join(BACKUP_FILENAME)).unwrap();
    dir.restart();

    let (cfg, _, result) = boot_like_main(&dir, &db).await;
    assert!(result.is_some());
    assert_eq!(cfg.tokens.keys.len(), 2, "no duplicate after the rerun");
    assert_eq!(db.with_conn(rows::list).await.unwrap().len(), 3);
    assert_no_secret_in(&dir.config_text(), &key, "config.toml");
    assert!(dir.path().join(BACKUP_FILENAME).exists());
    every_column_decrypts(&db, &key).await;
}

#[tokio::test]
#[serial]
async fn key_already_in_the_vault_and_still_in_config_converges() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    crate::core::keyvault::KeyVault::store(&SidecarFile::in_dir(dir.path()), &key).unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;

    let (_, outcome, _) = boot_like_main(&dir, &db).await;
    assert_eq!(outcome, KeyOutcome::Resolved { source: "sidecar" });
    assert!(!dir.config_text().contains("encryption_secret"));
}

#[tokio::test]
#[serial]
async fn the_key_stays_in_config_toml_when_no_vault_can_hold_it() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;

    let mut cfg = config::load().await.unwrap().unwrap();
    // No vault at all (read-only data dir, no keychain).
    let outcome =
        keystore::reconcile_with(&mut cfg, &db, &KeyStore::from_vaults(vec![]), dir.path())
            .await
            .unwrap();
    boot(&mut cfg, db.clone(), dir.path(), &outcome, None)
        .await
        .unwrap();
    let text = dir.config_text();
    assert!(text.contains(&key), "the only durable copy must stay");
    assert!(!text.contains(ANTHROPIC) && !text.contains(AUTH_TOKEN));
}

#[tokio::test]
#[serial]
async fn a_locked_key_leaves_config_toml_untouched() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    let original = write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mut cfg = config::load().await.unwrap().unwrap();
    let res = boot(
        &mut cfg,
        db.clone(),
        dir.path(),
        &KeyOutcome::Locked { encrypted_rows: 1 },
        None,
    )
    .await
    .unwrap();
    assert!(res.is_none());
    assert!(!is_armed(dir.path()));
    config::save(&cfg).await.unwrap();
    let text = dir.config_text();
    assert!(text.contains(ANTHROPIC) && text.contains(AUTH_TOKEN) && text.contains(&key));
    assert_eq!(text.len(), original.len());
}

#[tokio::test]
#[serial]
async fn saves_after_boot_go_to_the_store_and_never_to_the_file() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    let (mut cfg, _, _) = boot_like_main(&dir, &db).await;

    // A row from another key must survive every rewrite.
    let other = crypto::parse_secret(&crypto::generate_secret()).unwrap();
    let foreign = crypto::encrypt("from-a-lost-key", &other).unwrap();
    db.with_conn(move |conn| {
        rows::upsert_all(
            conn,
            &[StoredCredential {
                kind: KIND_PROVIDER_KEY.into(),
                id: "lost".into(),
                name: "lost".into(),
                provider: "openai".into(),
                active: false,
                position: 9,
                value_encrypted: foreign,
            }],
        )
    })
    .await
    .unwrap();
    dir.restart();
    let (reloaded, _, result) = boot_like_main(&dir, &db).await;
    assert_eq!(result.unwrap().locked_rows, 1);
    cfg.tokens.keys = reloaded.tokens.keys.clone();

    cfg.tokens.keys.push(ApiKey {
        id: "key-new".into(),
        name: "New".into(),
        provider: "openai".into(),
        value: "sk-openai-new-value".into(),
        active: true,
    });
    cfg.server.auth_token = Some("rotated-token-value".into());
    config::save(&cfg).await.unwrap();
    let text = dir.config_text();
    assert!(!text.contains("sk-openai-new-value") && !text.contains("rotated-token-value"));

    cfg.tokens.keys.retain(|k| k.id != "key-anthropic");
    config::save(&cfg).await.unwrap();

    dir.restart();
    let (after, _, _) = boot_like_main(&dir, &db).await;
    let ids: Vec<&str> = after.tokens.keys.iter().map(|k| k.id.as_str()).collect();
    assert_eq!(ids, vec!["key-conn", "key-new"]);
    assert_eq!(
        after.server.auth_token.as_deref(),
        Some("rotated-token-value")
    );
    let stored = db.with_conn(rows::list).await.unwrap();
    assert!(
        stored.iter().any(|r| r.id == "lost"),
        "undecryptable row kept"
    );
}

#[tokio::test]
#[serial]
async fn a_save_that_cannot_store_credentials_leaves_the_file_unchanged() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (mut cfg, _, _) = boot_like_main(&dir, &db).await;
    let before = dir.config_text();

    cfg.encryption_secret = None;
    cfg.server.auth_token = Some("changed".into());
    cfg.language = "es".into();
    let err = config::save(&cfg).await.unwrap_err();
    assert!(err.to_string().contains("locked"), "{err}");
    assert_eq!(dir.config_text(), before);
}

#[tokio::test]
#[serial]
async fn a_fresh_install_gets_an_auth_token_in_the_store_only() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (cfg, outcome, result) = boot_like_main(&dir, &db).await;
    assert_eq!(
        outcome,
        KeyOutcome::Resolved {
            source: "legacy-config"
        }
    );
    let result = result.unwrap();
    assert!(result.generated_auth_token);
    assert!(result.backup.is_none(), "nothing to back up");
    assert_eq!(
        cfg.server.auth_enabled,
        crate::core::env::auth_on_by_default()
    );
    let token = cfg.server.auth_token.clone().unwrap();
    assert!(!dir.config_text().contains(&token));

    dir.restart();
    let (again, _, second) = boot_like_main(&dir, &db).await;
    assert!(
        !second.unwrap().generated_auth_token,
        "the token is not regenerated"
    );
    assert_eq!(again.server.auth_token, Some(token));
}

#[tokio::test]
#[serial]
async fn forget_all_clears_the_store() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (mut cfg, _, _) = boot_like_main(&dir, &db).await;
    forget_all(dir.path(), &db).await.unwrap();
    assert!(db.with_conn(rows::list).await.unwrap().is_empty());
    cfg.tokens.keys.clear();
    cfg.server.auth_token = None;
    config::save(&cfg).await.unwrap();
    assert!(db.with_conn(rows::list).await.unwrap().is_empty());
}

#[test]
fn secret_detection_covers_every_field() {
    assert!(!toml_holds_secrets(
        "language = \"fr\"\n[server]\nport = 1\n"
    ));
    assert!(toml_holds_secrets("encryption_secret = \"ab\"\n"));
    assert!(toml_holds_secrets("[server]\nauth_token = \"t\"\n"));
    assert!(toml_holds_secrets("[tokens]\nanthropic = \"x\"\n"));
    assert!(toml_holds_secrets(
        "[tokens]\n[[tokens.keys]]\nid = \"a\"\nvalue = \"v\"\n"
    ));
    assert!(!toml_holds_secrets(
        "[tokens]\n[[tokens.keys]]\nid = \"a\"\nvalue = \"\"\n"
    ));
    assert!(toml_holds_secrets("not = [valid"));
}

#[test]
fn merge_prefers_the_file_and_keeps_table_order() {
    let cred = |id: &str, v: &str| PlainCredential {
        kind: KIND_PROVIDER_KEY.into(),
        id: id.into(),
        name: id.into(),
        provider: "p".into(),
        active: true,
        value: v.into(),
    };
    let merged = merge(
        &[cred("a", "old"), cred("b", "b")],
        &[cred("c", "c"), cred("a", "new")],
    );
    let got: Vec<(&str, &str)> = merged
        .iter()
        .map(|c| (c.id.as_str(), c.value.as_str()))
        .collect();
    assert_eq!(got, vec![("a", "new"), ("b", "b"), ("c", "c")]);
}

/// An operator-set KRONN_AUTH_TOKEN goes into the encrypted store, never into
/// config.toml, and leaves the process environment.
#[tokio::test]
#[serial]
async fn an_env_auth_token_is_stored_encrypted_and_leaves_the_environment() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    std::env::set_var("KRONN_AUTH_TOKEN", " operator-env-token-0007 ");
    let env_token = config::take_env_auth_token();
    assert_eq!(env_token.as_deref(), Some("operator-env-token-0007"));
    assert!(
        std::env::var("KRONN_AUTH_TOKEN").is_err(),
        "removed from the env"
    );

    let mut cfg = config::default_config();
    crate::resolve_key_and_credentials(&mut cfg, &db, env_token)
        .await
        .unwrap();
    assert_eq!(
        cfg.server.auth_token.as_deref(),
        Some("operator-env-token-0007")
    );
    assert!(cfg.server.auth_enabled, "setting the token asks for auth");
    assert!(!dir.config_text().contains("operator-env-token-0007"));
    let key = cfg.encryption_secret.clone().unwrap();
    let stored = db.with_conn(rows::list).await.unwrap();
    let token_row = stored.iter().find(|r| r.kind == KIND_AUTH_TOKEN).unwrap();
    assert!(!token_row
        .value_encrypted
        .contains("operator-env-token-0007"));
    assert_eq!(
        decrypt_value(&token_row.value_encrypted, &key).unwrap(),
        "operator-env-token-0007"
    );

    // Next start without the variable: the stored token is still the one.
    dir.restart();
    let mut again = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut again, &db, None)
        .await
        .unwrap();
    assert_eq!(
        again.server.auth_token.as_deref(),
        Some("operator-env-token-0007")
    );

    // A different env token does not replace the stored one.
    dir.restart();
    let mut third = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut third, &db, Some("another".into()))
        .await
        .unwrap();
    assert_eq!(
        third.server.auth_token.as_deref(),
        Some("operator-env-token-0007")
    );
}

async fn status_of(
    router: axum::Router,
    method: &str,
    path: &str,
    from: [u8; 4],
) -> axum::http::StatusCode {
    use tower::ServiceExt;
    let mut req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(if method == "POST" {
            "{}"
        } else {
            ""
        }))
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            from, 40000,
        ))));
    router.oneshot(req).await.unwrap().status()
}

/// A stored auth token the boot cannot decrypt (key lost) is "auth locked",
/// not "no auth": ordinary routes are refused, recovery stays reachable locally.
#[tokio::test]
#[serial]
async fn a_locked_auth_token_refuses_ordinary_routes_and_keeps_recovery_open() {
    use axum::http::StatusCode;
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    boot_like_main(&dir, &db).await;
    assert!(!dir.config_text().contains(&key));

    // The only copy of the key disappears (keychain reset, sidecar lost).
    dir.restart();
    std::fs::remove_file(dir.path().join(crate::core::keyvault::SIDECAR_FILENAME)).unwrap();
    let mut cfg = config::load().await.unwrap().unwrap();
    assert!(cfg.server.auth_enabled);
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    assert!(cfg.server.auth_token.is_none());
    assert!(
        cfg.server.auth_locked,
        "a missing key is not a deliberate auth disable"
    );
    // The LAN boot guard counts the lock as enforced auth.
    assert!(cfg.server.auth_token_or_lock());
    assert!(crate::core::net_expose::insecure_lan_boot_error(
        true,
        cfg.server.auth_enabled,
        cfg.server.auth_token_or_lock(),
        false
    )
    .is_none());

    let state = crate::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(cfg)),
        db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let router = crate::build_router_with_auth(state, true);
    let loopback = [127, 0, 0, 1];
    assert_eq!(
        status_of(router.clone(), "GET", "/api/projects", loopback).await,
        StatusCode::LOCKED
    );
    assert_eq!(
        status_of(
            router.clone(),
            "GET",
            "/api/config/recovery/status",
            loopback
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(router.clone(), "GET", "/api/health", loopback).await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(
            router.clone(),
            "GET",
            "/api/config/recovery/status",
            [192, 168, 1, 9]
        )
        .await,
        StatusCode::LOCKED,
        "recovery is local-only while locked"
    );
    // The restore route is reached (it answers, here with a refusal for the
    // missing passphrase, instead of the lock).
    assert_ne!(
        status_of(router, "POST", "/api/config/recovery/restore", loopback).await,
        StatusCode::LOCKED
    );
}

/// Sidecar-only ladder without a recovery passphrase: the credentials leave
/// config.toml, the key stays there and in config.toml.backup (two copies).
#[tokio::test]
#[serial]
async fn a_sidecar_only_install_keeps_a_second_copy_of_the_key() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    let mut cfg = config::load().await.unwrap().unwrap();
    let outcome = keystore::reconcile_with(&mut cfg, &db, &dir.sidecar_only(), dir.path())
        .await
        .unwrap();
    boot(&mut cfg, db.clone(), dir.path(), &outcome, None)
        .await
        .unwrap()
        .expect("armed");
    let text = dir.config_text();
    assert!(text.contains(&key), "config.toml keeps the second copy");
    assert!(!text.contains(ANTHROPIC) && !text.contains(AUTH_TOKEN));
    let backup = std::fs::read_to_string(dir.path().join("config.toml.backup")).unwrap();
    assert!(backup.contains(&key), "the backup keeps it too");
    assert!(!backup.contains(ANTHROPIC));
}

/// Migrate a 0.14.2 install on the macOS-like ladder, then lose every vault
/// copy of the key: the next start is locked with a stored auth token.
async fn migrated_then_key_lost(dir: &DataDir, db: &Arc<Database>) -> String {
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    seed_ciphertext(db, &key).await;
    boot_like_main(dir, db).await;
    dir.restart();
    std::fs::remove_file(dir.path().join(crate::core::keyvault::SIDECAR_FILENAME)).unwrap();
    key
}

#[tokio::test]
#[serial]
async fn the_locked_answer_carries_a_typed_error_code() {
    use tower::ServiceExt;
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    migrated_then_key_lost(&dir, &db).await;
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    let state = crate::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(cfg)),
        db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let mut req = axum::http::Request::builder()
        .uri("/api/setup/status")
        .body(axum::body::Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))));
    let resp = crate::build_router_with_auth(state, true)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::LOCKED);
    let body = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error_code"], "auth_locked");
}

/// KRONN_AUTH_TOKEN while locked: usable this session, never written to
/// config.toml, never replacing the stored token (which a restore brings back).
#[tokio::test]
#[serial]
async fn an_env_token_while_locked_is_session_only() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let key = migrated_then_key_lost(&dir, &db).await;
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, Some("session-env-token".into()))
        .await
        .unwrap();
    assert_eq!(cfg.server.auth_token.as_deref(), Some("session-env-token"));
    assert!(cfg.server.auth_token_session_only && !cfg.server.auth_locked);
    assert!(cfg.server.auth_token_or_lock());
    config::save(&cfg).await.unwrap();
    assert!(!dir.config_text().contains("session-env-token"));

    // Restore the key: the stored token is back, the session one is gone.
    let code = crate::core::recovery::to_code(
        &crate::core::recovery::wrap_key(&key, "pass phrase").unwrap(),
    );
    let store = KeyStore::from_vaults(vec![Box::new(SidecarFile::in_dir(dir.path()))]);
    let outcome = keystore::recover_with_passphrase(
        &mut cfg,
        &db,
        &store,
        "pass phrase",
        Some(&code),
        dir.path(),
    )
    .await
    .unwrap();
    boot(&mut cfg, db.clone(), dir.path(), &outcome, None)
        .await
        .unwrap();
    assert_eq!(cfg.server.auth_token.as_deref(), Some(AUTH_TOKEN));
    assert!(!cfg.server.auth_token_session_only);
    assert!(!dir.config_text().contains("session-env-token"));
}

/// An auth row under a lost key: an unrelated save still succeeds, and an
/// explicit new token replaces the row, which then decrypts under the key.
#[tokio::test]
#[serial]
async fn an_undecryptable_auth_row_never_blocks_saves_and_can_be_replaced() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    boot_like_main(&dir, &db).await;
    // The auth row is now under some other key.
    let other = crypto::parse_secret(&crypto::generate_secret()).unwrap();
    let foreign = crypto::encrypt("old-token", &other).unwrap();
    db.with_conn(move |conn| {
        conn.execute(
            "UPDATE stored_credentials SET value_encrypted = ?1 WHERE kind = 'auth_token'",
            [foreign],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    dir.restart();
    let (mut cfg, _, result) = boot_like_main(&dir, &db).await;
    assert_eq!(result.unwrap().locked_rows, 1);
    assert!(cfg.server.auth_locked && cfg.server.auth_token.is_none());

    cfg.language = "es".into();
    config::save(&cfg).await.unwrap();

    cfg.server.auth_token = Some("regenerated-token".into());
    config::save(&cfg).await.unwrap();
    cfg.language = "fr".into();
    config::save(&cfg).await.unwrap();
    let stored = db.with_conn(rows::list).await.unwrap();
    let row = stored.iter().find(|r| r.kind == KIND_AUTH_TOKEN).unwrap();
    assert_eq!(
        decrypt_value(&row.value_encrypted, &key).unwrap(),
        "regenerated-token"
    );
}

/// A credential boot that fails before loading the token locks auth instead
/// of leaving the API open.
#[tokio::test]
#[serial]
async fn a_failed_credential_boot_locks_auth() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    // Sidecar-only: config.toml keeps the key, so the backup step runs again.
    let mut cfg = config::load().await.unwrap().unwrap();
    let outcome = keystore::reconcile_with(&mut cfg, &db, &dir.sidecar_only(), dir.path())
        .await
        .unwrap();
    boot(&mut cfg, db.clone(), dir.path(), &outcome, None)
        .await
        .unwrap();
    dir.restart();
    std::fs::remove_file(dir.path().join(BACKUP_FILENAME)).unwrap();
    // The backup temp path is blocked: writing the backup fails.
    std::fs::create_dir(dir.path().join(format!(".{BACKUP_FILENAME}.tmp"))).unwrap();
    let mut cfg = config::load().await.unwrap().unwrap();
    assert!(cfg.server.auth_token.is_none());
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    assert!(!is_armed(dir.path()));
    assert!(cfg.server.auth_locked, "auth must lock, not open");
}

async fn reset_via_api(db: &Arc<Database>, cfg: AppConfig) -> AppConfig {
    let state = crate::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(cfg)),
        db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let status = status_of(
        crate::build_router_with_auth(state.clone(), true),
        "POST",
        "/api/setup/reset",
        [127, 0, 0, 1],
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let out = state.config.read().await.clone();
    out
}

async fn rows_in(db: &Database, table: &'static str) -> i64 {
    db.with_conn(move |c| {
        Ok(c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
    })
    .await
    .unwrap()
}

/// Reset where config.toml carries a needed copy of the key (sidecar-only):
/// the file keeps only the key, the next start is a first run with that key,
/// and every table holding ciphertext is emptied.
#[tokio::test]
#[serial]
async fn reset_never_removes_a_needed_key_copy_and_clears_every_encrypted_table() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    let mut cfg = config::load().await.unwrap().unwrap();
    let outcome = keystore::reconcile_with(&mut cfg, &db, &dir.sidecar_only(), dir.path())
        .await
        .unwrap();
    boot(&mut cfg, db.clone(), dir.path(), &outcome, None)
        .await
        .unwrap();
    assert!(config::retained_disk_key(dir.path()).is_some());

    let after = reset_via_api(&db, cfg).await;
    assert_eq!(after.encryption_secret.as_deref(), Some(key.as_str()));
    assert_eq!(
        dir.config_text().trim(),
        format!("encryption_secret = \"{key}\"")
    );
    assert!(config::is_first_run().await.unwrap());
    for col in keystore::ENCRYPTED_COLUMNS {
        assert_eq!(
            rows_in(&db, col.table).await,
            0,
            "{} not cleared",
            col.table
        );
    }
    // The next start reads the key back from the key-only file.
    dir.restart();
    let reloaded = config::load().await.unwrap().unwrap();
    assert_eq!(reloaded.encryption_secret.as_deref(), Some(key.as_str()));
    assert_eq!(config::retained_disk_key(dir.path()), Some(key));
}

/// With two vault copies, reset removes config.toml as before.
#[tokio::test]
#[serial]
async fn reset_removes_config_toml_when_the_vaults_hold_the_key() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    let (cfg, _, _) = boot_like_main(&dir, &db).await;
    assert!(config::retained_disk_key(dir.path()).is_none());
    reset_via_api(&db, cfg).await;
    assert!(!dir.path().join("config.toml").exists());
    assert!(config::is_first_run().await.unwrap());
}

/// No config.toml: no random key is offered, so none can be kept for good.
#[tokio::test]
#[serial]
async fn a_missing_config_toml_never_offers_a_random_key() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    crate::core::keyvault::KeyVault::store(&SidecarFile::in_dir(dir.path()), &key).unwrap();
    assert!(config::load().await.unwrap().is_none());

    let mut cfg = config::default_config_without_key();
    keystore::reconcile_with(&mut cfg, &db, &dir.sidecar_only(), dir.path())
        .await
        .unwrap();
    assert_eq!(cfg.encryption_secret.as_deref(), Some(key.as_str()));
    // Sidecar only: the second copy kept in config.toml is the REAL key.
    assert_eq!(config::retained_disk_key(dir.path()), Some(key.clone()));

    dir.restart();
    let mut cfg = config::default_config_without_key();
    keystore::reconcile_with(&mut cfg, &db, &dir.sidecar(), dir.path())
        .await
        .unwrap();
    assert_eq!(config::retained_disk_key(dir.path()), None);
}

/// Both mains take the data-dir lock before loading config.toml, and remove
/// KRONN_AUTH_TOKEN from their environment.
#[test]
fn both_mains_lock_before_loading_and_drop_the_env_token() {
    let backend = include_str!("../main.rs");
    let lock = backend.find("acquire_data_dir_lock()").unwrap();
    let load = backend.find("config::load().await").unwrap();
    assert!(lock < load, "backend: lock before config::load()");
    assert!(backend.contains("take_env_auth_token()"));

    let desktop = include_str!("../../../desktop/src-tauri/src/main.rs");
    let main_fn = desktop.find("fn main()").unwrap();
    let take = desktop.find("take_env_auth_token()").unwrap();
    let builder = desktop.find("tauri::Builder::default()").unwrap();
    assert!(
        main_fn < take && take < builder,
        "desktop: env token removed early in main()"
    );
    // config::load() runs in start_backend, which only the setup closure calls,
    // after main() took the lock.
    let lock = main_fn + desktop[main_fn..].find("acquire_data_dir_lock()").unwrap();
    let call = main_fn + desktop[main_fn..].find("start_backend(").unwrap();
    assert!(
        lock < builder && builder < call,
        "desktop: lock in main(), backend started later"
    );
    assert!(desktop.contains("config::load().await"));
}
