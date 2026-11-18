
/// Compose interpolates its environment into the repository's compose
/// file: it gets Docker's settings, never a backend secret.
#[cfg(unix)]
// dumps the child environment with probe::env_dumping_program, a #!/bin/sh script made executable through unix mode bits
#[tokio::test]
async fn docker_compose_runs_without_the_backend_environment() {
    use crate::core::child_env::probe;
    probe::plant_real_sentinel();
    let bin = tempfile::tempdir().unwrap();
    let out = probe::env_dumping_program(bin.path(), "docker");
    let path = format!("{}:/usr/bin:/bin", bin.path().display());
    let mut parent = probe::parent_with_secrets(&path, "/home/u");
    parent.push(("DOCKER_HOST".into(), "unix:///run/docker.sock".into()));
    let borrowed: Vec<(&str, &str)> = parent
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let (mut compose, running) = crate::core::child_env::with_parent_env(&borrowed, || {
        (
            super::docker_command(bin.path(), "compose.yml", &["ps"]),
            super::docker_cli(),
        )
    });
    probe::assert_built_without_secrets(running.as_std(), &path, &[]);
    assert!(compose.status().await.unwrap().success());
    let recorded = probe::read_dump(&out);
    probe::assert_dump_without_secrets(&recorded, &[]);
    assert_eq!(
        recorded.get("DOCKER_HOST").map(String::as_str),
        Some("unix:///run/docker.sock")
    );
}

use super::*;
