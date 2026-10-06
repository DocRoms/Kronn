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
        let previous = crate::core::child_env::var("KRONN_DATA_DIR").ok();
        crate::core::child_env::set_var("KRONN_DATA_DIR", dir.path());
        crate::core::child_env::remove_var(crate::core::keyvault::ENV_KEK);
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
            Some(v) => crate::core::child_env::set_var("KRONN_DATA_DIR", v),
            None => crate::core::child_env::remove_var("KRONN_DATA_DIR"),
        }
    }
}

/// A config.toml exactly as 0.14.2 wrote it: key, auth token and provider
/// keys (one of them an External API connection credential) in plaintext.
fn write_0142_config(dir: &Path, key: &str) -> String {
    let text = write_0142_config_text(key);
    std::fs::write(dir.join("config.toml"), &text).unwrap();
    // The DB migration runner's copy: credentials already out, key still in.
    std::fs::write(
        dir.join("config.toml.backup"),
        without_credentials(&text).unwrap(),
    )
    .unwrap();
    text
}

fn write_0142_config_text(key: &str) -> String {
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
    format!(
        "encryption_secret = \"{key}\"\n{}",
        toml::to_string_pretty(&cfg).unwrap()
    )
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
    let result = boot(
        &mut cfg,
        db.clone(),
        dir.path(),
        &outcome,
        None,
        BootMode::Startup,
    )
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
    boot(
        &mut cfg,
        db.clone(),
        dir.path(),
        &outcome,
        None,
        BootMode::Startup,
    )
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
        BootMode::Startup,
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
    crate::core::child_env::set_var("KRONN_AUTH_TOKEN", " operator-env-token-0007 ");
    let env_token = config::take_env_auth_token();
    assert_eq!(env_token.as_deref(), Some("operator-env-token-0007"));
    assert!(
        crate::core::child_env::var("KRONN_AUTH_TOKEN").is_err(),
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
    boot(
        &mut cfg,
        db.clone(),
        dir.path(),
        &outcome,
        None,
        BootMode::Startup,
    )
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
    boot(
        &mut cfg,
        db.clone(),
        dir.path(),
        &outcome,
        None,
        BootMode::Startup,
    )
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
    boot(
        &mut cfg,
        db.clone(),
        dir.path(),
        &outcome,
        None,
        BootMode::Startup,
    )
    .await
    .unwrap();
    dir.restart();
    std::fs::remove_file(dir.path().join(BACKUP_FILENAME)).unwrap();
    // The backup temp path is blocked: writing the backup fails.
    std::fs::create_dir(dir.path().join(format!(".{BACKUP_FILENAME}.tmp"))).unwrap();
    // config.toml holds a credential again (the key alone no longer triggers
    // a backup, C3-04), so the backup step runs and fails.
    let mut migrated: toml::Table = dir.config_text().parse().unwrap();
    migrated
        .get_mut("tokens")
        .and_then(|t| t.as_table_mut())
        .unwrap()
        .insert(
            "keys".into(),
            toml::Value::Array(vec![toml::Value::Table(
                "id = \"file-key\"\nname = \"n\"\nprovider = \"openai\"\nvalue = \"sk-file\"\nactive = true\n"
                    .parse()
                    .unwrap(),
            )]),
        );
    std::fs::write(
        dir.path().join("config.toml"),
        toml::to_string(&migrated).unwrap(),
    )
    .unwrap();
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
    boot(
        &mut cfg,
        db.clone(),
        dir.path(),
        &outcome,
        None,
        BootMode::Startup,
    )
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
        // The API auth token survives a reset (C2-20); every other row goes.
        let expected = i64::from(col.table == "stored_credentials");
        assert_eq!(
            rows_in(&db, col.table).await,
            expected,
            "{} not cleared",
            col.table
        );
    }
    let left = db.with_conn(rows::list).await.unwrap();
    assert!(left.iter().all(|r| r.kind == KIND_AUTH_TOKEN));
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
    // C2-29: read and removed before the multi-threaded runtime is built.
    let take = backend.find("take_env_auth_token()").unwrap();
    let runtime = backend.find("new_multi_thread()").unwrap();
    assert!(
        take < runtime,
        "backend: env token removed before any thread"
    );
    assert!(!backend.contains("#[tokio::main]"));
    // C2-30: the desktop applies the same LAN guard; C2-36: full error chain.
    let desktop_src = include_str!("../../../desktop/src-tauri/src/main.rs");
    assert!(desktop_src.contains("insecure_lan_boot_error("));
    assert!(desktop_src.contains("stopped during startup: {e:#}"));

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
    // C3-17: a database that cannot open reaches the startup error (no panic),
    // and the desktop writes kronn.log.
    assert!(!backend.contains("Database::open().expect"));
    assert!(!desktop.contains("Database::open().expect"));
    assert!(desktop.contains("kronn.log"));
}

async fn json_of(
    router: axum::Router,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    json_from(router, method, path, body, [127, 0, 0, 1]).await
}

async fn json_from(
    router: axum::Router,
    method: &str,
    path: &str,
    body: serde_json::Value,
    from: [u8; 4],
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let mut req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            from, 40000,
        ))));
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

/// C2-05 — while the store cannot be armed (key lost after the migration), no
/// save writes a provider key or a token to config.toml in clear: the API
/// refuses the change, and a direct save of a new credential fails.
#[tokio::test]
#[serial]
async fn a_locked_store_never_writes_credentials_to_config_toml() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let key = migrated_then_key_lost(&dir, &db).await;
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    assert!(!is_armed(dir.path()));
    // Auth off so the handler itself is reached.
    cfg.server.auth_enabled = false;
    let state = crate::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(cfg.clone())),
        db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let router = crate::build_router_with_auth(state.clone(), true);
    let (_, body) = json_of(
        router.clone(),
        "POST",
        "/api/config/api-keys",
        serde_json::json!({"name": "n", "provider": "openai", "value": "sk-locked-new-value"}),
    )
    .await;
    assert_eq!(body["success"], false, "{body}");
    assert!(body["error"].as_str().unwrap().contains("locked"));
    let (_, body) = json_of(
        router,
        "POST",
        "/api/config/auth-token/regenerate",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(body["success"], false, "{body}");
    assert!(
        state.config.read().await.tokens.keys.is_empty(),
        "memory untouched"
    );

    // An unrelated save still works and writes no secret.
    cfg.language = "es".into();
    config::save(&cfg).await.unwrap();
    // A credential pushed in memory anyway is refused by the save itself.
    cfg.tokens.keys.push(ApiKey {
        id: "sneaky".into(),
        name: "n".into(),
        provider: "openai".into(),
        value: "sk-should-never-land".into(),
        active: true,
    });
    assert!(config::save(&cfg).await.is_err());
    let text = dir.config_text();
    assert!(!text.contains("sk-locked-new-value") && !text.contains("sk-should-never-land"));
    assert!(!text.contains(ANTHROPIC) && !text.contains(AUTH_TOKEN));
    let _ = key;
}

fn state_with(cfg: AppConfig, db: &Arc<Database>) -> crate::AppState {
    crate::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(cfg)),
        db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

/// Migrated install whose auth row is now under another key, booted again.
async fn auth_row_under_another_key(dir: &DataDir, db: &Arc<Database>) -> (AppConfig, String) {
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    seed_ciphertext(db, &key).await;
    boot_like_main(dir, db).await;
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
    let (cfg, _, _) = boot_like_main(dir, db).await;
    (cfg, key)
}

/// C2-03 — a restore whose credential loading fails does not answer success.
#[tokio::test]
#[serial]
async fn a_restore_whose_credentials_fail_to_load_reports_the_failure() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let key = migrated_then_key_lost(&dir, &db).await;
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    assert!(cfg.server.auth_locked);
    // The credential boot after the restore will fail writing its backup.
    std::fs::write(dir.path().join("config.toml"), write_0142_config_text(&key)).unwrap();
    let _ = std::fs::remove_file(dir.path().join(BACKUP_FILENAME));
    std::fs::create_dir(dir.path().join(format!(".{BACKUP_FILENAME}.tmp"))).unwrap();
    let code = crate::core::recovery::to_code(
        &crate::core::recovery::wrap_key(&key, "pass phrase").unwrap(),
    );
    let state = state_with(cfg, &db);
    let (_, body) = json_of(
        crate::build_router_with_auth(state.clone(), true),
        "POST",
        "/api/config/recovery/restore",
        serde_json::json!({"passphrase": "pass phrase", "recovery_code": code}),
    )
    .await;
    assert_eq!(body["success"], false, "{body}");
    assert!(state.config.read().await.server.auth_token.is_none());
    // C5-04: the status then says why, so the locked screen asks for a restart.
    let (_, status) = json_of(
        crate::build_router_with_auth(state, true),
        "GET",
        "/api/config/recovery/status",
        serde_json::json!({}),
    )
    .await;
    assert!(
        status["data"]["credentials_unavailable"].is_string(),
        "{status}"
    );
}

/// C2-04 — a restore keeps the operator's session token and auth stays on,
/// even where the platform default is off (Docker).
#[tokio::test]
#[serial]
async fn a_restore_keeps_the_session_token_and_auth_enabled() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let key = migrated_then_key_lost(&dir, &db).await;
    db.with_conn(|conn| {
        conn.execute(
            "DELETE FROM stored_credentials WHERE kind = 'auth_token'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    crate::core::child_env::set_var("KRONN_IN_DOCKER", "1");
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, Some("operator-token".into()))
        .await
        .unwrap();
    assert!(cfg.server.auth_token_session_only && cfg.server.auth_enabled);
    let code = crate::core::recovery::to_code(
        &crate::core::recovery::wrap_key(&key, "pass phrase").unwrap(),
    );
    let state = state_with(cfg, &db);
    let (_, body) = json_from(
        crate::build_router_with_auth(state.clone(), true),
        "POST",
        "/api/config/recovery/restore",
        serde_json::json!({"passphrase": "pass phrase", "recovery_code": code}),
        [127, 0, 0, 1],
    )
    .await;
    crate::core::child_env::remove_var("KRONN_IN_DOCKER");
    assert_eq!(body["success"], true, "{body}");
    let after = state.config.read().await.clone();
    assert_eq!(after.server.auth_token.as_deref(), Some("operator-token"));
    assert!(after.server.auth_enabled, "a restore never turns auth off");
}

/// C2-06 — with the key in use but the auth row under another key, a local
/// caller replaces the token through the real router; the row then decrypts.
#[tokio::test]
#[serial]
async fn a_locked_token_row_is_replaced_through_the_router() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (cfg, key) = auth_row_under_another_key(&dir, &db).await;
    assert!(cfg.server.auth_locked && cfg.encryption_secret.is_some());
    let state = state_with(cfg, &db);
    let router = crate::build_router_with_auth(state.clone(), true);
    let (status, _) = json_of(
        router.clone(),
        "GET",
        "/api/projects",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::LOCKED);
    let (_, body) = json_of(
        router.clone(),
        "POST",
        "/api/config/auth-token/regenerate",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(body["success"], true, "{body}");
    let token = body["data"].as_str().unwrap().to_string();
    let stored = db.with_conn(rows::list).await.unwrap();
    let row = stored.iter().find(|r| r.kind == KIND_AUTH_TOKEN).unwrap();
    assert_eq!(decrypt_value(&row.value_encrypted, &key).unwrap(), token);
    let (status, _) = json_of(router, "GET", "/api/projects", serde_json::json!({})).await;
    assert_ne!(status, axum::http::StatusCode::LOCKED);
}

/// C2-07 — strict localhost does not block the recovery routes while locked.
#[tokio::test]
#[serial]
async fn strict_localhost_keeps_recovery_reachable_while_locked() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    migrated_then_key_lost(&dir, &db).await;
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    cfg.server.auth_strict_localhost = true;
    let router = crate::build_router_with_auth(state_with(cfg, &db), true);
    let (status, _) = json_of(
        router,
        "POST",
        "/api/config/recovery/restore",
        serde_json::json!({"passphrase": "x"}),
    )
    .await;
    assert_ne!(status, axum::http::StatusCode::LOCKED);
}

/// C2-15 — with the auth row unreadable, KRONN_AUTH_TOKEN serves the session
/// and an unrelated save leaves the row untouched.
#[tokio::test]
#[serial]
async fn an_env_token_never_replaces_an_unreadable_token_row() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (_, _) = auth_row_under_another_key(&dir, &db).await;
    let before = db.with_conn(rows::list).await.unwrap();
    dir.restart();
    let mut cfg = config::load().await.unwrap().unwrap();
    // The vaults hold the key (two copies), so the store arms.
    let outcome = keystore::reconcile_with(&mut cfg, &db, &dir.sidecar(), dir.path())
        .await
        .unwrap();
    boot(
        &mut cfg,
        db.clone(),
        dir.path(),
        &outcome,
        Some("env-token"),
        BootMode::Startup,
    )
    .await
    .unwrap();
    assert!(cfg.server.auth_locked);
    // resolve's adoption step, as in production.
    cfg.server.auth_token = Some("env-token".into());
    cfg.server.auth_token_session_only = true;
    cfg.language = "es".into();
    config::save(&cfg).await.unwrap();
    let after = db.with_conn(rows::list).await.unwrap();
    assert_eq!(
        before
            .iter()
            .find(|r| r.kind == KIND_AUTH_TOKEN)
            .unwrap()
            .value_encrypted,
        after
            .iter()
            .find(|r| r.kind == KIND_AUTH_TOKEN)
            .unwrap()
            .value_encrypted
    );
    assert!(!dir.config_text().contains("env-token"));
}

/// C2-16 — a token in config.toml wins over an unreadable stored row (the file
/// is newer), as the key-management doc states.
#[tokio::test]
#[serial]
async fn a_config_token_replaces_an_unreadable_token_row() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (_, key) = auth_row_under_another_key(&dir, &db).await;
    dir.restart();
    let mut cfg = config::load().await.unwrap().unwrap();
    cfg.server.auth_token = Some("downgrade-minted".into());
    let outcome = keystore::reconcile_with(&mut cfg, &db, &dir.sidecar(), dir.path())
        .await
        .unwrap();
    boot(
        &mut cfg,
        db.clone(),
        dir.path(),
        &outcome,
        None,
        BootMode::Startup,
    )
    .await
    .unwrap();
    let stored = db.with_conn(rows::list).await.unwrap();
    let row = stored.iter().find(|r| r.kind == KIND_AUTH_TOKEN).unwrap();
    assert_eq!(
        decrypt_value(&row.value_encrypted, &key).unwrap(),
        "downgrade-minted"
    );
}

/// C2-20 — after a reset, a non-local request without a token is still refused
/// and the credential backups are gone.
#[tokio::test]
#[serial]
async fn a_reset_keeps_auth_and_removes_the_credential_backups() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    let (cfg, _, _) = boot_like_main(&dir, &db).await;
    assert!(dir.path().join(BACKUP_FILENAME).exists());
    let state = state_with(cfg, &db);
    let router = crate::build_router_with_auth(state.clone(), true);
    let (_, body) = json_of(
        router.clone(),
        "POST",
        "/api/setup/reset",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(body["success"], true, "{body}");
    let (status, _) = json_from(
        router,
        "GET",
        "/api/projects",
        serde_json::json!({}),
        [192, 168, 1, 9],
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
    assert!(!dir.path().join(BACKUP_FILENAME).exists());
    assert!(!dir.path().join("config.toml.backup").exists());
}

/// C2-22 — config.toml lost after the migration, key lost too: the stored
/// token row still says auth is on, so the API is locked, not open.
#[tokio::test]
#[serial]
async fn a_lost_config_toml_does_not_turn_auth_off() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    migrated_then_key_lost(&dir, &db).await;
    std::fs::remove_file(dir.path().join("config.toml")).unwrap();
    let mut cfg = config::load()
        .await
        .unwrap()
        .unwrap_or_else(config::default_config_without_key);
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    let (status, _) = json_from(
        crate::build_router_with_auth(state_with(cfg, &db), true),
        "GET",
        "/api/projects",
        serde_json::json!({}),
        [192, 168, 1, 9],
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::LOCKED);
}

/// C2-28 — a failed save leaves the live token unchanged.
#[tokio::test]
#[serial]
async fn a_failed_token_save_leaves_the_live_token_unchanged() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    let (mut cfg, _, _) = boot_like_main(&dir, &db).await;
    // Armed, but no key in memory: storing a credential fails.
    cfg.encryption_secret = None;
    let state = state_with(cfg, &db);
    let router = crate::build_router_with_auth(state.clone(), true);
    let (_, body) = json_of(
        router.clone(),
        "POST",
        "/api/config/auth-token/regenerate",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(body["success"], false, "{body}");
    assert_eq!(
        state.config.read().await.server.auth_token.as_deref(),
        Some(AUTH_TOKEN)
    );
    let (_, body) = json_of(
        router,
        "POST",
        "/api/config/network-exposure",
        serde_json::json!({"exposed": true}),
    )
    .await;
    let _ = body;
    assert_eq!(
        state.config.read().await.server.auth_token.as_deref(),
        Some(AUTH_TOKEN)
    );
}

// ── review round 4 ──────────────────────────────────────────────────────────

/// C3-02 — a reset while the key is locked leaves no unreadable token row:
/// the next start mints a key instead of staying locked for good.
#[tokio::test]
#[serial]
async fn a_reset_while_locked_lets_the_next_start_mint() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    migrated_then_key_lost(&dir, &db).await;
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    assert!(cfg.encryption_secret.is_none());
    cfg.server.auth_enabled = false;
    let router = crate::build_router_with_auth(state_with(cfg, &db), true);
    let (_, body) = json_of(
        router.clone(),
        "POST",
        "/api/setup/reset",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(body["success"], true, "{body}");
    // C6-06: a reset is a first run, locked or not.
    assert!(config::is_first_run().await.unwrap());
    // C5-06: the running instance has a key and an armed store right away.
    let (_, status) = json_of(
        router.clone(),
        "GET",
        "/api/config/recovery/status",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status["data"]["key_locked"], false, "{status}");
    let (_, saved) = json_of(
        router,
        "POST",
        "/api/config/api-keys",
        serde_json::json!({"name": "n", "provider": "openai", "value": "sk-after-reset"}),
    )
    .await;
    assert_eq!(saved["success"], true, "{saved}");
    dir.restart();
    let mut next = config::default_config_without_key();
    let outcome = keystore::reconcile_with(&mut next, &db, &dir.sidecar_only(), dir.path())
        .await
        .unwrap();
    assert!(!matches!(outcome, KeyOutcome::Locked { .. }), "{outcome:?}");
}

/// C3-04 — the key is in use but the credentials fail to load: the status
/// says why, and credential changes are refused with that reason.
#[tokio::test]
#[serial]
async fn a_credential_boot_failure_with_the_key_in_use_is_reported() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    std::fs::create_dir(dir.path().join("config.toml")).unwrap();
    // A config that still carries a credential, so the backup step runs.
    let mut cfg = config::default_config_without_key();
    cfg.tokens.keys.push(ApiKey {
        id: "k".into(),
        name: "n".into(),
        provider: "openai".into(),
        value: "sk-file".into(),
        active: true,
    });
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    assert!(cfg.encryption_secret.is_some() && !is_armed(dir.path()));
    let failure = boot_failure(dir.path()).expect("recorded");
    let refusal = refuse_if_locked(dir.path()).unwrap_err();
    assert!(
        refusal.contains("could not be loaded at start"),
        "{refusal}"
    );
    assert!(refusal.contains(&failure) || !failure.is_empty());
    let (_, body) = json_of(
        crate::build_router_with_auth(state_with(cfg, &db), true),
        "GET",
        "/api/config/recovery/status",
        serde_json::json!({}),
    )
    .await;
    assert!(
        body["data"]["credentials_unavailable"].is_string(),
        "{body}"
    );
    std::fs::remove_dir(dir.path().join("config.toml")).unwrap();
}

/// C3-05 — the stored credentials cannot even be read and config.toml is
/// lost: auth is locked AND enabled (fail closed), not open.
#[tokio::test]
#[serial]
async fn an_unreadable_credential_store_fails_closed() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    db.with_conn(|c| {
        // The reconciler can count its ciphertext, but the full row read fails.
        c.execute_batch(
            "DROP TABLE stored_credentials; CREATE TABLE stored_credentials (value_encrypted TEXT);",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let mut cfg = config::default_config_without_key();
    assert!(!cfg.server.auth_enabled);
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    assert!(cfg.server.auth_locked && cfg.server.auth_enabled);
    let _ = dir;
}

/// C3-06 — copies rotated by an earlier boot (0.14.2 credentials in clear)
/// are scrubbed at the next credential boot; their key follows the copies rule.
#[tokio::test]
#[serial]
async fn already_rotated_backups_are_scrubbed() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    let old = write_0142_config_text(&key);
    std::fs::write(
        dir.path()
            .join("config.toml.backup.20261005T203513.668601Z"),
        &old,
    )
    .unwrap();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    boot_like_main(&dir, &db).await;
    let rotated = std::fs::read_to_string(
        dir.path()
            .join("config.toml.backup.20261005T203513.668601Z"),
    )
    .unwrap();
    assert!(
        !rotated.contains(ANTHROPIC) && !rotated.contains(AUTH_TOKEN),
        "{rotated}"
    );
    // Two vault copies on this ladder: the key in use may go from it too.
    assert!(!rotated.contains(&key));
}

/// C3-15 — the key in use in another spelling is still removed from the backup.
#[tokio::test]
#[serial]
async fn the_backup_scrub_compares_keys_canonically() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    let text = write_0142_config_text(&key);
    write_0142_config(dir.path(), &key);
    std::fs::write(
        dir.path().join("config.toml.backup"),
        text.replace(&key, &key.to_uppercase()),
    )
    .unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    boot_like_main(&dir, &db).await;
    let backup = std::fs::read_to_string(dir.path().join("config.toml.backup")).unwrap();
    assert!(!backup.contains(&key.to_uppercase()), "{backup}");
}

/// C3-16 — a write whose read-back fails commits nothing.
#[tokio::test]
#[serial]
async fn a_failed_read_back_commits_nothing() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (mut cfg, _, _) = boot_like_main(&dir, &db).await;
    let before = db.with_conn(rows::list).await.unwrap();
    db.with_conn(|c| {
        c.execute_batch(
            "CREATE TRIGGER garble AFTER INSERT ON stored_credentials BEGIN \
             UPDATE stored_credentials SET value_encrypted = 'garbled' WHERE id = NEW.id; END;",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    cfg.tokens.keys.push(ApiKey {
        id: "new".into(),
        name: "n".into(),
        provider: "openai".into(),
        value: "sk-new".into(),
        active: true,
    });
    cfg.tokens.keys.retain(|k| k.id != "key-anthropic");
    assert!(config::save(&cfg).await.is_err());
    assert_eq!(
        db.with_conn(rows::list).await.unwrap(),
        before,
        "rolled back"
    );
}

// ── review round 5 ──────────────────────────────────────────────────────────

/// C4-07 — one unreadable backup copy does not stop the others from being
/// scrubbed; the error is still returned.
#[test]
fn every_backup_copy_is_scrubbed_even_when_one_fails() {
    let dir = tempfile::tempdir().unwrap();
    let key = crypto::generate_secret();
    let text = write_0142_config_text(&key);
    std::fs::create_dir(dir.path().join("config.toml.backup.20261001T000000Z")).unwrap();
    std::fs::write(
        dir.path().join("config.toml.backup.20261002T000000Z"),
        &text,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("config.toml.backup.20260930T000000Z"),
        &text,
    )
    .unwrap();
    let err = scrub_migration_backup(dir.path(), &key).unwrap_err();
    assert!(
        err.to_string()
            .contains("could not scrub every config backup"),
        "{err}"
    );
    for name in [
        "config.toml.backup.20261002T000000Z",
        "config.toml.backup.20260930T000000Z",
    ] {
        let scrubbed = std::fs::read_to_string(dir.path().join(name)).unwrap();
        assert!(
            !scrubbed.contains(ANTHROPIC) && !scrubbed.contains(AUTH_TOKEN),
            "{name}"
        );
    }
}

/// C4-04 — after "Re-encrypt imported secrets", re-encrypted stored
/// credentials appear at once and no longer count as locked.
#[tokio::test]
#[serial]
async fn reencrypted_stored_credentials_load_at_once() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    boot_like_main(&dir, &db).await;
    // A provider key imported under another machine's key A.
    let a = crypto::generate_secret();
    let enc = crypto::encrypt("sk-imported", &crypto::parse_secret(&a).unwrap()).unwrap();
    db.with_conn(move |conn| {
        rows::upsert_all(
            conn,
            &[StoredCredential {
                kind: KIND_PROVIDER_KEY.into(),
                id: "imported".into(),
                name: "imported".into(),
                provider: "openai".into(),
                active: true,
                position: 9,
                value_encrypted: enc,
            }],
        )
    })
    .await
    .unwrap();
    dir.restart();
    let (cfg, _, _) = boot_like_main(&dir, &db).await;
    assert_eq!(locked_credential_count(dir.path()), 1);
    crate::core::recovery::save_imported_blob(
        dir.path(),
        &crate::core::recovery::wrap_key(&a, "source pass").unwrap(),
    )
    .unwrap();
    let state = state_with(cfg, &db);
    let router = crate::build_router_with_auth(state.clone(), true);
    let (_, body) = json_of(
        router.clone(),
        "POST",
        "/api/config/recovery/reencrypt",
        serde_json::json!({"passphrase": "source pass"}),
    )
    .await;
    assert_eq!(body["success"], true, "{body}");
    let (_, status) = json_of(
        router.clone(),
        "GET",
        "/api/config/recovery/status",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status["data"]["locked_credentials"], 0, "{status}");
    let (_, tokens) = json_of(router, "GET", "/api/config/tokens", serde_json::json!({})).await;
    assert!(tokens.to_string().contains("imported"), "{tokens}");
}

/// C4-11 — a restore with the key in use clears a failure recorded at start.
#[tokio::test]
#[serial]
async fn a_restore_clears_a_recorded_credential_failure() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    let (cfg, _, _) = boot_like_main(&dir, &db).await;
    disarm(dir.path());
    record_boot_failure(dir.path(), Some("disk full".into()));
    let code = crate::core::recovery::to_code(
        &crate::core::recovery::wrap_key(&key, "pass phrase").unwrap(),
    );
    let router = crate::build_router_with_auth(state_with(cfg, &db), true);
    let (_, body) = json_of(
        router.clone(),
        "POST",
        "/api/config/recovery/restore",
        serde_json::json!({"passphrase": "pass phrase", "recovery_code": code}),
    )
    .await;
    assert_eq!(body["success"], true, "{body}");
    let (_, status) = json_of(
        router,
        "GET",
        "/api/config/recovery/status",
        serde_json::json!({}),
    )
    .await;
    assert!(
        status["data"]["credentials_unavailable"].is_null(),
        "{status}"
    );
}

/// C4-06 — an export carrying encrypted MCP secrets with no recovery
/// passphrase says so in its warning header.
#[tokio::test]
#[serial]
async fn an_export_of_secrets_without_recovery_warns() {
    use tower::ServiceExt;
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &key).await;
    let (cfg, _, _) = boot_like_main(&dir, &db).await;
    let router = crate::build_router_with_auth(state_with(cfg, &db), true);
    let mut req = axum::http::Request::builder()
        .uri("/api/config/export")
        .body(axum::body::Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))));
    let res = router.oneshot(req).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    assert_eq!(
        res.headers()
            .get("X-Kronn-Export-Warning")
            .and_then(|v| v.to_str().ok()),
        Some("no-recovery-passphrase")
    );
}

// ── review round 6 ──────────────────────────────────────────────────────────

/// C5-08 — a provider key whose save fails is not left in the live config.
#[tokio::test]
#[serial]
async fn a_failed_api_key_save_leaves_the_live_config_unchanged() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (cfg, _, _) = boot_like_main(&dir, &db).await;
    db.with_conn(|c| {
        c.execute_batch(
            "CREATE TRIGGER garble AFTER INSERT ON stored_credentials BEGIN \
             UPDATE stored_credentials SET value_encrypted = 'garbled' WHERE id = NEW.id; END;",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let router = crate::build_router_with_auth(state_with(cfg, &db), true);
    let (_, saved) = json_of(
        router.clone(),
        "POST",
        "/api/config/api-keys",
        serde_json::json!({"name": "never", "provider": "openai", "value": "sk-never-stored"}),
    )
    .await;
    assert_eq!(saved["success"], false, "{saved}");
    let (_, tokens) = json_of(router, "GET", "/api/config/tokens", serde_json::json!({})).await;
    assert!(!tokens.to_string().contains("never"), "{tokens}");
}

// ── review round 7 ──────────────────────────────────────────────────────────

/// C6-01 — a locked reset whose new key cannot be set up keeps the previous
/// auth (the session token) and no key: a LAN request without a bearer is
/// still refused.
#[tokio::test]
#[serial]
async fn a_failed_locked_reset_keeps_auth_and_no_key() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    migrated_then_key_lost(&dir, &db).await;
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, Some("session-token".into()))
        .await
        .unwrap();
    assert!(cfg.encryption_secret.is_none());
    // The key store becomes unreadable: the reset's reconcile fails.
    std::fs::create_dir(dir.path().join(crate::core::keyvault::SIDECAR_FILENAME)).unwrap();
    let state = state_with(cfg, &db);
    let router = crate::build_router_with_auth(state.clone(), true);
    let (_, body) = json_of(
        router.clone(),
        "POST",
        "/api/setup/reset",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(body["success"], false, "{body}");
    let (status, _) = json_from(
        router.clone(),
        "GET",
        "/api/projects",
        serde_json::json!({}),
        [192, 168, 1, 9],
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
    let live = state.config.read().await;
    assert!(live.encryption_secret.is_none());
    assert_eq!(live.server.auth_token.as_deref(), Some("session-token"));
}

/// C6-02 — key lost for good: the locked secrets go to a kept file, a new key
/// starts, and the projects stay.
#[tokio::test]
#[serial]
async fn locked_secrets_set_aside_and_a_new_key_starts() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let old = migrated_then_key_lost(&dir, &db).await;
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    assert!(
        cfg.server.auth_locked,
        "auth locked: the token row is unreadable"
    );
    let state = state_with(cfg, &db);
    let router = crate::build_router_with_auth(state.clone(), true);
    let (_, body) = json_of(
        router.clone(),
        "POST",
        "/api/config/recovery/start-new-key",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(body["success"], true, "{body}");
    let new_key = state.config.read().await.encryption_secret.clone().unwrap();
    assert!(!crate::core::keyvault::same_key(&new_key, &old));
    let (_, projects) = json_of(router, "GET", "/api/projects", serde_json::json!({})).await;
    assert!(projects.to_string().contains("p1"), "{projects}");
    let kept = body["data"]["kept_file"].as_str().unwrap().to_string();
    let text = std::fs::read_to_string(dir.path().join(&kept)).unwrap();
    assert!(
        text.contains("mcp_configs") && text.contains("stored_credentials"),
        "{text}"
    );
    // The MCP config stays, without its secret values.
    let envs: Vec<String> = db
        .with_conn(|c| {
            let mut s = c.prepare("SELECT env_encrypted FROM mcp_configs")?;
            let v = s
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        })
        .await
        .unwrap();
    assert!(!envs.is_empty() && envs.iter().all(|e| e.is_empty()));
}

/// C6-05 — a connection key whose save fails is reported, not shown as saved.
#[tokio::test]
#[serial]
async fn a_failed_connection_key_save_is_reported() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (cfg, _, _) = boot_like_main(&dir, &db).await;
    let before = cfg.tokens.keys.len();
    db.with_conn(|c| {
        c.execute_batch(
            "CREATE TRIGGER garble AFTER INSERT ON stored_credentials BEGIN \
             UPDATE stored_credentials SET value_encrypted = 'garbled' WHERE id = NEW.id; END;",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let state = state_with(cfg, &db);
    let router = crate::build_router_with_auth(state.clone(), true);
    let (_, created) = json_of(
        router,
        "POST",
        "/api/external-api/connections",
        serde_json::json!({
            "display_name": "OpenRouter",
            "mention_alias": "orfail",
            "endpoint": "https://openrouter.ai/api",
            "origin_preset": "open_router",
            "api_key": "sk-or-v1-neverstoredkey"
        }),
    )
    .await;
    assert_eq!(created["success"], false, "{created}");
    assert_eq!(state.config.read().await.tokens.keys.len(), before);
}

/// C6-05 — discovered keys whose save fails are not left in the live config.
#[tokio::test]
#[serial]
async fn failed_discovered_keys_are_not_kept_live() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (cfg, _, _) = boot_like_main(&dir, &db).await;
    let before = cfg.tokens.keys.len();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".vibe")).unwrap();
    std::fs::write(
        home.path().join(".vibe/.env"),
        "MISTRAL_API_KEY=mistral-discovered-never-stored\n",
    )
    .unwrap();
    let old_home = crate::core::child_env::var("KRONN_HOST_HOME").ok();
    crate::core::child_env::set_var("KRONN_HOST_HOME", home.path());
    db.with_conn(|c| {
        c.execute_batch(
            "CREATE TRIGGER garble AFTER INSERT ON stored_credentials BEGIN \
             UPDATE stored_credentials SET value_encrypted = 'garbled' WHERE id = NEW.id; END;",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let state = state_with(cfg, &db);
    let (_, body) = json_of(
        crate::build_router_with_auth(state.clone(), true),
        "POST",
        "/api/config/discover-keys",
        serde_json::json!({}),
    )
    .await;
    match old_home {
        Some(h) => crate::core::child_env::set_var("KRONN_HOST_HOME", h),
        None => crate::core::child_env::remove_var("KRONN_HOST_HOME"),
    }
    assert_eq!(body["success"], false, "{body}");
    let live = state.config.read().await;
    assert_eq!(live.tokens.keys.len(), before);
    assert!(!live
        .tokens
        .keys
        .iter()
        .any(|k| k.value == "mistral-discovered-never-stored"));
}

/// C6-07 — a schema-invalid 0.14.2 config.toml: its provider key reaches the
/// store, and the notice says the kept file holds credentials in clear.
#[tokio::test]
#[serial]
async fn credentials_of_a_schema_invalid_config_are_recovered() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    // Valid TOML, but a field of the wrong type: not a config this Kronn reads.
    let text = write_0142_config_text(&key).replacen("port = ", "port = \"x\" # ", 1);
    assert!(text.contains("port = \"x\""));
    std::fs::write(dir.path().join("config.toml"), &text).unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    assert!(
        cfg.tokens.keys.iter().any(|k| k.value == ANTHROPIC),
        "{:?}",
        cfg.tokens.keys
    );
    let notice = config::set_aside_notice(dir.path()).unwrap();
    assert!(notice.contains("credentials in clear"), "{notice}");
}

// ── review round 8 ──────────────────────────────────────────────────────────

fn set_docker(on: bool) -> Option<String> {
    let previous = crate::core::child_env::var("KRONN_IN_DOCKER").ok();
    if on {
        crate::core::child_env::set_var("KRONN_IN_DOCKER", "1");
    } else {
        crate::core::child_env::remove_var("KRONN_IN_DOCKER");
    }
    previous
}

fn restore_docker(previous: Option<String>) {
    match previous {
        Some(v) => crate::core::child_env::set_var("KRONN_IN_DOCKER", v),
        None => crate::core::child_env::remove_var("KRONN_IN_DOCKER"),
    }
}

/// C7-01 — under Docker, starting a new key or a locked reset keeps auth on:
/// the new token is returned and a LAN request without it is refused.
#[tokio::test]
#[serial]
async fn auth_stays_on_after_a_new_key_or_a_locked_reset_under_docker() {
    // The locked reset is reachable only with an operator session token.
    for (route, session) in [
        ("/api/config/recovery/start-new-key", None),
        ("/api/setup/reset", Some("session-token".to_string())),
    ] {
        let previous = set_docker(true);
        let dir = DataDir::new();
        let db = Arc::new(Database::open_in_memory().unwrap());
        migrated_then_key_lost(&dir, &db).await;
        let mut cfg = config::load().await.unwrap().unwrap();
        crate::resolve_key_and_credentials(&mut cfg, &db, session)
            .await
            .unwrap();
        assert!(cfg.server.auth_enabled && cfg.encryption_secret.is_none());
        let state = state_with(cfg, &db);
        let router = crate::build_router_with_auth(state.clone(), true);
        let (_, body) = json_of(router.clone(), "POST", route, serde_json::json!({})).await;
        restore_docker(previous);
        assert_eq!(body["success"], true, "{route}: {body}");
        assert!(state.config.read().await.server.auth_enabled, "{route}");
        if route.ends_with("start-new-key") {
            assert!(body["data"]["auth_token"].is_string(), "{body}");
        }
        let (status, _) = json_from(
            router,
            "GET",
            "/api/projects",
            serde_json::json!({}),
            [192, 168, 1, 9],
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED, "{route}");
    }
}

/// C7-02 — rows set aside when the key was given up come back once its
/// passphrase is offered to "Re-encrypt imported secrets".
#[tokio::test]
#[serial]
async fn rows_set_aside_come_back_with_the_old_key() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let old = migrated_then_key_lost(&dir, &db).await;
    crate::core::recovery::save_blob(
        dir.path(),
        &crate::core::recovery::wrap_key(&old, "old passphrase").unwrap(),
    )
    .unwrap();
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    let state = state_with(cfg, &db);
    let router = crate::build_router_with_auth(state.clone(), true);
    let (_, fresh) = json_of(
        router.clone(),
        "POST",
        "/api/config/recovery/start-new-key",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(fresh["success"], true, "{fresh}");
    let (_, status) = json_of(
        router.clone(),
        "GET",
        "/api/config/recovery/status",
        serde_json::json!({}),
    )
    .await;
    assert!(
        status["data"]["locked_file_rows"].as_u64().unwrap() > 0,
        "{status}"
    );
    let (_, back) = json_of(
        router.clone(),
        "POST",
        "/api/config/recovery/reencrypt",
        serde_json::json!({"passphrase": "old passphrase"}),
    )
    .await;
    assert_eq!(back["success"], true, "{back}");
    let new_key = state.config.read().await.encryption_secret.clone().unwrap();
    let parsed = crypto::parse_secret(&new_key).unwrap();
    let (mcp, snap, gh): (String, String, String) = db
        .with_conn(|c| {
            Ok((
                c.query_row("SELECT env_encrypted FROM mcp_configs", [], |r| r.get(0))?,
                c.query_row(
                    "SELECT values_encrypted FROM execution_variable_snapshots",
                    [],
                    |r| r.get(0),
                )?,
                c.query_row(
                    "SELECT token_encrypted FROM project_github_connections",
                    [],
                    |r| r.get(0),
                )?,
            ))
        })
        .await
        .unwrap();
    assert!(crate::db::mcps::decrypt_env(&mcp, &new_key)
        .unwrap()
        .values()
        .any(|v| v == MCP_SECRET));
    assert_eq!(crypto::decrypt(&snap, &parsed).unwrap(), SNAPSHOT_SECRET);
    assert!(crypto::decrypt(&gh, &parsed).is_ok());
    let (_, tokens) = json_of(router, "GET", "/api/config/tokens", serde_json::json!({})).await;
    assert!(tokens.to_string().contains("Personal API Key"), "{tokens}");
}

/// C7-05 — a resolve that ends locked adopts nothing.
#[tokio::test]
#[serial]
async fn a_locked_resolve_adopts_nothing() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &crypto::generate_secret()).await;
    let mut live = config::default_config_without_key();
    live.server.auth_token = Some("session".into());
    live.server.auth_token_session_only = true;
    live.server.auth_enabled = true;
    let before = live.clone();
    assert!(
        crate::api::setup::resolve_fresh_key(&mut live, &db, dir.path())
            .await
            .is_err()
    );
    assert!(live.encryption_secret.is_none());
    assert_eq!(live.server.auth_token, before.server.auth_token);
    assert_eq!(live.server.auth_enabled, before.server.auth_enabled);
    assert!(live.server.auth_token_session_only);
}

/// C7-06 — a readable config.toml token survives a new key.
#[tokio::test]
#[serial]
async fn a_readable_config_token_survives_a_new_key() {
    let dir = DataDir::new();
    let lost = crypto::generate_secret();
    let text = write_0142_config_text(&lost)
        .lines()
        .filter(|l| !l.starts_with("encryption_secret"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.path().join("config.toml"), text).unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed_ciphertext(&db, &lost).await;
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    assert!(cfg.encryption_secret.is_none());
    assert_eq!(cfg.server.auth_token.as_deref(), Some(AUTH_TOKEN));
    let state = state_with(cfg, &db);
    let (_, body) = json_of(
        crate::build_router_with_auth(state.clone(), true),
        "POST",
        "/api/config/recovery/start-new-key",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(body["success"], true, "{body}");
    let live = state.config.read().await;
    assert_eq!(live.server.auth_token.as_deref(), Some(AUTH_TOKEN));
    let key = live.encryption_secret.clone().unwrap();
    let stored = db.with_conn(rows::list).await.unwrap();
    let row = stored.iter().find(|r| r.kind == KIND_AUTH_TOKEN).unwrap();
    assert_eq!(
        decrypt_value(&row.value_encrypted, &key).unwrap(),
        AUTH_TOKEN
    );
}

/// C7-08 — a native schema-invalid 0.14.2 file keeps its auth choice.
#[tokio::test]
#[serial]
async fn a_schema_invalid_config_keeps_auth_enabled() {
    let previous = set_docker(false);
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    let text = write_0142_config_text(&key).replacen("port = ", "port = \"x\" # ", 1);
    std::fs::write(dir.path().join("config.toml"), &text).unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    restore_docker(previous);
    assert!(cfg.server.auth_enabled);
    assert_eq!(cfg.server.auth_token.as_deref(), Some(AUTH_TOKEN));
}

fn garble_writes(c: &rusqlite::Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "CREATE TRIGGER garble AFTER INSERT ON stored_credentials BEGIN \
         UPDATE stored_credentials SET value_encrypted = 'garbled' WHERE id = NEW.id; END; \
         CREATE TRIGGER no_delete BEFORE DELETE ON stored_credentials BEGIN \
         SELECT RAISE(ABORT, 'disk full'); END;",
    )
}

/// C7-10 — connection update and delete whose save fails leave the live
/// keys as stored.
#[tokio::test]
#[serial]
async fn failed_connection_update_and_delete_leave_the_live_keys() {
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (cfg, _, _) = boot_like_main(&dir, &db).await;
    let state = state_with(cfg, &db);
    let router = crate::build_router_with_auth(state.clone(), true);
    let (_, created) = json_of(
        router.clone(),
        "POST",
        "/api/external-api/connections",
        serde_json::json!({
            "display_name": "OpenRouter",
            "mention_alias": "orkeep",
            "endpoint": "https://openrouter.ai/api",
            "origin_preset": "open_router",
            "api_key": "sk-or-v1-firstkey"
        }),
    )
    .await;
    assert_eq!(created["success"], true, "{created}");
    let id = created["data"]["id"].as_str().unwrap().to_string();
    let before: Vec<String> = state
        .config
        .read()
        .await
        .tokens
        .keys
        .iter()
        .map(|k| k.value.clone())
        .collect();
    db.with_conn(|c| Ok(garble_writes(c)?)).await.unwrap();
    let (_, updated) = json_of(
        router.clone(),
        "PUT",
        &format!("/api/external-api/connections/{id}"),
        serde_json::json!({
            "display_name": "OpenRouter",
            "mention_alias": "orkeep",
            "endpoint": "https://openrouter.ai/api",
            "origin_preset": "open_router",
            "api_key": "sk-or-v1-secondkey"
        }),
    )
    .await;
    assert_eq!(updated["success"], false, "{updated}");
    let (_, deleted) = json_of(
        router,
        "DELETE",
        &format!("/api/external-api/connections/{id}"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(deleted["success"], false, "{deleted}");
    let after: Vec<String> = state
        .config
        .read()
        .await
        .tokens
        .keys
        .iter()
        .map(|k| k.value.clone())
        .collect();
    assert_eq!(after, before);
}

/// C7-10 — LiteLLM and the standalone start-up discovery adopt their copy
/// only inside the successful-save arm.
#[test]
fn litellm_and_startup_discovery_adopt_only_once_saved() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (file, adopt) in [
        ("src/api/lite_llm.rs", "*live = cfg;"),
        ("src/main.rs", "*live = config;"),
    ] {
        let text = std::fs::read_to_string(root.join(file)).unwrap();
        assert_eq!(text.matches(adopt).count(), 1, "{file}");
        let at = text.find(adopt).unwrap();
        let before = &text[..at];
        let ok = before.rfind("Ok(_) => {").unwrap();
        assert!(
            !before[ok..].contains("Err("),
            "{file}: the adoption must sit in the Ok arm"
        );
        assert!(
            text.contains("live.clone()"),
            "{file}: changes go to a copy"
        );
    }
}

// ── final review ────────────────────────────────────────────────────────────

/// CF-01 — a credential boot that fails after the token row went never opens
/// auth: nothing is adopted and a LAN request is still refused.
#[tokio::test]
#[serial]
async fn a_failed_credential_store_after_a_new_key_keeps_auth_closed() {
    for (route, session) in [
        ("/api/config/recovery/start-new-key", None),
        ("/api/setup/reset", Some("session-token".to_string())),
    ] {
        let previous = set_docker(true);
        let dir = DataDir::new();
        let db = Arc::new(Database::open_in_memory().unwrap());
        migrated_then_key_lost(&dir, &db).await;
        let mut cfg = config::load().await.unwrap().unwrap();
        crate::resolve_key_and_credentials(&mut cfg, &db, session.clone())
            .await
            .unwrap();
        // Only inserts fail: the set-aside and forget_all delete freely.
        db.with_conn(|c| {
            c.execute_batch(
                "CREATE TRIGGER garble AFTER INSERT ON stored_credentials BEGIN \
                 UPDATE stored_credentials SET value_encrypted = 'garbled' WHERE id = NEW.id; END;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let state = state_with(cfg, &db);
        let router = crate::build_router_with_auth(state.clone(), true);
        let (_, body) = json_of(router.clone(), "POST", route, serde_json::json!({})).await;
        restore_docker(previous);
        {
            let live = state.config.read().await;
            if session.is_none() {
                // No token at all: nothing may be adopted.
                assert_eq!(body["success"], false, "{route}: {body}");
                assert!(live.encryption_secret.is_none(), "{route}");
            } else {
                // The operator's session token keeps serving: auth stays closed.
                assert_eq!(live.server.auth_token.as_deref(), Some("session-token"));
            }
        }
        let (status, _) = json_from(
            router,
            "GET",
            "/api/projects",
            serde_json::json!({}),
            [192, 168, 1, 9],
        )
        .await;
        assert_ne!(status, axum::http::StatusCode::OK, "{route}");
    }
}

/// CF-01 — auth on with no token loaded, after a failed credential boot, is
/// locked, never open.
#[tokio::test]
#[serial]
async fn a_failed_boot_with_auth_on_and_no_token_locks_auth() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    db.with_conn(|c| Ok(garble_writes(c)?)).await.unwrap();
    let mut cfg = config::default_config_without_key();
    cfg.server.auth_enabled = true;
    cfg.server.auth_token = None;
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    assert!(!is_armed(dir.path()));
    assert!(cfg.server.auth_locked);
}

/// CF-02 — rows put back from a locked-secrets file survive a failed reload
/// and a later save, and their file is not marked restored.
#[tokio::test]
#[serial]
async fn restored_rows_survive_a_failed_reload() {
    let dir = DataDir::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let old = migrated_then_key_lost(&dir, &db).await;
    crate::core::recovery::save_blob(
        dir.path(),
        &crate::core::recovery::wrap_key(&old, "old passphrase").unwrap(),
    )
    .unwrap();
    let mut cfg = config::load().await.unwrap().unwrap();
    crate::resolve_key_and_credentials(&mut cfg, &db, None)
        .await
        .unwrap();
    let state = state_with(cfg, &db);
    let router = crate::build_router_with_auth(state.clone(), true);
    let (_, fresh) = json_of(
        router.clone(),
        "POST",
        "/api/config/recovery/start-new-key",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(fresh["success"], true, "{fresh}");
    FAIL_BOOT.lock().unwrap().insert(dir.path().to_path_buf());
    let (_, back) = json_of(
        router,
        "POST",
        "/api/config/recovery/reencrypt",
        serde_json::json!({"passphrase": "old passphrase"}),
    )
    .await;
    FAIL_BOOT.lock().unwrap().remove(dir.path());
    assert_eq!(back["success"], false, "{back}");
    // A save that writes the table (a key added) must not drop the rows.
    let mut snapshot = state.config.read().await.clone();
    snapshot.tokens.keys.push(ApiKey {
        id: "later".into(),
        name: "later".into(),
        provider: "openai".into(),
        value: "sk-later".into(),
        active: true,
    });
    config::save(&snapshot).await.unwrap();
    let stored = db.with_conn(rows::list).await.unwrap();
    assert!(
        stored
            .iter()
            .any(|r| r.kind == KIND_PROVIDER_KEY && r.id == "key-anthropic"),
        "{stored:?}"
    );
    assert!(crate::core::keystore::locked_file_rows(dir.path()) > 0);
}

/// CF-09 — an export says the locked-secrets files stay behind.
#[tokio::test]
#[serial]
async fn an_export_warns_about_locked_secrets_files() {
    use tower::ServiceExt;
    let dir = DataDir::new();
    let key = crypto::generate_secret();
    write_0142_config(dir.path(), &key);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (cfg, _, _) = boot_like_main(&dir, &db).await;
    std::fs::write(
        dir.path().join("locked-secrets-20261006T000000Z.json"),
        r#"[{"table":"mcp_configs","column":"env_encrypted","row":{"id":"x"}}]"#,
    )
    .unwrap();
    let router = crate::build_router_with_auth(state_with(cfg, &db), true);
    let mut req = axum::http::Request::builder()
        .uri("/api/config/export")
        .body(axum::body::Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))));
    let res = router.oneshot(req).await.unwrap();
    let header = res
        .headers()
        .get("X-Kronn-Export-Warning")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(header.contains("locked-secrets-not-exported"), "{header}");
}
