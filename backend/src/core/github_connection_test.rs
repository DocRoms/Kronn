use super::*;
use serial_test::serial;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CLASSIC: &str = "ghp_classicTOKENvalue0123456789";
const FINE: &str = "github_pat_11FINEgrainedTOKEN_value0123456789";

#[test]
fn github_slugs_from_every_remote_form() {
    for (url, slug) in [
        ("https://github.com/Octo/Repo.git", Some("Octo/Repo")),
        ("https://github.com/octo/repo/", Some("octo/repo")),
        ("http://www.github.com/octo/repo", Some("octo/repo")),
        ("git@github.com:octo/réseau.git", Some("octo/réseau")),
        ("ssh://git@github.com/octo/repo.git", Some("octo/repo")),
        (
            "ssh://git@ssh.github.com:443/octo/repo.git",
            Some("octo/repo"),
        ),
        ("https://user:pw@github.com/octo/repo", Some("octo/repo")),
        ("https://gitlab.com/octo/repo", None),
        ("https://github.example.com/octo/repo", None),
        ("https://notgithub.com/octo/repo", None),
        ("git@gitlab.com:octo/repo.git", None),
        ("file:///srv/github.com/octo/repo", None),
        ("https://github.com/octo", None),
        ("https://github.com/octo/repo/tree/main", None),
        ("", None),
    ] {
        assert_eq!(github_repo_slug(url).as_deref(), slug, "{url}");
    }
}

#[test]
fn remotes_are_read_from_a_worktree_pointer_too() {
    let main = tempfile::tempdir().unwrap();
    let git_dir = main.path().join(".git");
    std::fs::create_dir_all(git_dir.join("worktrees/wt")).unwrap();
    std::fs::write(
        git_dir.join("config"),
        "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://github.com/octo/repo.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
    )
    .unwrap();
    std::fs::write(git_dir.join("worktrees/wt/commondir"), "../..\n").unwrap();
    let worktree = tempfile::tempdir().unwrap();
    std::fs::write(
        worktree.path().join(".git"),
        format!("gitdir: {}\n", git_dir.join("worktrees/wt").display()),
    )
    .unwrap();

    assert_eq!(
        git_remote_urls(main.path()),
        vec!["https://github.com/octo/repo.git".to_string()]
    );
    assert_eq!(
        git_remote_urls(worktree.path()),
        git_remote_urls(main.path())
    );
    let path = worktree.path().to_string_lossy().into_owned();
    assert!(project_is_on_github(&path, None));
    assert!(project_is_on_github(
        "/nowhere",
        Some("git@github.com:o/r.git")
    ));
    assert!(!project_is_on_github(
        "/nowhere",
        Some("https://gitlab.com/o/r")
    ));
    assert!(!project_is_on_github("/nowhere", None));
}

#[test]
fn token_kinds_follow_github_prefixes() {
    assert_eq!(token_kind(CLASSIC), GithubTokenKind::Classic);
    assert_eq!(token_kind("gho_abc"), GithubTokenKind::Oauth);
    assert_eq!(token_kind(FINE), GithubTokenKind::FineGrained);
    assert_eq!(token_kind("ghs_abc"), GithubTokenKind::App);
    assert_eq!(token_kind("ghu_abc"), GithubTokenKind::App);
    assert_eq!(token_kind("abcdef"), GithubTokenKind::Unknown);
}

#[test]
fn pasted_tokens_are_validated_without_echoing_them() {
    assert_eq!(validate_pasted_token(&format!("  {FINE}\n")).unwrap(), FINE);
    for bad in ["", "   ", "ghp_with space", "ghp_é", "ghp_\"quote"] {
        let error = validate_pasted_token(bad).unwrap_err();
        assert!(!bad.trim().is_empty() || error.contains("empty"));
        if !bad.trim().is_empty() {
            assert!(
                !error.contains(bad.trim()),
                "error echoes the token: {error}"
            );
        }
    }
    assert!(validate_pasted_token(&"a".repeat(MAX_TOKEN_CHARS + 1)).is_err());
}

#[tokio::test]
async fn classic_scope_comes_from_the_oauth_scopes_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .and(header(
            "authorization",
            format!("Bearer {CLASSIC}").as_str(),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-OAuth-Scopes", "repo, read:org,  gist")
                .set_body_json(serde_json::json!({ "login": "octo" })),
        )
        .mount(&server)
        .await;

    let scope = verify_scope(&server.uri(), CLASSIC).await;
    assert!(scope.verified);
    assert_eq!(scope.token_kind, GithubTokenKind::Classic);
    assert_eq!(scope.login.as_deref(), Some("octo"));
    assert_eq!(scope.scopes, vec!["repo", "read:org", "gist"]);
    assert!(scope.broad, "`repo` reaches every repository");
    assert!(scope.reason.is_none());
}

#[tokio::test]
async fn a_classic_token_without_repo_scopes_is_not_broad() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-OAuth-Scopes", "")
                .set_body_json(serde_json::json!({ "login": "octo" })),
        )
        .mount(&server)
        .await;
    let scope = verify_scope(&server.uri(), CLASSIC).await;
    assert!(scope.verified);
    assert!(scope.scopes.is_empty());
    assert!(!scope.broad);
}

#[tokio::test]
async fn fine_grained_scope_lists_the_reachable_repositories() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "login": "octo" })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("per_page", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "full_name": "octo/kronn" },
            { "full_name": "octo/réseau" }
        ])))
        .mount(&server)
        .await;

    let scope = verify_scope(&server.uri(), FINE).await;
    assert!(scope.verified, "{:?}", scope.reason);
    assert_eq!(scope.token_kind, GithubTokenKind::FineGrained);
    assert_eq!(scope.repositories, vec!["octo/kronn", "octo/réseau"]);
    assert!(!scope.repositories_truncated);
    assert!(!scope.broad);
    assert!(scope.scopes.is_empty());
}

#[tokio::test]
async fn a_next_page_marks_the_repository_list_truncated() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "login": "octo" })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "Link",
                    "<https://api.github.com/user/repos?page=2>; rel=\"next\"",
                )
                .set_body_json(serde_json::json!([{ "full_name": "octo/a" }])),
        )
        .mount(&server)
        .await;
    let scope = verify_scope(&server.uri(), FINE).await;
    assert!(scope.repositories_truncated);
}

#[tokio::test]
async fn unverifiable_scopes_say_why_and_never_quote_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let scope = verify_scope(&server.uri(), CLASSIC).await;
    assert!(!scope.verified);
    assert!(scope.reason.as_deref().unwrap().contains("401"));

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let scope = verify_scope(&server.uri(), "ghs_installationTOKEN").await;
    assert!(!scope.verified);
    assert_eq!(scope.token_kind, GithubTokenKind::App);
    let reason = scope.reason.unwrap();
    assert!(reason.contains("403"), "{reason}");
    assert!(!reason.contains("installationTOKEN"));

    // Nothing listening: a transport error, still without the token.
    let scope = verify_scope("http://127.0.0.1:9", CLASSIC).await;
    assert!(!scope.verified);
    assert!(!scope.reason.as_deref().unwrap().contains(CLASSIC));
    assert!(!serde_json::to_string(&scope).unwrap().contains(CLASSIC));
}

#[tokio::test]
#[serial(github_machine_token)]
async fn env_for_launch_follows_each_state() {
    override_machine_token_for_tests(Some(Some("gho_machineTOKEN")));
    let none = "r16-env-none";
    let off = "r16-env-off";
    let login = "r16-env-login";
    let stored = "r16-env-stored";
    set_grant(off, GithubConnectionMode::NotConnected, None);
    set_grant(login, GithubConnectionMode::GhLogin, None);
    set_grant(stored, GithubConnectionMode::StoredToken, Some(FINE));

    assert!(
        env_for_launch(None).await.is_empty(),
        "no project, no token"
    );
    assert!(
        env_for_launch(Some(none)).await.is_empty(),
        "new projects: not connected"
    );
    assert!(env_for_launch(Some(off)).await.is_empty());

    let names = |env: &[(String, String)]| env.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
    let env = env_for_launch(Some(login)).await;
    assert_eq!(names(&env), GITHUB_ENV_NAMES.to_vec());
    assert!(env.iter().all(|(_, v)| v == "gho_machineTOKEN"));
    let env = env_for_launch(Some(stored)).await;
    assert!(
        env.iter().all(|(_, v)| v == FINE),
        "a stored token wins over the machine's"
    );

    // gh login connected but no token on the machine: nothing to give.
    override_machine_token_for_tests(Some(None));
    assert!(env_for_launch(Some(login)).await.is_empty());

    // Turning off reaches the next launch.
    set_grant(stored, GithubConnectionMode::NotConnected, None);
    assert!(env_for_launch(Some(stored)).await.is_empty());
    // A stored-token grant without a token is not a grant.
    set_grant(stored, GithubConnectionMode::StoredToken, None);
    assert!(env_for_launch(Some(stored)).await.is_empty());
    override_machine_token_for_tests(None);
}

#[tokio::test]
#[serial(github_machine_token)]
async fn boot_loads_grants_and_decrypts_stored_tokens() {
    let db = crate::db::Database::open_in_memory().unwrap();
    let secret = crate::core::crypto::generate_secret();
    let cipher = encrypt_stored_token(FINE, &secret).unwrap();
    assert!(!cipher.contains("FINEgrained"));
    assert_eq!(*decrypt_stored_token(&cipher, &secret).unwrap(), FINE);
    db.with_conn(move |conn| {
        for id in ["r16-boot-stored", "r16-boot-login", "r16-boot-broken"] {
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES (?1, ?1, '/nowhere/' || ?1, datetime('now'), datetime('now'))",
                [id],
            )?;
        }
        crate::db::github_connections::set_mode(
            conn,
            "r16-boot-stored",
            GithubConnectionMode::StoredToken,
            Some(&cipher),
        )?;
        crate::db::github_connections::set_mode(
            conn,
            "r16-boot-login",
            GithubConnectionMode::GhLogin,
            None,
        )?;
        crate::db::github_connections::set_mode(
            conn,
            "r16-boot-broken",
            GithubConnectionMode::StoredToken,
            Some("not-a-ciphertext"),
        )?;
        Ok(())
    })
    .await
    .unwrap();

    override_machine_token_for_tests(Some(Some("gho_machineTOKEN")));
    assert_eq!(load_grants(&db, Some(&secret)).await.unwrap(), 3);
    let stored = env_for_launch(Some("r16-boot-stored")).await;
    assert_eq!(stored[0].1, FINE);
    assert_eq!(
        env_for_launch(Some("r16-boot-login")).await[0].1,
        "gho_machineTOKEN"
    );
    assert!(
        env_for_launch(Some("r16-boot-broken")).await.is_empty(),
        "an undecryptable token fails closed"
    );
    override_machine_token_for_tests(None);
}

#[tokio::test]
#[serial(github_machine_token)]
async fn no_token_value_reaches_the_logs() {
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> MakeWriter<'a> for Sink {
        type Writer = Sink;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    let sink = Sink::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(sink.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let db = crate::db::Database::open_in_memory().unwrap();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('r16-log', 'r16-log', '/nowhere', datetime('now'), datetime('now'))",
            [],
        )?;
        crate::db::github_connections::set_mode(
            conn,
            "r16-log",
            GithubConnectionMode::StoredToken,
            Some(FINE),
        )
    })
    .await
    .unwrap();
    // Wrong key: the warning path runs.
    load_grants(&db, Some(&crate::core::crypto::generate_secret()))
        .await
        .unwrap();
    set_grant("r16-log", GithubConnectionMode::StoredToken, Some(FINE));
    let _ = env_for_launch(Some("r16-log")).await;
    let _ = verify_scope("http://127.0.0.1:9", FINE).await;
    let request = crate::models::SetProjectGithubConnectionRequest {
        mode: GithubConnectionMode::StoredToken,
        token: Some(FINE.into()),
    };
    tracing::info!("request: {request:?}");

    let logs = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("could not be decrypted"), "{logs}");
    assert!(logs.contains("<redacted>"), "{logs}");
    assert!(!logs.contains("FINEgrainedTOKEN"), "token in logs: {logs}");
}

#[test]
fn launch_env_drops_inherited_tokens_and_keeps_explicit_ones() {
    let value = |cmd: &std::process::Command, name: &str| {
        cmd.get_envs()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.map(|v| v.to_string_lossy().into_owned()))
    };
    // Not connected: every GitHub variable is explicitly removed.
    let mut cmd = std::process::Command::new("true");
    apply_launch_env(&mut cmd, &[]);
    for name in GITHUB_ENV_NAMES {
        assert_eq!(value(&cmd, name), Some(None), "{name} must be removed");
    }

    // A project MCP's own GITHUB_TOKEN survives an unconnected launch.
    let mut cmd = std::process::Command::new("true");
    cmd.env("GITHUB_TOKEN", "mcp-configured");
    apply_launch_env(&mut cmd, &[]);
    assert_eq!(
        value(&cmd, "GITHUB_TOKEN"),
        Some(Some("mcp-configured".into()))
    );
    assert_eq!(value(&cmd, "GH_TOKEN"), Some(None));

    // Connected: the project's token is set on all three names.
    let mut cmd = std::process::Command::new("true");
    let env: Vec<(String, String)> = GITHUB_ENV_NAMES
        .iter()
        .map(|name| (name.to_string(), "gho_project".to_string()))
        .collect();
    apply_launch_env(&mut cmd, &env);
    for name in GITHUB_ENV_NAMES {
        assert_eq!(value(&cmd, name), Some(Some("gho_project".into())));
    }
}
