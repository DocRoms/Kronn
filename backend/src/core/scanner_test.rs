#[cfg(test)]
mod tests {
    use crate::core::scanner::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn resolve_host_path_no_env() {
        // Without KRONN_HOST_HOME, paths should pass through unchanged
        std::env::remove_var("KRONN_HOST_HOME");
        let result = resolve_host_path("/some/local/path");
        assert_eq!(result.to_string_lossy(), "/some/local/path");
    }

    #[test]
    fn detect_audit_status_no_ai_dir() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-no-dir");
        let _ = std::fs::create_dir_all(&tmp);
        // No ai/ dir → NoTemplate
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::NoTemplate));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_with_bootstrap() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-bootstrap");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(
            ai_dir.join("index.md"),
            "# Project\nKRONN:BOOTSTRAP:START\n",
        )
        .unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(
            status,
            crate::models::AiAuditStatus::TemplateInstalled
        ));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_bootstrapped() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp = tmp_dir.path();
        let ai_dir = tmp.join("ai");
        std::fs::create_dir_all(&ai_dir).unwrap();
        std::fs::write(
            ai_dir.join("index.md"),
            "# Project\n<!-- KRONN:BOOTSTRAPPED:2026-03-14 -->\n",
        )
        .unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::Bootstrapped));
    }

    #[test]
    fn detect_audit_status_bootstrapped_and_validated() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-bootstrapped-validated");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(ai_dir.join("index.md"), "# Project\n<!-- KRONN:BOOTSTRAPPED:2026-03-14 -->\n<!-- KRONN:VALIDATED:2026-03-14 -->\n").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::Validated));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_with_placeholder() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-placeholder");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(ai_dir.join("index.md"), "# {{PROJECT_NAME}}\n").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(
            status,
            crate::models::AiAuditStatus::TemplateInstalled
        ));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_instructional_braces_not_placeholder() {
        // Text that mentions {{...}} as an instruction (not a real placeholder) should NOT
        // trigger the placeholder check. When combined with a real audit record in
        // .kronn.json the project should resolve to Audited.
        let tmp = std::env::temp_dir().join("kronn-test-audit-instr-braces");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(
            ai_dir.join("index.md"),
            "# My Project\nIf you see an unfilled `{{...}}`, say NOT_FOUND.\n",
        )
        .unwrap();
        crate::core::kronn_state::record_audit(&tmp, "full").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(
            matches!(status, crate::models::AiAuditStatus::Audited),
            "Instructional {{...}} + .kronn.json audit entry should yield Audited, got {:?}",
            status
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_validated() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-validated");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(ai_dir.join("index.md"), "# Project\nKRONN:VALIDATED\n").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::Validated));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_audited() {
        // 0.8.4 — Audited now requires a recorded audit in `.kronn.json`,
        // or a legacy `checksums.json`, or a legacy KRONN: marker.
        // A plain `index.md` no longer counts (that was the bug: any project
        // with a pre-existing `docs/AGENTS.md` was tagged green).
        let tmp = std::env::temp_dir().join("kronn-test-audit-audited");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(ai_dir.join("index.md"), "# My Project\nFilled content\n").unwrap();
        crate::core::kronn_state::record_audit(&tmp, "full").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::Audited));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // 0.8.4 — regression coverage for the false-positive "Audited" bug.
    #[test]
    fn detect_audit_status_plain_docs_without_audit_is_template_installed() {
        // A project with a user-written `docs/AGENTS.md` and no Kronn artefact
        // (no .kronn.json, no checksums.json, no KRONN: marker) must NOT be
        // reported as Audited — that was the symptom on AMP_EASY_BACKO and
        // DEMOCRATISCORE_WEB before this fix.
        let tmp = std::env::temp_dir().join("kronn-test-audit-plain-docs");
        let docs_dir = tmp.join("docs");
        let _ = std::fs::create_dir_all(&docs_dir);
        std::fs::write(
            docs_dir.join("AGENTS.md"),
            "# My Project\nUser-written documentation, no Kronn involvement.\n",
        )
        .unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(
            matches!(status, crate::models::AiAuditStatus::TemplateInstalled),
            "Plain docs/AGENTS.md without Kronn artefact must be TemplateInstalled, got {:?}",
            status
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_kronn_state_validated() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-state-validated");
        let docs_dir = tmp.join("docs");
        let _ = std::fs::create_dir_all(&docs_dir);
        std::fs::write(docs_dir.join("AGENTS.md"), "# Project\nContent\n").unwrap();
        let mut state = crate::core::kronn_state::KronnState {
            validated_at: Some("2026-05-16".into()),
            ..Default::default()
        };
        crate::core::kronn_state::write(&tmp, &mut state).unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::Validated));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_kronn_state_bootstrapped() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-state-bootstrapped");
        let docs_dir = tmp.join("docs");
        let _ = std::fs::create_dir_all(&docs_dir);
        std::fs::write(docs_dir.join("AGENTS.md"), "# Project\nContent\n").unwrap();
        let mut state = crate::core::kronn_state::KronnState {
            bootstrapped_at: Some("2026-05-16".into()),
            ..Default::default()
        };
        crate::core::kronn_state::write(&tmp, &mut state).unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::Bootstrapped));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn completed_or_attested_audit_advances_a_bootstrapped_project() {
        let tmp = std::env::temp_dir().join(format!(
            "kronn-test-audit-after-bootstrap-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(tmp.join("docs")).unwrap();
        std::fs::write(tmp.join("docs/AGENTS.md"), "# Project\nContent\n").unwrap();
        crate::core::kronn_state::mark_bootstrapped(&tmp).unwrap();
        crate::core::kronn_state::record_audit(&tmp, "full").unwrap();
        assert!(matches!(
            detect_audit_status(&tmp.to_string_lossy()),
            crate::models::AiAuditStatus::Audited
        ));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_legacy_checksums_promotes_to_audited() {
        // A project audited before `.kronn.json` existed has `docs/checksums.json`
        // but no state file. We must keep recognising it as Audited so the badge
        // doesn't regress on upgrade.
        let tmp = std::env::temp_dir().join("kronn-test-audit-legacy-checksums");
        let docs_dir = tmp.join("docs");
        let _ = std::fs::create_dir_all(&docs_dir);
        std::fs::write(docs_dir.join("AGENTS.md"), "# Project\nFilled\n").unwrap();
        let checksums = crate::core::checksums::ChecksumsFile {
            audited_at: "2026-01-01T00:00:00Z".to_string(),
            mappings: vec![],
        };
        std::fs::write(
            docs_dir.join("checksums.json"),
            serde_json::to_string(&checksums).unwrap(),
        )
        .unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(
            matches!(status, crate::models::AiAuditStatus::Audited),
            "legacy checksums.json must keep the project Audited, got {:?}",
            status
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn count_ai_todos_empty() {
        let tmp = std::env::temp_dir().join("kronn-test-todos-empty");
        let _ = std::fs::create_dir_all(&tmp);
        assert_eq!(count_ai_todos(&tmp.to_string_lossy()), 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn count_ai_todos_with_markers() {
        let tmp = std::env::temp_dir().join("kronn-test-todos-markers");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(
            ai_dir.join("index.md"),
            "# Project\n<!-- TODO: verify -->\nSome text\n<!-- TODO: check -->\n",
        )
        .unwrap();
        assert_eq!(count_ai_todos(&tmp.to_string_lossy()), 2);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─── count_tech_debt ──────────────────────────────────────────────────

    #[test]
    fn count_tech_debt_no_docs() {
        // No docs/ → 0 (graceful empty).
        let tmp = std::env::temp_dir().join("kronn-test-td-none");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::create_dir_all(&tmp);
        assert_eq!(count_tech_debt(&tmp.to_string_lossy()), 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn count_tech_debt_files_and_index_rows() {
        // 3 TD detail files. The index has 4 TD rows; 2 of them
        // mirror existing detail files (`one`, `two`), 2 are
        // file-less entries (`extra-1`, `extra-2`). Deduped count:
        //   {one, two, three} ∪ {one, two, extra-1, extra-2}
        //   = {one, two, three, extra-1, extra-2} = 5
        // The README.md and TEMPLATE.md in the same folder must NOT
        // be counted (they're scaffolding, not actual debt items).
        let tmp = std::env::temp_dir().join("kronn-test-td-mix");
        let _ = std::fs::remove_dir_all(&tmp);
        let docs = tmp.join("docs");
        let td = docs.join("tech-debt");
        std::fs::create_dir_all(&td).unwrap();
        std::fs::write(td.join("TD-20260101-one.md"), "# one\n").unwrap();
        std::fs::write(td.join("TD-20260102-two.md"), "# two\n").unwrap();
        std::fs::write(td.join("TD-20260103-three.md"), "# three\n").unwrap();
        std::fs::write(td.join("README.md"), "# folder readme\n").unwrap();
        std::fs::write(td.join("TEMPLATE.md"), "# template\n").unwrap();
        std::fs::write(
            docs.join("inconsistencies-tech-debt.md"),
            "# Index\n\n| ID | Problem | Area | Severity |\n|---|---|---|---|\n\
             | TD-20260101-one | mirrors a detail file | Backend | medium |\n\
             | TD-20260102-two | also a mirror | Frontend | low |\n\
             | TD-20260104-extra-1 | index-only entry | Other | low |\n\
             | TD-20260105-extra-2 | index-only entry | Other | low |\n\
             | Not a TD row | ignored | - | - |\n",
        )
        .unwrap();
        // 3 unique file IDs + 2 index-only IDs = 5 (the 2 mirrored
        // rows collapse onto their detail files).
        assert_eq!(count_tech_debt(&tmp.to_string_lossy()), 5);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn count_tech_debt_dedupes_file_and_index_pair() {
        // Sanity: when every detail file ALSO has a matching index
        // row (the well-documented common case), the count is the
        // number of files, NOT files + rows. This catches the 0.8.1
        // double-counting regression flagged by the user.
        let tmp = std::env::temp_dir().join("kronn-test-td-dedupe");
        let _ = std::fs::remove_dir_all(&tmp);
        let docs = tmp.join("docs");
        let td = docs.join("tech-debt");
        std::fs::create_dir_all(&td).unwrap();
        std::fs::write(td.join("TD-20260201-alpha.md"), "# alpha\n").unwrap();
        std::fs::write(td.join("TD-20260202-beta.md"), "# beta\n").unwrap();
        std::fs::write(
            docs.join("inconsistencies-tech-debt.md"),
            "| TD-20260201-alpha | a | A | low |\n\
             | TD-20260202-beta  | b | B | low |\n",
        )
        .unwrap();
        assert_eq!(count_tech_debt(&tmp.to_string_lossy()), 2);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn count_tech_debt_legacy_ai_folder() {
        // Path-agnostic: a project still on the pre-0.7.1 layout
        // (legacy `ai/` directory) should still get its TDs counted.
        let tmp = std::env::temp_dir().join("kronn-test-td-legacy");
        let _ = std::fs::remove_dir_all(&tmp);
        let docs = tmp.join("ai"); // legacy layout
        let td = docs.join("tech-debt");
        std::fs::create_dir_all(&td).unwrap();
        std::fs::write(td.join("TD-20260101-legacy.md"), "# legacy\n").unwrap();
        // Make it look like an audited project so detect_docs_dir returns
        // the `ai/` directory (otherwise it falls back to `docs/`).
        std::fs::write(docs.join("index.md"), "# legacy index\n").unwrap();
        assert_eq!(count_tech_debt(&tmp.to_string_lossy()), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─── scan_paths_with_depth: ignore list ────────────────────────────────────

    #[tokio::test]
    async fn scan_skips_ignored_directories() {
        let tmp = std::env::temp_dir().join("kronn-test-scan-ignore");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("Library/some-app/.git")).unwrap();
        std::fs::create_dir_all(tmp.join("real-project/.git")).unwrap();

        let ignore = vec!["Library".into()];
        let repos = scan_paths_with_depth(&[tmp.to_string_lossy().to_string()], &ignore, 3)
            .await
            .unwrap();

        // "Library" should be ignored, only "real-project" found
        let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"real-project"),
            "Expected real-project, got: {:?}",
            names
        );
        assert!(
            !names.iter().any(|n| n.contains("Library")),
            "Library should be ignored"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn scan_ignore_is_case_insensitive() {
        let tmp = std::env::temp_dir().join("kronn-test-scan-case");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("NODE_MODULES/foo/.git")).unwrap();
        std::fs::create_dir_all(tmp.join("my-repo/.git")).unwrap();

        let ignore = vec!["node_modules".into()];
        let repos = scan_paths_with_depth(&[tmp.to_string_lossy().to_string()], &ignore, 3)
            .await
            .unwrap();

        let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
        assert!(
            !names.iter().any(|n| n.contains("NODE_MODULES")),
            "NODE_MODULES should be ignored (case-insensitive)"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─── des dépôts hors du répertoire personnel ────────────────────

    /// A symlinked scan root reaches repositories outside the home directory.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_root_is_followed_to_the_repositories_it_points_at() {
        let real = tempfile::tempdir().unwrap();
        // /<réel>/git/agaches/site/.git — l'arborescence d'Arnaud.
        let repo = real.path().join("git/agaches/site");
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let home = tempfile::tempdir().unwrap();
        let link = home.path().join("workspace");
        std::os::unix::fs::symlink(real.path(), &link).unwrap();

        let repos = scan_paths_with_depth(&[link.to_string_lossy().into()], &[], 4)
            .await
            .unwrap();
        assert_eq!(
            repos.len(),
            1,
            "un lien vers la racine réelle doit mener aux dépôts: {repos:?}"
        );
        assert!(repos[0].path.contains("site"));
    }

    /// Suivre les liens veut dire se protéger des cycles. Un lien qui pointe sur
    /// un ancêtre ferait tourner la marche indéfiniment, ou rapporterait le même
    /// dépôt une fois par chemin qui y mène.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_loop_is_walked_once_and_does_not_hang() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("projet");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // projet/retour -> la racine : un cycle.
        std::os::unix::fs::symlink(root.path(), repo.join("retour")).unwrap();
        // raccourci -> projet : un second chemin vers le même dépôt.
        std::os::unix::fs::symlink(&repo, root.path().join("raccourci")).unwrap();

        let repos = scan_paths_with_depth(&[root.path().to_string_lossy().into()], &[], 6)
            .await
            .unwrap();
        assert_eq!(
            repos.len(),
            1,
            "un dépôt atteignable par deux routes reste un dépôt: {repos:?}"
        );
    }

    /// Local-only repositories sharing a directory name remain distinct.
    #[tokio::test]
    async fn two_local_repositories_sharing_a_name_are_two_repositories() {
        let root = tempfile::tempdir().unwrap();
        for org in ["agaches", "onepoint"] {
            std::fs::create_dir_all(root.path().join(format!("{org}/site/.git"))).unwrap();
        }

        let repos = scan_paths_with_depth(&[root.path().to_string_lossy().into()], &[], 4)
            .await
            .unwrap();
        assert_eq!(
            repos.len(),
            2,
            "deux organisations, deux `site`, deux dépôts: {repos:?}"
        );
        let mut paths: Vec<&str> = repos.iter().map(|r| r.path.as_str()).collect();
        paths.sort();
        assert!(paths[0].contains("agaches") && paths[1].contains("onepoint"));
    }

    /// Independent clones of the same remote remain distinct. Initialize real Git
    /// repositories so remote discovery exercises the intended collision.
    #[cfg(unix)]
    #[tokio::test]
    async fn two_clones_of_one_repository_are_two_working_copies() {
        const ORIGINE: &str = "git@example.com:acme/kronn.git";
        let root = tempfile::tempdir().unwrap();
        for copie in ["travail", "relecture"] {
            let repo = root.path().join(format!("{copie}/kronn"));
            std::fs::create_dir_all(&repo).unwrap();
            git_dans(&repo, &["init", "-q"]);
            // Même remote pour les deux : c'est bien le même dépôt d'origine.
            git_dans(&repo, &["remote", "add", "origin", ORIGINE]);
        }

        let repos = scan_paths_with_depth(&[root.path().to_string_lossy().into()], &[], 4)
            .await
            .unwrap();
        assert_eq!(
            repos.len(),
            2,
            "deux copies de travail du même dépôt restent deux: {repos:?}"
        );
        for r in &repos {
            assert_eq!(
                r.remote_url.as_deref(),
                Some(ORIGINE),
                "les deux entrées portent bien le remote commun: {repos:?}"
            );
        }
    }

    /// `git init` sans réseau ni configuration globale héritée.
    #[cfg(unix)]
    fn git_dans(repo: &std::path::Path, args: &[&str]) {
        let sortie = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git est disponible");
        assert!(
            sortie.status.success(),
            "git {args:?} dans {}: {}",
            repo.display(),
            String::from_utf8_lossy(&sortie.stderr)
        );
    }

    /// A directory revisited through a shorter path may expose deeper repositories.
    /// Deduplication must not prune that traversal, regardless of filesystem order.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_long_route_seen_first_does_not_close_the_short_one() {
        let cible = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cible.path().join("a/b/depot/.git")).unwrap();

        for (ordre, longue, courte) in [
            ("longue d'abord", "a-profond", "z-direct"),
            ("courte d'abord", "z-profond", "a-direct"),
        ] {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(root.path().join(format!("{longue}/n1"))).unwrap();
            std::os::unix::fs::symlink(cible.path(), root.path().join(format!("{longue}/n1/lien")))
                .unwrap();
            std::os::unix::fs::symlink(cible.path(), root.path().join(courte)).unwrap();

            // La route longue atteint `lien` à 3, donc `a` à 4 : le dépôt est
            // hors d'atteinte. La courte atteint le lien à 1, le dépôt à 4.
            let repos = scan_paths_with_depth(&[root.path().to_string_lossy().into()], &[], 4)
                .await
                .unwrap();
            assert_eq!(
                repos.len(),
                1,
                "{ordre}: le dépôt est trouvé, et une seule fois: {repos:?}"
            );
            assert!(repos[0].path.ends_with("a/b/depot"), "{ordre}: {repos:?}");
        }
    }

    /// À l'inverse, deux chemins d'accès vers le MÊME dossier ne font qu'une
    /// copie de travail, quel que soit le nom du lien.
    #[cfg(unix)]
    #[tokio::test]
    async fn two_names_for_one_directory_are_one_working_copy() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("reel");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::os::unix::fs::symlink(&repo, root.path().join("autre-nom")).unwrap();

        let repos = scan_paths_with_depth(&[root.path().to_string_lossy().into()], &[], 4)
            .await
            .unwrap();
        assert_eq!(
            repos.len(),
            1,
            "un seul dossier, atteint par deux noms: {repos:?}"
        );
    }

    /// Default scan depth reaches root/organisation/repository outside the home.
    #[tokio::test]
    async fn the_default_depth_reaches_a_repository_nested_under_an_organisation() {
        let root = tempfile::tempdir().unwrap();
        if let Some(maison) = directories::UserDirs::new() {
            assert!(
                !root.path().starts_with(maison.home_dir()),
                "le banc doit se tenir hors du répertoire personnel: {}",
                root.path().display()
            );
        }
        for org in ["agaches", "onepoint", "client1"] {
            std::fs::create_dir_all(root.path().join(format!("git/{org}/depot/.git"))).unwrap();
        }
        std::fs::create_dir_all(root.path().join("thinking/2e-cerveau/.git")).unwrap();

        let repos = scan_paths(&[root.path().to_string_lossy().into()], &[])
            .await
            .unwrap();
        assert_eq!(repos.len(), 4, "les quatre dépôts: {repos:?}");
        let mut chemins: Vec<&str> = repos.iter().map(|r| r.path.as_str()).collect();
        chemins.sort();
        assert!(
            chemins.iter().filter(|p| p.contains("/git/")).count() == 3
                && chemins.iter().any(|p| p.ends_with("2e-cerveau")),
            "trois dépôts sous git/<organisation>/, plus celui à côté: {chemins:?}"
        );
    }

    #[tokio::test]
    async fn scan_nonexistent_path_returns_empty() {
        let repos =
            scan_paths_with_depth(&["/nonexistent/path/that/does/not/exist".into()], &[], 3)
                .await
                .unwrap();
        assert!(repos.is_empty());
    }

    #[tokio::test]
    async fn scan_empty_paths_returns_empty() {
        let repos = scan_paths_with_depth(&[], &[], 3).await.unwrap();
        assert!(repos.is_empty());
    }

    // ─── default_config ignore list ────────────────────────────────────────────

    #[test]
    fn default_config_ignores_macos_dirs() {
        let config = crate::core::config::default_config();
        let ignore = &config.scan.ignore;
        assert!(
            ignore.contains(&"Library".to_string()),
            "Should ignore macOS Library"
        );
        assert!(
            ignore.contains(&".Trash".to_string()),
            "Should ignore macOS .Trash"
        );
        assert!(
            ignore.contains(&"node_modules".to_string()),
            "Should ignore node_modules"
        );
    }

    #[test]
    fn default_config_ignores_cache_dirs() {
        let config = crate::core::config::default_config();
        let ignore = &config.scan.ignore;
        assert!(
            ignore.contains(&".cache".to_string()),
            "Should ignore .cache"
        );
        assert!(ignore.contains(&".npm".to_string()), "Should ignore .npm");
        assert!(
            ignore.contains(&".cargo".to_string()),
            "Should ignore .cargo"
        );
    }

    // ─── count_ai_todos: Phase 2 markers ─────────────────────────────────────

    #[test]
    fn count_ai_todos_with_ask_user_markers() {
        let tmp = std::env::temp_dir().join("kronn-test-todos-ask-user");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(
            ai_dir.join("glossary.md"),
            "# Glossary\n| Widget | some entity <!-- TODO: ask user --> | |\n| Known | definition | |\n",
        ).unwrap();
        assert_eq!(count_ai_todos(&tmp.to_string_lossy()), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn count_ai_todos_in_tech_debt_subdir() {
        let tmp = std::env::temp_dir().join("kronn-test-todos-techdebt");
        let td_dir = tmp.join("ai/tech-debt");
        let _ = std::fs::create_dir_all(&td_dir);
        std::fs::write(
            td_dir.join("TD-20260313-old-php.md"),
            "# TD\n<!-- TODO: verify -->\nSome content\n",
        )
        .unwrap();
        std::fs::write(
            tmp.join("ai").join("index.md"),
            "# Project\nClean content\n",
        )
        .unwrap();
        assert_eq!(count_ai_todos(&tmp.to_string_lossy()), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─── needs_docs_migration ─────────────────────────────────────────────────

    #[test]
    fn needs_migration_when_legacy_ai_only() {
        let tmp = std::env::temp_dir().join("kronn-test-needs-mig-legacy");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("ai")).unwrap();
        std::fs::write(tmp.join("ai/index.md"), "# legacy\n").unwrap();
        assert!(needs_docs_migration(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn needs_migration_false_when_docs_agents_exists() {
        let tmp = std::env::temp_dir().join("kronn-test-needs-mig-migrated");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("docs")).unwrap();
        std::fs::write(tmp.join("docs/AGENTS.md"), "# agents\n").unwrap();
        // Even with a residual ai/index.md (e.g. operator chose symlink rétro-compat)
        // the migrated marker wins.
        std::fs::create_dir_all(tmp.join("ai")).unwrap();
        std::fs::write(tmp.join("ai/index.md"), "# old\n").unwrap();
        assert!(!needs_docs_migration(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn needs_migration_false_for_fresh_project() {
        let tmp = std::env::temp_dir().join("kronn-test-needs-mig-fresh");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // No ai/ and no docs/ — fresh, no banner.
        assert!(!needs_docs_migration(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn needs_migration_false_for_doc_agents_layout() {
        let tmp = std::env::temp_dir().join("kronn-test-needs-mig-doc-variant");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("doc")).unwrap();
        std::fs::write(tmp.join("doc/AGENTS.md"), "# agents\n").unwrap();
        std::fs::create_dir_all(tmp.join("ai")).unwrap();
        std::fs::write(tmp.join("ai/index.md"), "# old\n").unwrap();
        assert!(!needs_docs_migration(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─── detect_audit_status: ai/ dir exists but no index.md ─────────────────

    #[test]
    fn detect_audit_status_ai_dir_no_index() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-no-index");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        // ai/ dir exists but no index.md → TemplateInstalled
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(
            status,
            crate::models::AiAuditStatus::TemplateInstalled
        ));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[serial]
    fn docker_write_access_uses_the_explicit_rw_perimeter() {
        let root = tempfile::tempdir().unwrap();
        let writable = root.path().join("repos/writable");
        let outside = root.path().join("outside/readonly");
        std::fs::create_dir_all(&writable).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        std::env::set_var("KRONN_IN_DOCKER", "1");
        std::env::set_var("KRONN_REPOS_DIR", root.path().join("repos"));
        std::env::remove_var("KRONN_EXTRA_REPOS");
        std::env::remove_var("KRONN_HOST_HOME");

        let inside = diagnose_project_write_access(&writable.to_string_lossy());
        assert_eq!(
            inside.status,
            crate::models::ProjectWriteAccessStatus::Writable
        );
        let blocked = diagnose_project_write_access(&outside.to_string_lossy());
        assert_eq!(
            blocked.status,
            crate::models::ProjectWriteAccessStatus::ReadOnly
        );
        assert_eq!(blocked.reason.as_deref(), Some("outside_rw_perimeter"));

        std::env::remove_var("KRONN_IN_DOCKER");
        std::env::remove_var("KRONN_REPOS_DIR");
    }
}
