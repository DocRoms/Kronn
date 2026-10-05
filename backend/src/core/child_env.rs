//! The environment of every process Kronn starts on a caller's behalf: agent
//! CLIs (direct, adapters, native ACP), the project exec route, workflow Exec
//! steps, Quick Exec and the API-call credential CLIs (KT-1006, KT-1013).
//!
//! A child never inherits the backend's environment wholesale. It starts empty
//! (`env_clear`), receives the reviewed allow-list below from the backend's
//! environment, then the values its launch needs. [`seal`] runs last and drops
//! what no child may hold whatever added it: Kronn's admin token, the raw key,
//! another provider's key, and any secret-looking name nobody granted.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::agents::runner::{
    RoomAgentBridgeContext, TaskWorkerBridgeContext, WorkflowStepBridgeContext,
};

/// Variable the bridge reads its per-launch token from (layer B).
pub const BRIDGE_TOKEN_ENV: &str = "KRONN_BRIDGE_TOKEN";

/// Names no child process may receive, from any source.
pub const FORBIDDEN: &[&str] = &[
    // Kronn's admin bearer: an agent holding it is the operator.
    "KRONN_AUTH_TOKEN",
    // The raw encryption key (tier 1 of the key vault) and its legacy name.
    "KRONN_ENCRYPTION_KEK",
    "KRONN_KEK",
];

/// Provider credentials. A child receives one only when it is its own agent's.
pub const PROVIDER_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "MISTRAL_API_KEY",
    "OPENROUTER_API_KEY",
    "NVIDIA_API_KEY",
    "LITELLM_API_KEY",
    "LITELLM_MASTER_KEY",
];

/// GitHub variables. Never inherited: only `core::github_connection` adds
/// them, for a project connected to GitHub (design note §4.5, D2).
pub const GITHUB_ENV: &[&str] = &crate::core::github_connection::GITHUB_ENV_NAMES;

/// Inherited by every route. Reviewed list: each group says why a child needs it.
const BASE_NAMES: &[&str] = &[
    // Process essentials: binary lookup, identity, shell, terminal, time zone.
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "COLORTERM",
    "NO_COLOR",
    "TZ",
    "LANG",
    "LANGUAGE",
    // Temporary directories (an agent launch then points them at the project).
    "TMPDIR",
    "TEMP",
    "TMP",
    // Proxies, in both spellings tools read.
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    // TLS trust stores behind a corporate proxy.
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    // XDG base directories (CLI configs and caches live there on Linux).
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    // Git over SSH. The agent socket stays usable: residual risk, design note §2.
    "SSH_AUTH_SOCK",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    // Toolchains found through the environment rather than PATH alone.
    "CARGO_HOME",
    "CARGO_TARGET_DIR",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "GOPATH",
    "GOROOT",
    "JAVA_HOME",
    "NVM_DIR",
    "NVM_BIN",
    "PNPM_HOME",
    "COREPACK_HOME",
    "VOLTA_HOME",
    "PLAYWRIGHT_BROWSERS_PATH",
    "PYENV_ROOT",
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
    "HOMEBREW_PREFIX",
    // macOS: the encoding CoreFoundation programs expect.
    "__CF_USER_TEXT_ENCODING",
    // Windows: programs fail to start or resolve paths without these.
    "SystemRoot",
    "windir",
    "ComSpec",
    "PATHEXT",
    "USERPROFILE",
    "USERNAME",
    "USERDOMAIN",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "HOMEDRIVE",
    "HOMEPATH",
    "PUBLIC",
    "ALLUSERSPROFILE",
    "COMPUTERNAME",
    "OS",
    "PROCESSOR_ARCHITECTURE",
    "NUMBER_OF_PROCESSORS",
    // Windows: the user's own WSL forwarding list, extended per launch.
    "WSLENV",
];

/// Prefixes inherited by every route: locale, TLS, and npm's own settings (the
/// Docker image sets `NPM_CONFIG_PREFIX`; a token-like npm key is still sealed).
const BASE_PREFIXES: &[&str] = &["LC_", "SSL_CERT_", "NPM_CONFIG_", "npm_config_"];

/// Git's own settings a user may export: identity, SSH, config location,
/// prompts. Repository selectors (`GIT_DIR`, `GIT_WORK_TREE`…) are never
/// inherited; Kronn sets them explicitly where it needs them.
const GIT_NAMES: &[&str] = &[
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_TERMINAL_PROMPT",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
];

/// `gh` and `glab` configuration: where their own login lives and which host
/// they talk to. Their tokens are never inherited (a connected project's comes
/// from `core::github_connection`).
const GIT_HOST_NAMES: &[&str] = &[
    "GH_CONFIG_DIR",
    "GH_HOST",
    "GH_PROMPT_DISABLED",
    "GH_NO_UPDATE_NOTIFIER",
    "GLAB_CONFIG_DIR",
    "GITLAB_HOST",
    "GL_HOST",
];

/// Package managers' own locations: registries, caches, toolchain roots. No
/// registry token: a repository's config file could print it.
const DEPENDENCY_CHECK_NAMES: &[&str] = &[
    "GOPROXY",
    "GOPRIVATE",
    "GONOPROXY",
    "GONOSUMDB",
    "GOSUMDB",
    "GOINSECURE",
    "GOFLAGS",
    "GOMODCACHE",
    "GOCACHE",
    "GOENV",
    "GOTOOLCHAIN",
    "GEM_HOME",
    "GEM_PATH",
    "BUNDLE_USER_HOME",
    "BUNDLE_USER_CONFIG",
    "BUNDLE_APP_CONFIG",
    "BUNDLE_PATH",
    "RBENV_ROOT",
    "RBENV_VERSION",
    "DOTNET_ROOT",
    "DOTNET_CLI_HOME",
    "NUGET_PACKAGES",
    "POETRY_HOME",
    "POETRY_CONFIG_DIR",
    "POETRY_CACHE_DIR",
    "COMPOSER_HOME",
    "COMPOSER_CACHE_DIR",
];

/// Docker's and Compose's own settings (daemon, context, TLS paths, project).
const DOCKER_PREFIXES: &[&str] = &["DOCKER_", "COMPOSE_"];

/// Cloud CLIs an API connection may use to mint its credential (`az`, `gcloud`).
const CREDENTIAL_CLI_PREFIXES: &[&str] = &["AZURE_", "CLOUDSDK_", "AWS_"];
const CREDENTIAL_CLI_NAMES: &[&str] = &["GOOGLE_APPLICATION_CREDENTIALS"];

/// Which agent a launch starts, for its own documented variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentFamily {
    Claude,
    Codex,
    Gemini,
    Copilot,
    Kiro,
    Vibe,
    OpenCode,
    Ollama,
    /// A program Kronn does not know: base allow-list only.
    Other,
}

impl AgentFamily {
    /// From what `try_spawn` receives: the binary, its npx package, and the
    /// provider variable the command builder chose (Vibe runs as `python3`).
    pub fn from_launch(binary: &str, npx_package: Option<&str>, env_key: &str) -> Self {
        let name = Path::new(binary)
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or(binary);
        match (name, npx_package, env_key) {
            (_, Some("@anthropic-ai/claude-code"), _) | ("claude", _, _) => Self::Claude,
            (_, Some("@openai/codex"), _) | ("codex", _, _) => Self::Codex,
            (_, Some("@google/gemini-cli"), _) | ("gemini", _, _) => Self::Gemini,
            (_, Some("@github/copilot"), _) | ("copilot", _, _) => Self::Copilot,
            (_, Some("opencode-ai"), _) | ("opencode", _, _) => Self::OpenCode,
            ("kiro-cli", _, _) => Self::Kiro,
            ("vibe", _, _) | (_, _, "MISTRAL_API_KEY") => Self::Vibe,
            ("ollama", _, _) => Self::Ollama,
            _ => Self::Other,
        }
    }

    pub fn from_agent_type(agent: &crate::models::AgentType) -> Self {
        use crate::models::AgentType;
        match agent {
            AgentType::ClaudeCode => Self::Claude,
            AgentType::Codex => Self::Codex,
            AgentType::GeminiCli => Self::Gemini,
            AgentType::CopilotCli => Self::Copilot,
            AgentType::Kiro => Self::Kiro,
            AgentType::Vibe => Self::Vibe,
            AgentType::OpenCode => Self::OpenCode,
            AgentType::Ollama => Self::Ollama,
            _ => Self::Other,
        }
    }

    /// The variable holding this agent's provider key, as `get_api_key` reads it.
    pub fn provider_key_env(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("ANTHROPIC_API_KEY"),
            Self::Codex => Some("OPENAI_API_KEY"),
            Self::Gemini => Some("GEMINI_API_KEY"),
            Self::Vibe => Some("MISTRAL_API_KEY"),
            Self::Copilot => Some("GH_TOKEN"),
            _ => None,
        }
    }

    /// Each agent's own documented variables (vendor docs), exact names.
    fn own_names(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[
                "MAX_THINKING_TOKENS",
                "MAX_MCP_OUTPUT_TOKENS",
                "MCP_TIMEOUT",
                "MCP_TOOL_TIMEOUT",
                "BASH_DEFAULT_TIMEOUT_MS",
                "BASH_MAX_TIMEOUT_MS",
                "BASH_MAX_OUTPUT_LENGTH",
                "USE_BUILTIN_RIPGREP",
                "CLOUD_ML_REGION",
                "GOOGLE_APPLICATION_CREDENTIALS",
            ],
            Self::Copilot => &["GH_HOST"],
            Self::Gemini => &["GOOGLE_API_KEY"],
            _ => &[],
        }
    }

    /// Each agent's own documented variable families.
    fn own_prefixes(self) -> &'static [&'static str] {
        match self {
            // Claude Code: its settings, Anthropic API, Bedrock (AWS) and Vertex.
            Self::Claude => &[
                "CLAUDE_",
                "ANTHROPIC_",
                "DISABLE_",
                "VERTEX_REGION_",
                "AWS_",
            ],
            Self::Codex => &["CODEX_", "OPENAI_"],
            Self::Gemini => &["GEMINI_", "GOOGLE_CLOUD_", "GOOGLE_GENAI_"],
            Self::Copilot => &["COPILOT_"],
            Self::Kiro => &["KIRO_", "AWS_"],
            Self::Vibe => &["MISTRAL_", "VIBE_"],
            Self::OpenCode => &["OPENCODE_"],
            Self::Ollama => &["OLLAMA_"],
            Self::Other => &[],
        }
    }

    fn owns(self, name: &str) -> bool {
        name_in(name, self.own_names()) || has_prefix(name, self.own_prefixes())
    }
}

/// The kind of process being started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildRoute {
    /// An agent CLI, on any of the three routes.
    Agent(AgentFamily),
    /// `POST /api/projects/{id}/exec` and `/api/discussions/{id}/exec`.
    ProjectExec,
    /// A workflow Exec step, its setup command included.
    WorkflowExec,
    /// A Quick Exec collector.
    QuickExec,
    /// A credential CLI an API connection declares (`az`, `gcloud`).
    CredentialCli,
    /// Every `git` process Kronn starts: a repository's hooks, filters and
    /// drivers run inside it, and agents can write repositories.
    Git,
    /// `gh` and `glab`: they start git in the repository themselves.
    GitHost,
    /// A dependency check: package managers read the repository's config.
    DependencyCheck,
    /// `docker` and `docker compose`: Compose interpolates the environment
    /// into a repository's compose file.
    Docker,
    /// Any other program Kronn starts for itself (installers, system probes).
    Tool,
}

impl ChildRoute {
    /// Credentials this route's program legitimately reads: its agent's own
    /// variables, or a credential CLI's cloud configuration.
    fn owns(self, name: &str) -> bool {
        match self {
            Self::Agent(family) => family.owns(name),
            Self::CredentialCli => {
                name_in(name, CREDENTIAL_CLI_NAMES) || has_prefix(name, CREDENTIAL_CLI_PREFIXES)
            }
            Self::Git => name_in(name, GIT_NAMES),
            Self::GitHost => name_in(name, GIT_NAMES) || name_in(name, GIT_HOST_NAMES),
            Self::DependencyCheck => name_in(name, DEPENDENCY_CHECK_NAMES),
            // A prefix grants a family, never a credential inside it.
            Self::Docker => has_prefix(name, DOCKER_PREFIXES) && !looks_secret(name),
            Self::ProjectExec | Self::WorkflowExec | Self::QuickExec | Self::Tool => false,
        }
    }

    /// Whether this route inherits `name` from the backend's environment.
    pub fn inherits(self, name: &str) -> bool {
        if name_in(name, FORBIDDEN) || name_in(name, GITHUB_ENV) {
            return false;
        }
        if is_base(name) {
            return true;
        }
        self.owns(name)
    }
}

/// Whether `name` is on the allow-list every route inherits.
pub fn is_base(name: &str) -> bool {
    name_in(name, BASE_NAMES) || has_prefix(name, BASE_PREFIXES)
}

/// The names a command currently carries (after [`reset`]: what it inherited).
pub fn names_of(command: &std::process::Command) -> std::collections::HashSet<String> {
    command
        .get_envs()
        .filter(|(_, value)| value.is_some())
        .filter_map(|(name, _)| name.to_str().map(str::to_owned))
        .collect()
}

/// Names a launch sets itself although its route may also inherit them; a
/// launch crossing into WSL forwards these (they are per-launch values).
pub const LAUNCH_OVERRIDES: &[&str] = &[
    "TMPDIR",
    "TEMP",
    "TMP",
    "CLAUDE_CODE_BUBBLEWRAP",
    "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
];

/// Whether a launch crossing into WSL forwards `name`: everything the launch
/// set, never what it merely inherited from the Windows side (`inherited`, the
/// names right after [`reset`]), as before the environment was built.
pub fn forwarded_into_wsl(
    name: &str,
    inherited: &std::collections::HashSet<String>,
    launch_set: &[&str],
) -> bool {
    !inherited.contains(name) || name_in(name, LAUNCH_OVERRIDES) || name_in(name, launch_set)
}

/// Names that look like a credential. Inherited or set, such a name reaches a
/// child only when its agent owns it or the caller granted it.
pub fn looks_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "API_KEY",
        "APIKEY",
        "PRIVATE",
        "CREDENTIAL",
        "_KEY",
        "KEK",
    ]
    .iter()
    .any(|needle| upper.contains(needle))
}

fn name_in(name: &str, list: &[&str]) -> bool {
    if cfg!(windows) {
        list.iter().any(|entry| entry.eq_ignore_ascii_case(name))
    } else {
        list.contains(&name)
    }
}

fn has_prefix(name: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| {
        if cfg!(windows) {
            name.len() >= prefix.len()
                && name.is_char_boundary(prefix.len())
                && name[..prefix.len()].eq_ignore_ascii_case(prefix)
        } else {
            name.starts_with(prefix)
        }
    })
}

/// The subset of `parent` a route inherits.
pub fn inherited_from(
    route: ChildRoute,
    parent: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    parent
        .into_iter()
        .filter(|(name, _)| name.to_str().is_some_and(|name| route.inherits(name)))
        .collect()
}

#[cfg(test)]
thread_local! {
    static PARENT_OVERRIDE: std::cell::RefCell<Option<Vec<(OsString, OsString)>>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `body` as if the backend's environment were `parent` (this thread only).
#[cfg(test)]
pub(crate) fn with_parent_env<T>(parent: &[(&str, &str)], body: impl FnOnce() -> T) -> T {
    let parent = parent
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect();
    PARENT_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(parent));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            PARENT_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
        }
    }
    let _reset = Reset;
    body()
}

fn parent_env() -> Vec<(OsString, OsString)> {
    #[cfg(test)]
    if let Some(parent) = PARENT_OVERRIDE.with(|cell| cell.borrow().clone()) {
        return parent;
    }
    std::env::vars_os().collect()
}

/// One variable of the backend's environment, as the builder sees it.
pub fn parent_var(name: &str) -> Option<OsString> {
    parent_env()
        .into_iter()
        .find_map(|(key, value)| (key == name).then_some(value))
}

/// Empty the child's environment and give it what its route inherits. Call
/// before setting any per-launch value.
pub fn reset(command: &mut std::process::Command, route: ChildRoute) {
    command.env_clear();
    command.envs(inherited_from(route, parent_env()));
}

/// Drop what no child may hold, whatever set it: the forbidden names, other
/// agents' provider keys, and secret-looking names neither owned by the route's
/// agent nor in `granted`. Names in `granted` ending with `*` are prefixes.
pub fn seal(command: &mut std::process::Command, route: ChildRoute, granted: &[&str]) {
    let is_granted = |name: &str| {
        granted.iter().any(|grant| match grant.strip_suffix('*') {
            Some(prefix) => has_prefix(name, &[prefix]),
            None => name_in(name, &[grant]),
        })
    };
    let refused: Vec<OsString> = command
        .get_envs()
        .filter_map(|(name, value)| value.map(|_| name.to_os_string()))
        .filter(|name| {
            let Some(name) = name.to_str() else {
                return true;
            };
            if name_in(name, FORBIDDEN) {
                return true;
            }
            let owned = route.owns(name);
            if name_in(name, PROVIDER_KEYS) {
                return !owned && !is_granted(name);
            }
            looks_secret(name) && !owned && !is_granted(name)
        })
        .collect();
    for name in refused {
        command.env_remove(name);
    }
}

/// The values one agent launch adds on top of its inherited environment.
#[derive(Default, Clone, Copy)]
pub struct AgentLaunch<'a> {
    pub discussion_id: Option<&'a str>,
    pub task_worker: Option<&'a TaskWorkerBridgeContext>,
    pub room_agent: Option<&'a RoomAgentBridgeContext>,
    pub workflow_step: Option<&'a WorkflowStepBridgeContext>,
    /// The scoped bridge token minted for this launch (layer B).
    pub bridge_token: Option<&'a str>,
    /// This agent's provider key: `(variable, value)`.
    pub api_key: Option<(&'a str, &'a str)>,
}

impl AgentLaunch<'_> {
    /// Names a sealed agent launch keeps although they look like secrets.
    pub fn granted(&self) -> Vec<&str> {
        let mut granted = vec![BRIDGE_TOKEN_ENV, "KRONN_MCP_*"];
        if let Some((name, _)) = self.api_key {
            granted.push(name);
        }
        granted
    }
}

/// Apply one agent launch's own values: temporary directory beside the
/// project, room and workflow contexts, backend URL, bridge token and key.
/// `backend_url` is resolved by the caller (WSL-aware on the direct route).
pub fn apply_agent_launch(
    command: &mut std::process::Command,
    work_dir: &Path,
    launch: &AgentLaunch<'_>,
    backend_url: Option<String>,
) -> Result<(), String> {
    // Same filesystem as the project, so an agent's rename from a temp file
    // into the work tree never crosses devices (EXDEV under Docker VirtioFS).
    let agent_tmpdir = work_dir.join(".kronn/tmp");
    let _ = std::fs::create_dir_all(&agent_tmpdir);
    if let Some(project_path) = work_dir.to_str() {
        crate::core::mcp_scanner::ensure_gitignore_public(project_path, ".kronn/tmp/");
    }
    command.env("TMPDIR", &agent_tmpdir);
    command.env("TEMP", &agent_tmpdir);
    command.env("TMP", &agent_tmpdir);

    if let Some(discussion_id) = launch.discussion_id {
        command.env("KRONN_DISCUSSION_ID", discussion_id);
    }
    if let Some(url) = backend_url {
        command.env("KRONN_BACKEND_URL", url);
    }
    if let Some(context) = launch.task_worker {
        let encoded = serde_json::to_string(context)
            .map_err(|error| format!("Unable to encode task worker context: {error}"))?;
        command.env("KRONN_TASK_WORKER_CONTEXT", encoded);
    }
    // A worker's capability excludes the room and step identities.
    if launch.task_worker.is_none() {
        if let Some(context) = launch.room_agent {
            let encoded = serde_json::to_string(context)
                .map_err(|error| format!("Unable to encode room agent context: {error}"))?;
            command.env("KRONN_ROOM_AGENT_CONTEXT", encoded);
        }
        if let Some(context) = launch.workflow_step {
            let encoded = serde_json::to_string(context)
                .map_err(|error| format!("Unable to encode workflow step context: {error}"))?;
            command.env("KRONN_WORKFLOW_STEP_CONTEXT", encoded);
        }
    }
    if let Some(token) = launch.bridge_token {
        command.env(BRIDGE_TOKEN_ENV, token);
    }
    if let Some((name, value)) = launch.api_key {
        command.env(name, value);
    }
    Ok(())
}

/// A command that may push or call `gh` (Exec step, its setup, a workspace
/// hook): the built environment plus `github_env`, the project's GitHub
/// variables from `core::github_connection::env_for_launch` (empty when the
/// project is not connected or there is no project).
pub fn isolate_with_github(
    command: &mut std::process::Command,
    route: ChildRoute,
    github_env: &[(String, String)],
) {
    reset(command, route);
    crate::core::github_connection::apply_launch_env(command, github_env);
    seal(command, route, GITHUB_ENV);
}

/// [`isolate_with_github`] plus per-launch values the step itself sets, such
/// as where an approved-script step's worktree and copy are (KT-918).
pub fn isolate_with_github_and_values(
    command: &mut std::process::Command,
    route: ChildRoute,
    github_env: &[(String, String)],
    values: &[(&str, &OsStr)],
) {
    reset(command, route);
    crate::core::github_connection::apply_launch_env(command, github_env);
    for (name, value) in values {
        command.env(name, value);
    }
    seal(command, route, GITHUB_ENV);
}

/// Reset + seal for a child that adds nothing of its own (exec routes, Quick
/// Exec, credential CLIs).
pub fn isolate(command: &mut std::process::Command, route: ChildRoute) {
    reset(command, route);
    seal(command, route, &[]);
}

/// Test helpers shared by the spawn sites' environment tests.
#[cfg(test)]
pub(crate) mod probe {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// Never inherited by any route: what a leak test plants in the parent.
    pub const SECRET_NAMES: &[&str] = &[
        "KRONN_AUTH_TOKEN",
        "KRONN_ENCRYPTION_KEK",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "KRONN_SPAWN_SENTINEL_API_KEY",
        "GH_TOKEN",
        "GITLAB_TOKEN",
        "DOCKER_AUTH_TOKEN",
    ];

    /// Also put the sentinel in the real process environment, so a command
    /// that skips the builder and inherits it fails the dump assertions.
    pub fn plant_real_sentinel() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| std::env::set_var("KRONN_SPAWN_SENTINEL_API_KEY", "sentinel-real"));
    }

    /// A backend environment: `path` plus every name of [`SECRET_NAMES`].
    pub fn parent_with_secrets(path: &str, home: &str) -> Vec<(String, String)> {
        let mut parent = vec![
            ("PATH".to_string(), path.to_string()),
            ("HOME".to_string(), home.to_string()),
        ];
        parent.extend(
            SECRET_NAMES
                .iter()
                .map(|name| (name.to_string(), format!("sentinel-{name}"))),
        );
        parent
    }

    /// Run `body` with [`parent_with_secrets`] as the backend's environment.
    pub fn with_secret_parent<T>(path: &str, home: &str, body: impl FnOnce() -> T) -> T {
        let parent = parent_with_secrets(path, home);
        let borrowed: Vec<(&str, &str)> = parent
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        super::with_parent_env(&borrowed, body)
    }

    /// What a command will hand its child, by name.
    pub fn env_of(command: &std::process::Command) -> BTreeMap<String, String> {
        command
            .get_envs()
            .filter_map(|(name, value)| {
                value.map(|value| {
                    (
                        name.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect()
    }

    /// The command was built (its PATH comes from the builder) and carries
    /// none of [`SECRET_NAMES`] except those in `allowed`.
    pub fn assert_built_without_secrets(
        command: &std::process::Command,
        expected_path: &str,
        allowed: &[&str],
    ) {
        let env = env_of(command);
        assert_eq!(
            env.get("PATH").map(String::as_str),
            Some(expected_path),
            "the environment was not built: {env:?}"
        );
        for name in SECRET_NAMES {
            if !allowed.contains(name) {
                assert!(!env.contains_key(*name), "{name} reached the child");
            }
        }
    }

    /// A fake `name` in `dir` that writes its environment to the returned file.
    #[cfg(unix)]
    pub fn env_dumping_program(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let out = dir.join(format!("{name}.env"));
        let script = dir.join(name);
        std::fs::write(
            &script,
            format!("#!/bin/sh\n/usr/bin/env > '{}'\n", out.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        out
    }

    /// The environment a fake program recorded, by name.
    pub fn read_dump(out: &Path) -> BTreeMap<String, String> {
        std::fs::read_to_string(out)
            .unwrap_or_else(|error| panic!("{} was not written: {error}", out.display()))
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    /// No name of [`SECRET_NAMES`] (except `allowed`) in a recorded environment.
    pub fn assert_dump_without_secrets(env: &BTreeMap<String, String>, allowed: &[&str]) {
        for name in SECRET_NAMES {
            if !allowed.contains(name) {
                assert!(
                    !env.contains_key(*name),
                    "{name} reached the child: {env:?}"
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "child_env_test.rs"]
mod child_env_test;
