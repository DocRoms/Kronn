//! Run explicitly on an authenticated host with OS sandbox privileges.
#![cfg(unix)]

use kronn::agents::runner::{start_agent_with_config, AgentStartConfig};
use kronn::models::AgentType;

#[tokio::test]
#[ignore = "Requires authenticated Claude Code and Codex CLIs and OS sandbox privileges"]
async fn workflow_read_only_repos_live_probe() {
    for agent in [AgentType::ClaudeCode, AgentType::Codex] {
        // Keep the external checkout under the repository's home/mount path,
        // where Claude's read block applies, rather than an exempt OS temp dir.
        let probe_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.kronn/tmp");
        std::fs::create_dir_all(&probe_root).unwrap();
        let probe_root = probe_root.canonicalize().unwrap();
        let temp = tempfile::tempdir_in(probe_root).unwrap();
        let work = temp.path().join("work");
        let repo = temp.path().join("linked repo");
        std::fs::create_dir(&work).unwrap();
        std::fs::create_dir(&repo).unwrap();
        let markers: Vec<_> = (0..4).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        std::fs::write(repo.join("README.md"), &markers[0]).unwrap();
        std::fs::write(repo.join("head.txt"), &markers[1]).unwrap();
        std::fs::write(repo.join("grep.txt"), format!("probe={}", markers[2])).unwrap();
        std::fs::write(repo.join("protected.txt"), "original").unwrap();
        for args in [
            vec!["init"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Probe",
                "-c",
                "user.email=probe@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                &markers[3],
            ],
        ] {
            let output = kronn::core::cmd::git_cmd()
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::fs::write(
            work.join("probe_shell.py"),
            r#"import errno, json, pathlib, subprocess, sys
repo = pathlib.Path(sys.argv[1])
results = {}
for name, command in {
    'git': ['git', '-C', str(repo), 'log', '-1', '--format=%s'],
    'head': ['head', '-1', str(repo / 'head.txt')],
    'grep': ['grep', 'probe=', str(repo / 'grep.txt')],
}.items():
    run = subprocess.run(command, capture_output=True, text=True)
    results[name] = {'code': run.returncode, 'stdout': run.stdout, 'stderr': run.stderr}
try:
    (repo / 'protected.txt').write_text('forbidden')
    results['write_denied'] = False
except OSError as error:
    if error.errno not in (errno.EACCES, errno.EPERM, errno.EROFS):
        raise
    results['write_denied'] = True
    results['denial'] = str(error)
pathlib.Path('writable.txt').write_text('WORKSPACE_WRITE_OK')
pathlib.Path('shell-evidence.json').write_text(json.dumps(results))
print(json.dumps(results))
"#,
        )
        .unwrap();
        let prompt = format!(
            "Verify repository access using real tools. The linked repository is {}. \
             First read README.md using Read (or the Codex file-read equivalent). \
             Then run python3 probe_shell.py <repo> in the shell. This fixture executes \
             git -C, head and grep and attempts a write, recording the OS result. Copy the four \
             values you actually read into your final answer. Then attempt to \
             change <repo>/protected.txt using Edit or Write, and attempt the same \
             using a shell redirection. Both must be denied: do not seek approval \
             or bypass the sandbox. Report each denial. Finally use the shell to \
             create writable.txt in your working directory containing WORKSPACE_WRITE_OK. \
             The fixture exists only for this test; attempts to write protected.txt \
             are explicitly authorized to prove that the sandbox rejects them.",
            serde_json::to_string(&repo).unwrap()
        );
        let repos = vec![repo.to_str().unwrap().to_string()];
        let config = kronn::core::config::default_config();
        let mut process = start_agent_with_config(AgentStartConfig {
            work_dir: Some(work.to_str().unwrap()),
            read_only_repos: &repos,
            full_access: true,
            mcp_context_override: Some(""),
            ..AgentStartConfig::new(&agent, work.to_str().unwrap(), &prompt, &config.tokens)
        })
        .await
        .unwrap();
        let separator = if process.raw_token_stream() { "" } else { "\n" };
        let observed = tokio::time::timeout(std::time::Duration::from_secs(180), async {
            let mut lines = Vec::new();
            while let Some(line) = process.next_line().await {
                lines.push(line);
            }
            let status = process.child.wait().await.unwrap();
            (status, lines.join(separator))
        })
        .await
        .expect("live probe timed out");
        let stderr = process.captured_stderr_flushed().await;
        assert!(
            observed.0.success(),
            "{agent:?}: {:?}\n{}",
            stderr,
            observed.1
        );
        for marker in &markers {
            assert!(
                observed.1.contains(marker.as_str()),
                "{agent:?}: external read missing: {}",
                observed.1
            );
        }
        let evidence: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(work.join("shell-evidence.json")).unwrap(),
        )
        .unwrap();
        for (operation, marker) in [
            ("git", &markers[3]),
            ("head", &markers[1]),
            ("grep", &markers[2]),
        ] {
            assert_eq!(evidence[operation]["code"], 0, "{agent:?}: {evidence}");
            assert!(evidence[operation]["stdout"]
                .as_str()
                .unwrap()
                .contains(marker));
        }
        assert_eq!(evidence["write_denied"], true, "{agent:?}: {evidence}");
        assert_eq!(
            std::fs::read_to_string(repo.join("protected.txt")).unwrap(),
            "original",
            "{agent:?} wrote outside its worktree"
        );
        assert_eq!(
            std::fs::read_to_string(work.join("writable.txt"))
                .unwrap()
                .trim(),
            "WORKSPACE_WRITE_OK"
        );
        println!("{agent:?}: Read, Git history, head and grep succeeded; linked file unchanged; worktree writable. Review denial receipts:\n{}", observed.1);
    }
}
