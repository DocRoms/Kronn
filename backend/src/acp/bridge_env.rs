//! The environment Kronn's bridge needs when its ACP runtime does not pass its
//! own environment to the MCP servers it starts (KT-1082).
//!
//! Vibe starts stdio servers with the MCP SDK's default environment (`HOME`,
//! `PATH`...) plus the `env` a client declares in `session/new`, and persists
//! that declaration in its session metadata. So the plain values travel in the
//! declaration, and the secrets in an owner-only file the declaration names,
//! removed when the session's transport is dropped.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::AcpAgent;

/// The variable naming the secrets file; the bridge loads it at start-up.
pub const SECRET_FILE_ENV: &str = "KRONN_BRIDGE_SECRET_ENV_FILE";

/// Values the bridge reads that hold no secret.
const PLAIN: &[&str] = &[
    "KRONN_DISCUSSION_ID",
    "KRONN_BACKEND_URL",
    "KRONN_ROOM_AGENT_CONTEXT",
    "KRONN_TASK_WORKER_CONTEXT",
];

/// Values the bridge reads that are secrets: the launch's bridge token and a
/// workflow step's capability.
const SECRET: &[&str] = &[
    crate::core::child_env::BRIDGE_TOKEN_ENV,
    "KRONN_WORKFLOW_STEP_CONTEXT",
];

/// Runtimes whose stdio MCP servers do not inherit the runtime's environment.
fn needs_declared_env(agent: AcpAgent) -> bool {
    agent == AcpAgent::Vibe
}

/// An owner-only directory holding one launch's secrets file.
pub struct SecretEnvFile {
    dir: PathBuf,
    path: PathBuf,
}

impl SecretEnvFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write(values: &serde_json::Map<String, Value>) -> Result<Self, String> {
        let dir =
            std::env::temp_dir().join(format!("kronn-bridge-{}", uuid::Uuid::new_v4().simple()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        // Not recursive: an existing path (or a planted link) is refused.
        builder
            .create(&dir)
            .map_err(|error| format!("create bridge secrets directory: {error}"))?;
        let file = Self {
            path: dir.join("env.json"),
            dir,
        };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut handle = options
            .open(&file.path)
            .map_err(|error| format!("create bridge secrets file: {error}"))?;
        let bytes = serde_json::to_vec(values)
            .map_err(|error| format!("encode bridge secrets: {error}"))?;
        std::io::Write::write_all(&mut handle, &bytes)
            .map_err(|error| format!("write bridge secrets file: {error}"))?;
        Ok(file)
    }
}

impl Drop for SecretEnvFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// What a session declares as its bridge's `env`, and the file it names.
#[derive(Default)]
pub struct BridgeEnv {
    /// ACP `EnvVariable` entries: `{"name", "value"}`, never a secret value.
    pub declared: Vec<Value>,
    /// Held only so the file is removed when the session's transport drops.
    pub _secret_file: Option<SecretEnvFile>,
}

/// The bridge environment for `agent`, read from the command Kronn built, so
/// the bridge receives exactly what the runtime itself was given.
pub fn for_launch(agent: AcpAgent, command: &Command) -> Result<BridgeEnv, String> {
    if !needs_declared_env(agent) {
        return Ok(BridgeEnv::default());
    }
    let value_of = |name: &str| {
        command.get_envs().find_map(|(key, value)| {
            (key == name)
                .then(|| value.and_then(|value| value.to_str()))
                .flatten()
                .map(str::to_owned)
        })
    };
    let mut declared: Vec<Value> = PLAIN
        .iter()
        .filter_map(|name| value_of(name).map(|value| json!({"name": name, "value": value})))
        .collect();
    let secrets: serde_json::Map<String, Value> = SECRET
        .iter()
        .filter_map(|name| value_of(name).map(|value| ((*name).to_owned(), Value::String(value))))
        .collect();
    let secret_file = if secrets.is_empty() {
        None
    } else {
        let file = SecretEnvFile::write(&secrets)?;
        declared.push(json!({
            "name": SECRET_FILE_ENV,
            "value": file.path().to_string_lossy(),
        }));
        Some(file)
    };
    Ok(BridgeEnv {
        declared,
        _secret_file: secret_file,
    })
}
