// Setup & Configuration — top-level config tree (`AppConfig`),
// per-section configs (server, tokens, scan, agents, model tiers),
// the API-key types, and the Setup Wizard's status types.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ─── App config ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub tokens: TokensConfig,
    pub scan: ScanConfig,
    pub agents: AgentsConfig,
    /// Local ComfyUI media target (KT-1381).
    #[serde(default, skip_serializing_if = "crate::models::comfyui::is_unset")]
    pub comfyui: crate::models::ComfyUiConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media_reference_limits: Vec<crate::models::MediaReferenceLimit>,
    /// Output language used by agents when they write their replies.
    /// Separate from `ui_language` below which controls the Kronn UI locale.
    #[serde(default = "default_language")]
    pub language: String,
    /// UI language (FR/EN/ES/ZH) for the React frontend. Persisted here so a
    /// Tauri WebView2 localStorage wipe doesn't reset the user's choice
    /// every time the app updates or Windows rotates the WebView2 profile.
    /// Frontend still writes to localStorage as a fast-path + fallback when
    /// the backend is unreachable.
    #[serde(default = "default_ui_language")]
    pub ui_language: String,
    /// Persistent STT model choice (e.g. "onnx-community/whisper-tiny").
    /// None = first-launch default / user never set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_model: Option<String>,
    /// Persistent TTS voice choices, keyed by output language code
    /// ("fr" → "voice-id-fr", "en" → "voice-id-en", …).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    #[ts(type = "Record<string, string>")]
    pub tts_voices: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub disabled_agents: Vec<AgentType>,
    /// The key the running process uses. Never serialized: the only copy
    /// written to `config.toml` is the one `config::retain_disk_key` keeps
    /// while no vault holds the key (KT-1007).
    #[serde(default, skip_serializing)]
    #[ts(skip)]
    pub encryption_secret: Option<String>,
    /// Secret theme unlock codes (theme_name → code). Read-only from the
    /// server — users populate this table in their local
    /// `~/.config/kronn/config.toml` to enable hidden themes for testers.
    /// The values are NEVER exported to TypeScript and NEVER returned by
    /// any endpoint — only consumed during POST /api/themes/unlock. The
    /// public bundle therefore cannot leak them to a curious user.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    #[ts(skip)]
    pub secret_themes: std::collections::HashMap<String, String>,
    /// Profile IDs the operator has unlocked via a secret code. Secret
    /// builtins (e.g. "batman") are filtered out of `GET /api/profiles`
    /// when their id is not listed here — unlock adds the id and
    /// persists the config so the profile sticks across restarts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub unlocked_profiles: Vec<String>,
    /// Skill IDs for which the frontend must NOT auto-activate even when
    /// the user's message matches the skill's `auto_triggers` regexes.
    /// Read by the frontend's `detectTriggeredSkills` filter and by the
    /// Settings UI toggle. Empty by default — every skill opts in by
    /// virtue of declaring triggers, the config lets the operator opt
    /// out per-skill without editing the skill file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub disabled_auto_skills: Vec<String>,
    /// Origins (`scheme://host[:port]`, compared exactly) whose content Live
    /// Pages may embed through `data-kronn-embed`. One list for every Page of
    /// this Kronn; an imported Artifact only declares what it needs.
    /// Served by `/api/config/embed-origins`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub embed_allowed_origins: Vec<String>,
    /// Five-field cron for the scheduled re-verification of cited lines
    /// (`citations.json`). Absent means off, the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub audit_reverify_cron: Option<String>,
    /// KT-999 — give the audit steps that use it the briefing's « Existing
    /// documentation » (reference docs are cited, obsolete ones become tech
    /// debt). Off by default until a bench shows the gain; the env switch
    /// `KRONN_EXP_BRIEFING_DOCS=1` turns it on for a run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(skip)]
    pub audit_briefing_docs: bool,
    /// KT-1345 — task-worker policy (Settings > Workers).
    #[serde(default)]
    pub workers: WorkersConfig,
}

/// KT-1345 — Settings > Workers. The review-round limit every launch path
/// starts from (campaign dialog, single-task MCP launch, delegated subtasks);
/// each launch may still carry its own. Two fields rather than an optional
/// integer so that unticking « Unlimited » gives the previous number back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkersConfig {
    /// Review rounds a task execution may use before it escalates (min 1).
    /// Ignored while `review_rounds_unlimited` is set.
    #[serde(default = "default_workers_review_rounds")]
    pub default_max_review_rounds: u32,
    /// No round cap. Stalls (no delivery, watchdog) still escalate and a campaign's
    /// `token_budget`, when set, still stops every further correction attempt; the
    /// cost is tokens spent on a review that may never converge.
    #[serde(default)]
    pub review_rounds_unlimited: bool,
    /// KT-1633 — run task workers (first dispatch and re-dispatch), their
    /// validations and workflow Agent / Exec steps at background priority
    /// (`taskpolicy -b`, `nice -n 10`, below-normal on Windows), so the owner's
    /// applications and Kronn's own UI stay responsive. On by default; off keeps
    /// every spawn at the backend's priority. `KRONN_BACKGROUND_PRIORITY=0` turns
    /// it off too, whatever this says.
    #[serde(default = "default_true")]
    pub background_priority: bool,
    /// KT-1632 — local task workers (a CLI agent in its own worktree) running at
    /// once. A launch or re-dispatch beyond it waits `Queued`, in launch order,
    /// and starts when a slot frees. Remote (HTTP) and Ollama workers do not
    /// count. `max_concurrent_agents` counts agent processes, not the builds
    /// behind them, so it cannot bound the machine load on its own.
    #[serde(default = "default_workers_max_concurrent_task_workers")]
    pub max_concurrent_task_workers: u32,
    /// KT-1525 — a workflow step output above this many KB is shown as a
    /// « heavy output » warning in the run view and on the workflow card.
    /// The run itself is never refused.
    #[serde(default = "default_workers_heavy_output_kb")]
    pub heavy_output_kb: u32,
    /// KT-1718 — the time, in minutes, a Claude task worker's shell command may
    /// run when it names no limit of its own (`BASH_DEFAULT_TIMEOUT_MS`).
    #[serde(default = "default_workers_bash_default_timeout_min")]
    pub bash_default_timeout_min: u32,
    /// KT-1718 — the longest a Claude task worker's shell command may run, even
    /// when it asks for more (`BASH_MAX_TIMEOUT_MS`). Claude Code's own cap is
    /// 10 minutes: a cold test-profile build outlasts it, and the worker loses
    /// the turn. Keep it below the generation limit (`agent_global_timeout_min`),
    /// which cuts the whole turn whatever a command is allowed.
    #[serde(default = "default_workers_bash_max_timeout_min")]
    pub bash_max_timeout_min: u32,
    /// KT-1750 — file names an operating system or a file manager drops in any
    /// folder (`.DS_Store`, `Thumbs.db`, …): they never count as a worker's
    /// changes, so they block neither a delivery nor the cleanup of a checkout,
    /// and are excluded from `git status` in every lane Kronn creates. A name,
    /// or a name with one `*` standing for any run of characters; an empty list
    /// means none.
    #[serde(default = "default_workers_os_junk_patterns")]
    pub os_junk_patterns: Vec<String>,
    /// KT-1748 — absolute directories whose worktrees Kronn manages, on top of the
    /// built-in roots (`<repo>/.kronn/worktrees`, `~/.kronn/autocode-worktrees`). A
    /// branch checked out under one may be an integration target; the owner's own
    /// checkout never is. A project can name more of its own.
    #[serde(default)]
    pub managed_worktree_roots: Vec<String>,
    /// KT-1750 — who the commits Kronn makes are attributed to, and whether they
    /// carry a `Signed-off-by`, for every project that neither sets its own in
    /// its card nor has a repository that says (its own `user.name` /
    /// `user.email`, a DCO file or hook, a signed history). Nothing set: the
    /// machine's git identity, reported, and no sign-off. A TOML table: after the
    /// plain values.
    #[serde(
        default,
        skip_serializing_if = "super::ProjectCommitSettings::is_default"
    )]
    pub commit: super::ProjectCommitSettings,
    /// KT-1742 — how every project's lane is warmed unless the project says
    /// otherwise. Last field: a TOML table goes after the plain values.
    #[serde(default)]
    pub lane_warm: LaneWarmConfig,
}

impl Default for WorkersConfig {
    fn default() -> Self {
        Self {
            default_max_review_rounds: DEFAULT_MAX_REVIEW_ROUNDS,
            review_rounds_unlimited: false,
            background_priority: true,
            max_concurrent_task_workers: DEFAULT_MAX_CONCURRENT_TASK_WORKERS,
            heavy_output_kb: DEFAULT_HEAVY_OUTPUT_KB,
            bash_default_timeout_min: DEFAULT_BASH_DEFAULT_TIMEOUT_MIN,
            bash_max_timeout_min: DEFAULT_BASH_MAX_TIMEOUT_MIN,
            os_junk_patterns: default_workers_os_junk_patterns(),
            managed_worktree_roots: Vec::new(),
            commit: super::ProjectCommitSettings::default(),
            lane_warm: LaneWarmConfig::default(),
        }
    }
}

/// The OS junk of the three desktop systems: macOS, Windows, and the AppleDouble
/// files macOS leaves on foreign file systems.
pub const DEFAULT_OS_JUNK_PATTERNS: [&str; 4] = [".DS_Store", "Thumbs.db", "desktop.ini", "._*"];
/// Most patterns, and longest one, the Settings form accepts.
pub const MAX_OS_JUNK_PATTERNS: usize = 50;
pub const MAX_OS_JUNK_PATTERN_LEN: usize = 100;

fn default_workers_os_junk_patterns() -> Vec<String> {
    DEFAULT_OS_JUNK_PATTERNS
        .iter()
        .map(|pattern| pattern.to_string())
        .collect()
}

/// Why `pattern` cannot be an OS-junk pattern, or `None` when it can. A pattern
/// is written into the repository's local exclude, so what git would read as
/// something else (a comment, a negation, a path, a character class) is refused.
pub fn os_junk_pattern_problem(pattern: &str) -> Option<String> {
    if pattern.is_empty() {
        return Some("a pattern cannot be empty".into());
    }
    if pattern.len() > MAX_OS_JUNK_PATTERN_LEN {
        return Some(format!(
            "`{pattern}` is longer than {MAX_OS_JUNK_PATTERN_LEN} characters"
        ));
    }
    if pattern != pattern.trim() || pattern.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some(format!(
            "`{pattern}` must not contain spaces or control characters"
        ));
    }
    if pattern.starts_with('#') || pattern.starts_with('!') {
        return Some(format!("`{pattern}` cannot start with `#` or `!`"));
    }
    if pattern.contains(['/', '\\', '[', ']', '?']) {
        return Some(format!(
            "`{pattern}` must be a file name: no `/`, `\\`, `[`, `]` or `?`"
        ));
    }
    if pattern.matches('*').count() > 1 {
        return Some(format!("`{pattern}` may contain only one `*`"));
    }
    if pattern.chars().all(|c| c == '*' || c == '.') {
        return Some(format!("`{pattern}` would match too much"));
    }
    None
}

/// Most managed worktree roots one list (global or per project) may hold.
pub const MAX_MANAGED_WORKTREE_ROOTS: usize = 32;

/// Refuse a list of managed worktree roots the forms must never persist: each
/// entry an absolute path with no `..`, and never the filesystem root (which
/// would make every checkout on the machine Kronn's).
pub fn validate_managed_worktree_roots(roots: &[String]) -> Result<(), String> {
    if roots.len() > MAX_MANAGED_WORKTREE_ROOTS {
        return Err(format!(
            "managed_worktree_roots holds at most {MAX_MANAGED_WORKTREE_ROOTS} paths"
        ));
    }
    for root in roots {
        let path = std::path::Path::new(root.trim());
        if root.trim().is_empty() || root.contains('\0') || !path.is_absolute() {
            return Err(format!(
                "managed_worktree_roots: `{root}` is not an absolute path"
            ));
        }
        if path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(format!(
                "managed_worktree_roots: `{root}` must not contain `..`"
            ));
        }
        if path.parent().is_none() {
            return Err(format!(
                "managed_worktree_roots: `{root}` is the filesystem root; name the directory \
                 that holds the worktrees"
            ));
        }
    }
    Ok(())
}

/// The usable roots of a stored list, whatever a hand-edited file holds: only
/// absolute paths without `..`, and never the filesystem root.
pub fn effective_managed_worktree_roots(roots: &[String]) -> Vec<std::path::PathBuf> {
    roots
        .iter()
        .filter(|root| validate_managed_worktree_roots(std::slice::from_ref(root)).is_ok())
        .map(|root| std::path::PathBuf::from(root.trim()))
        .collect()
}

fn default_workers_review_rounds() -> u32 {
    DEFAULT_MAX_REVIEW_ROUNDS
}

fn default_workers_heavy_output_kb() -> u32 {
    DEFAULT_HEAVY_OUTPUT_KB
}

fn default_workers_bash_default_timeout_min() -> u32 {
    DEFAULT_BASH_DEFAULT_TIMEOUT_MIN
}

fn default_workers_bash_max_timeout_min() -> u32 {
    DEFAULT_BASH_MAX_TIMEOUT_MIN
}

fn default_workers_max_concurrent_task_workers() -> u32 {
    DEFAULT_MAX_CONCURRENT_TASK_WORKERS
}

impl WorkersConfig {
    /// The value stored on a new run / execution: the integer, or the
    /// [`UNLIMITED_REVIEW_ROUNDS`] sentinel. Never 0.
    pub fn effective_max_review_rounds(&self) -> u32 {
        if self.review_rounds_unlimited {
            UNLIMITED_REVIEW_ROUNDS
        } else {
            self.default_max_review_rounds
                .clamp(1, MAX_REVIEW_ROUNDS_INTEGER)
        }
    }

    /// The cap the dispatcher enforces: never 0 (a 0 would queue every worker
    /// for ever), whatever a hand-edited `config.toml` holds.
    pub fn effective_max_concurrent_task_workers(&self) -> usize {
        self.max_concurrent_task_workers
            .clamp(1, MAX_CONCURRENT_TASK_WORKERS_LIMIT) as usize
    }

    /// `BASH_DEFAULT_TIMEOUT_MS` and `BASH_MAX_TIMEOUT_MS` for a Claude task
    /// worker, in milliseconds, whatever a hand-edited `config.toml` holds: the
    /// default never exceeds the maximum.
    pub fn effective_bash_timeouts_ms(&self) -> (u64, u64) {
        let max = self
            .bash_max_timeout_min
            .clamp(1, MAX_BASH_TIMEOUT_MIN_LIMIT);
        let default = self.bash_default_timeout_min.clamp(1, max);
        (u64::from(default) * 60_000, u64::from(max) * 60_000)
    }

    /// Refuse what the Settings form must never persist.
    pub fn validate(&self) -> Result<(), String> {
        self.commit
            .clone()
            .normalized()
            .map_err(|problem| format!("commit: {problem}"))?;
        if self.os_junk_patterns.len() > MAX_OS_JUNK_PATTERNS {
            return Err(format!(
                "os_junk_patterns holds at most {MAX_OS_JUNK_PATTERNS} patterns"
            ));
        }
        for pattern in &self.os_junk_patterns {
            if let Some(problem) = os_junk_pattern_problem(pattern) {
                return Err(format!("os_junk_patterns: {problem}"));
            }
        }
        validate_managed_worktree_roots(&self.managed_worktree_roots)?;
        // KT-1751 review: a project's own setting; the global entry is never
        // read for it, so storing one there would be silently ignored.
        if self
            .lane_warm
            .validation_executables
            .as_ref()
            .is_some_and(|entries| !entries.is_empty())
        {
            return Err(
                "lane_warm.validation_executables is a project's own setting: set it \
                 on the project card (Lane warm-up), not in Settings > Workers"
                    .to_string(),
            );
        }
        if !(1..=MAX_BASH_TIMEOUT_MIN_LIMIT).contains(&self.bash_max_timeout_min) {
            return Err(format!(
                "bash_max_timeout_min must be between 1 and {MAX_BASH_TIMEOUT_MIN_LIMIT}"
            ));
        }
        if !(1..=self.bash_max_timeout_min).contains(&self.bash_default_timeout_min) {
            return Err(format!(
                "bash_default_timeout_min must be between 1 and bash_max_timeout_min ({})",
                self.bash_max_timeout_min
            ));
        }
        if !(1..=MAX_CONCURRENT_TASK_WORKERS_LIMIT).contains(&self.max_concurrent_task_workers) {
            return Err(format!(
                "max_concurrent_task_workers must be between 1 and \
                 {MAX_CONCURRENT_TASK_WORKERS_LIMIT}"
            ));
        }
        if !(1..=MAX_HEAVY_OUTPUT_KB).contains(&self.heavy_output_kb) {
            return Err(format!(
                "heavy_output_kb must be between 1 and {MAX_HEAVY_OUTPUT_KB}"
            ));
        }
        if !(1..=MAX_REVIEW_ROUNDS_INTEGER).contains(&self.default_max_review_rounds) {
            return Err(format!(
                "default_max_review_rounds must be between 1 and {MAX_REVIEW_ROUNDS_INTEGER}"
            ));
        }
        self.lane_warm.validate()
    }
}

/// KT-1742 — how a worker lane is warmed: its compiler environment, whether the
/// seed it is cloned from is aligned with that environment, the command that
/// builds it, and the time that build may take.
///
/// Set once globally (`[workers.lane_warm]`, Settings > Workers) and optionally
/// per project (`ProjectWorkspace::lane_warm`, stored with the project). A field
/// left `None` inherits: the project from the global entry, the global entry
/// from what Kronn detects in the repository. Nothing here is specific to one
/// language or repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaneWarmConfig {
    /// Build the seed (the project's own main-checkout target) once with the
    /// lane's compiler environment, so a lane cloned from it reuses it. Default:
    /// on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub seed_alignment: Option<bool>,
    /// The environment the lane's builds run with for the C compiler, one
    /// `NAME=value` per entry. `None` = detected (on macOS, the target-scoped
    /// `CFLAGS_<triple>=-save-temps=obj` when the project builds C through the
    /// `cc` crate, nothing otherwise); an empty list = none at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub compiler_env: Option<Vec<String>>,
    /// A shell command that replaces the detected first build (run in the
    /// worktree root, same bound, same event). The way to warm a project that is
    /// not a Cargo one: `composer install`, `npm ci`. `None` = detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub first_build_command: Option<String>,
    /// The first build's time limit, in minutes. `None` = 60.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub first_build_timeout_min: Option<u32>,
    /// Which Rust workspace roots of the project to warm, as directories
    /// relative to the repository (`.` for the root itself). `None` = every root
    /// found from the manifests. A project's own setting only: the global entry
    /// cannot name directories of a repository it does not know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rust_roots: Option<Vec<String>>,
    /// KT-1747 — the dependency directories a lane layers read-only from the main
    /// checkout (`node_modules`, `vendor`, `Pods`, … at any depth), as
    /// directories relative to the repository. `None` = the ones detected from
    /// the manifests; a list = exactly those; an empty list = none. A project's
    /// own setting only, like `rust_roots`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dependency_dirs: Option<Vec<String>>,
    /// KT-1751 — the executables this project's validations may run beyond
    /// Kronn's own allowlist: a bare name (`composer`, `go`, `pytest`) or a
    /// repository-relative path (`vendor/bin/phpunit`). The human's decision for
    /// one project, set in its workspace settings: no agent tool writes it and the
    /// global entry is not read for it. A shell is never accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub validation_executables: Option<Vec<String>>,
}

/// KT-1742 — default bound of a lane's first build, in minutes (measured: the
/// `kronn` crate's own two compiles take 33 min at host load 14).
pub const DEFAULT_LANE_FIRST_BUILD_TIMEOUT_MIN: u32 = 60;
/// Largest first-build bound the settings accept (a day).
pub const MAX_LANE_FIRST_BUILD_TIMEOUT_MIN: u32 = 1_440;
const MAX_LANE_ENV_ENTRIES: usize = 32;
const MAX_LANE_ENV_BYTES: usize = 2_000;
const MAX_LANE_COMMAND_BYTES: usize = 4_000;
const MAX_LANE_ROOTS: usize = 16;
/// Variables a lane owns: letting a setting move them would break its sandbox,
/// its target or its isolation.
const LANE_OWNED_ENV: [&str; 6] = [
    "CARGO_TARGET_DIR",
    "CARGO_HOME",
    "CARGO_BUILD_JOBS",
    "CARGO_NET_OFFLINE",
    "TMPDIR",
    "PATH",
];

impl LaneWarmConfig {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `compiler_env` as `(name, value)` pairs, `None` when it is left to
    /// detection. An entry that is not `NAME=value` is skipped (validation
    /// refuses it before it is stored).
    pub fn compiler_env_pairs(&self) -> Option<Vec<(String, String)>> {
        self.compiler_env.as_ref().map(|entries| {
            entries
                .iter()
                .filter_map(|entry| parse_lane_env_entry(entry).ok())
                .collect()
        })
    }

    /// The stored shape: a blank command is no command, a blank entry no entry.
    pub fn normalized(mut self) -> Self {
        self.first_build_command = self
            .first_build_command
            .map(|command| command.trim().to_string())
            .filter(|command| !command.is_empty());
        self.compiler_env = self.compiler_env.map(|entries| {
            entries
                .into_iter()
                .map(|entry| entry.trim().to_string())
                .filter(|entry| !entry.is_empty())
                .collect()
        });
        // A blank list is « every detected root », not « no root ».
        self.rust_roots = self
            .rust_roots
            .map(|roots| {
                let mut kept: Vec<String> = Vec::new();
                for root in roots {
                    let root = normalize_lane_root(&root);
                    if !root.is_empty() && !kept.contains(&root) {
                        kept.push(root);
                    }
                }
                kept
            })
            .filter(|roots| !roots.is_empty());
        // An empty list is kept: it says "layer nothing", not "detect".
        self.dependency_dirs = self.dependency_dirs.map(|dirs| {
            let mut kept: Vec<String> = Vec::new();
            for dir in dirs {
                let dir = normalize_lane_root(&dir);
                if !dir.is_empty() && !kept.contains(&dir) {
                    kept.push(dir);
                }
            }
            kept
        });
        // A blank list is no executable at all.
        self.validation_executables = self
            .validation_executables
            .map(|entries| {
                let mut kept: Vec<String> = Vec::new();
                for entry in entries {
                    let entry = normalize_validation_executable(&entry);
                    if !entry.is_empty() && !kept.contains(&entry) {
                        kept.push(entry);
                    }
                }
                kept
            })
            .filter(|entries| !entries.is_empty());
        self
    }

    /// Refuse what the settings forms must never persist.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(minutes) = self.first_build_timeout_min {
            if !(1..=MAX_LANE_FIRST_BUILD_TIMEOUT_MIN).contains(&minutes) {
                return Err(format!(
                    "first_build_timeout_min must be between 1 and \
                     {MAX_LANE_FIRST_BUILD_TIMEOUT_MIN}"
                ));
            }
        }
        if let Some(command) = &self.first_build_command {
            if command.len() > MAX_LANE_COMMAND_BYTES || command.contains('\0') {
                return Err(format!(
                    "first_build_command is limited to {MAX_LANE_COMMAND_BYTES} characters \
                     and cannot hold a NUL byte; put a long recipe in a script it calls"
                ));
            }
        }
        if let Some(roots) = &self.rust_roots {
            if roots.len() > MAX_LANE_ROOTS {
                return Err(format!("rust_roots is limited to {MAX_LANE_ROOTS} entries"));
            }
            for root in roots.iter().filter(|root| !root.trim().is_empty()) {
                validate_lane_root(root)?;
            }
        }
        if let Some(dirs) = &self.dependency_dirs {
            if dirs.len() > MAX_LANE_ROOTS {
                return Err(format!(
                    "dependency_dirs is limited to {MAX_LANE_ROOTS} entries"
                ));
            }
            for dir in dirs.iter().filter(|dir| !dir.trim().is_empty()) {
                validate_lane_root(dir)
                    .ok()
                    .filter(|_| normalize_lane_root(dir) != ".")
                    .ok_or_else(|| {
                        format!(
                            "dependency_dirs entry `{}` must be a directory inside the \
                             repository (relative, no `..`, not the root itself)",
                            normalize_lane_root(dir)
                        )
                    })?;
            }
        }
        if let Some(entries) = &self.validation_executables {
            if entries.len() > MAX_LANE_EXECUTABLES {
                return Err(format!(
                    "validation_executables is limited to {MAX_LANE_EXECUTABLES} entries"
                ));
            }
            for entry in entries.iter().filter(|entry| !entry.trim().is_empty()) {
                validate_validation_executable(entry)?;
            }
        }
        if let Some(entries) = &self.compiler_env {
            if entries.len() > MAX_LANE_ENV_ENTRIES {
                return Err(format!(
                    "compiler_env is limited to {MAX_LANE_ENV_ENTRIES} entries"
                ));
            }
            for entry in entries.iter().filter(|entry| !entry.trim().is_empty()) {
                parse_lane_env_entry(entry)?;
            }
        }
        Ok(())
    }
}

const MAX_LANE_EXECUTABLES: usize = 32;

/// KT-1751 — one entry of `validation_executables`: a bare program name or a
/// path inside the repository, in plain characters (a validation is a literal
/// argv, so nothing a shell would read), and never a program that runs a command
/// line of its own.
fn validate_validation_executable(entry: &str) -> Result<(), String> {
    let entry = entry.trim();
    // `./gradlew` is a repository path, `gradlew` a program on the PATH.
    let is_path = entry.contains('/');
    let entry = entry.strip_prefix("./").unwrap_or(entry);
    let plain = entry
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | '@' | '/'));
    let refused = |why: &str| {
        Err(format!(
            "validation_executables entry `{entry}` {why}: use a program name \
             (`composer`) or a path inside the repository (`vendor/bin/phpunit`)"
        ))
    };
    if entry.is_empty() || entry.len() > 200 || !plain {
        return refused("holds characters a program name or path does not");
    }
    if is_path {
        let inside = !entry.starts_with('/')
            && entry
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..");
        return if inside {
            Ok(())
        } else {
            refused("is not a path inside the repository")
        };
    }
    if entry.starts_with('-') {
        return refused("is an option, not a program");
    }
    if crate::core::quick_exec::DENIED_BINARIES.contains(&entry) {
        return refused("runs a command line of its own, which a validation never does");
    }
    Ok(())
}

/// KT-1751 review — a stored validation executable: trimmed, and a leading
/// `./` dropped only where the rest is still a path (`./vendor/bin/phpunit` →
/// `vendor/bin/phpunit`). A single-segment repository file keeps it
/// (`./gradlew`): without it, it would be a bare name, a program on the PATH.
pub fn normalize_validation_executable(entry: &str) -> String {
    let entry = entry.trim();
    match entry.strip_prefix("./") {
        Some(rest) if rest.contains('/') => rest.to_string(),
        _ => entry.to_string(),
    }
}

/// `./redis-flush/` → `redis-flush`; the repository root stays `.`.
pub fn normalize_lane_root(root: &str) -> String {
    let trimmed = root.trim().trim_end_matches('/');
    trimmed.strip_prefix("./").unwrap_or(trimmed).to_string()
}

/// A root names a directory inside the repository: relative, no `..`, no NUL.
fn validate_lane_root(root: &str) -> Result<(), String> {
    let root = normalize_lane_root(root);
    let escapes = std::path::Path::new(&root).components().any(|part| {
        !matches!(
            part,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    if root.is_empty() || escapes || root.contains('\0') || root.len() > 300 {
        return Err(format!(
            "rust_roots entry `{root}` must be a directory inside the repository \
             (relative, no `..`)"
        ));
    }
    Ok(())
}

/// `NAME=value` → `(NAME, value)`: a portable variable name, a value without a
/// NUL byte, and not one of the variables a lane owns.
pub fn parse_lane_env_entry(entry: &str) -> Result<(String, String), String> {
    let (name, value) = entry
        .trim()
        .split_once('=')
        .ok_or_else(|| format!("compiler_env entry `{entry}` is not NAME=value"))?;
    let name = name.trim();
    let portable = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !portable {
        return Err(format!(
            "compiler_env name `{name}` must be letters, digits and underscores"
        ));
    }
    if name.starts_with("KRONN_") || LANE_OWNED_ENV.contains(&name) {
        return Err(format!(
            "compiler_env cannot set `{name}`: the lane owns it"
        ));
    }
    if value.len() > MAX_LANE_ENV_BYTES || value.contains('\0') {
        return Err(format!(
            "compiler_env value of `{name}` is too long or holds a NUL byte"
        ));
    }
    Ok((name.to_string(), value.to_string()))
}

/// KT-1525 — default size (KB) above which a step output is flagged heavy.
pub const DEFAULT_HEAVY_OUTPUT_KB: u32 = 256;
/// Largest threshold the Settings form accepts (100 MB).
pub const MAX_HEAVY_OUTPUT_KB: u32 = 102_400;
/// KT-1718 — default time a Claude task worker's shell command may run when it
/// names no limit of its own. Claude Code's own default is 2 minutes.
pub const DEFAULT_BASH_DEFAULT_TIMEOUT_MIN: u32 = 15;
/// KT-1718 — default longest shell command of a Claude task worker (Claude
/// Code's own cap is 10 minutes, which a cold `cargo test` build exceeds).
pub const DEFAULT_BASH_MAX_TIMEOUT_MIN: u32 = 60;
/// Largest per-command cap the Settings form accepts: the largest generation
/// limit (`clamp_agent_global_timeout_min`), since a command outliving the
/// generation is cut with it.
pub const MAX_BASH_TIMEOUT_MIN_LIMIT: u32 = 240;
/// Default cap on local task workers running at once (KT-1632). Builds, not
/// agent processes, are what loads the machine: two lanes is what it carries.
pub const DEFAULT_MAX_CONCURRENT_TASK_WORKERS: u32 = 2;
/// Largest cap the Settings form accepts.
pub const MAX_CONCURRENT_TASK_WORKERS_LIMIT: u32 = 64;
/// Default review-round limit (KT-1345; it was a hard-coded 3).
pub const DEFAULT_MAX_REVIEW_ROUNDS: u32 = 5;
/// Largest explicit integer; above it, say « unlimited ».
pub const MAX_REVIEW_ROUNDS_INTEGER: u32 = 1_000;
/// « No cap », stored in the `max_review_rounds` columns (NOT NULL INTEGER).
/// A sentinel and never 0, which `db::orchestration` refuses.
pub const UNLIMITED_REVIEW_ROUNDS: u32 = u32::MAX;

/// Parse the `max_review_rounds` argument of an MCP launch: an integer or the
/// string `"unlimited"`. Anything else is refused, never defaulted.
pub fn parse_review_rounds_arg(value: &serde_json::Value) -> Result<u32, String> {
    match value {
        serde_json::Value::String(text) if text.trim().eq_ignore_ascii_case("unlimited") => {
            Ok(UNLIMITED_REVIEW_ROUNDS)
        }
        serde_json::Value::Number(number) => number
            .as_u64()
            .filter(|n| (1..=u64::from(MAX_REVIEW_ROUNDS_INTEGER)).contains(n))
            .map(|n| n as u32)
            .ok_or_else(|| {
                format!(
                    "max_review_rounds must be an integer between 1 and \
                     {MAX_REVIEW_ROUNDS_INTEGER}, or \"unlimited\""
                )
            }),
        _ => Err(format!(
            "max_review_rounds must be an integer between 1 and {MAX_REVIEW_ROUNDS_INTEGER}, \
             or \"unlimited\""
        )),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// Embedded desktop listener override; never persisted into the CLI config.
    #[serde(skip)]
    #[ts(skip)]
    pub runtime_port: Option<u16>,
    /// Custom domain for CORS and TLS (e.g. "kronn.local")
    #[serde(default)]
    pub domain: Option<String>,
    /// Bearer token for API authentication (opt-in from Settings UI)
    #[serde(default)]
    #[ts(skip)]
    pub auth_token: Option<String>,
    /// Whether auth was explicitly enabled by the user (distinguishes from migration artifacts)
    #[serde(default)]
    #[ts(skip)]
    pub auth_enabled: bool,
    /// Runtime only: a stored auth token exists but cannot be decrypted (the
    /// key is locked). Not the same as "no token": the middleware then refuses
    /// everything but the routes that restore the key (KT-1007).
    #[serde(skip)]
    #[ts(skip)]
    pub auth_locked: bool,
    /// Runtime only: `auth_token` came from `KRONN_AUTH_TOKEN` while the
    /// credential store was not armed. It is used for this session and never
    /// written to config.toml nor to the store (KT-1007).
    #[serde(skip)]
    #[ts(skip)]
    pub auth_token_session_only: bool,
    /// Strict-auth opt-in: when `true`, the localhost auto-bypass is
    /// disabled and even `127.0.0.1` / Docker bridge clients must
    /// present the Bearer token. Defaults to `false` (current
    /// pragmatic self-hosted behaviour). Flipping to `true` is the
    /// hardening path for users who run multiple processes on the
    /// same host (one of which they don't trust) — e.g. shared dev
    /// VMs. Once peers reach Kronn over TLS by default we'll deprecate the
    /// bypass entirely; this flag is the early-opt-out for users who
    /// can't wait.
    #[serde(default)]
    #[ts(skip)]
    pub auth_strict_localhost: bool,
    /// KT-1492 — "Protect the Kronn vault from agents". On by default: whatever a
    /// trust level grants (a level-approved Quick Exec or Exec line, a validation
    /// a level opened to Docker or the host) runs under an OS sandbox that hides
    /// Kronn's data directory. Off: it runs unconfined, as the owner chose. A host
    /// with no sandbox reports the protection unavailable and refuses nothing.
    #[serde(default = "default_protect_vault_from_agents")]
    #[ts(skip)]
    pub protect_vault_from_agents: bool,
    /// KT-1438 — copy a project's lane seeds (gitignored generated assets, such
    /// as `public/images`) from its main checkout into each task worktree. On by
    /// default; off seeds nothing anywhere, whatever a project declared.
    #[serde(default = "default_true")]
    #[ts(skip)]
    pub lane_seeds_copy_enabled: bool,
    /// KT-1438 — seed Kronn's suggested candidates into the task worktrees of a
    /// project that declared no seeds of its own. Off by default. A candidate
    /// holding a secret-looking file stays held until a human confirms that file.
    #[serde(default)]
    #[ts(skip)]
    pub lane_seeds_adopt_suggested: bool,
    /// KT-1438 — `PUT /api/projects/{id}/lane-seeds` asks for the strong
    /// confirmation (when one is enrolled). On by default; off waives it for that
    /// route only. Changing this flag is itself a `POST /api/config/server`, which
    /// always asks.
    #[serde(default = "default_true")]
    #[ts(skip)]
    pub lane_seeds_strong_confirm: bool,
    /// KT-1596 — the owner declares that a TLS-terminating reverse proxy
    /// (nginx, Caddy, `tailscale serve`) on THIS machine carries every peer's
    /// traffic. Kronn cannot verify it. It lifts the plain-HTTP refusal of the
    /// network-exposure toggle, binds the backend to loopback (only the proxy
    /// reaches it) and withdraws the loopback auth bypass, since every proxied
    /// request then arrives from 127.0.0.1. `KRONN_TLS_TERMINATED_BY_PROXY=1`
    /// declares the same thing without touching this file.
    #[serde(default)]
    #[ts(skip)]
    pub tls_terminated_by_proxy: bool,
    /// KT-1596 — the proxy's public origin (`https://kronn.example.com[:port]`),
    /// the address invite codes advertise and peers dial over wss. Required to
    /// declare a proxy; `KRONN_PUBLIC_ORIGIN` supplies it without this file.
    #[serde(default)]
    #[ts(skip)]
    pub public_origin: Option<String>,
    /// Accept federation peers (inbound `/api/ws` peers, the peer HTTP routes)
    /// and dial contacts. Off by default: a peer is identified only by its
    /// invite code until pairing secrets exist (KT-1033).
    #[serde(default)]
    pub p2p_enabled: bool,
    /// Extra browser origins of this Kronn's frontend (LAN or Tailscale
    /// aliases), exact `scheme://host[:port]`. Checked on the WS upgrade.
    #[serde(default)]
    pub frontend_origins: Vec<String>,
    /// 0.8.11 (B6) — optional webhook (Slack/Teams/generic JSON) fired when a
    /// scheduled/triggered run ends in a non-success terminal state
    /// (Failed / Interrupted / StoppedByGuard). Lets an autonomous cron that
    /// dies at 6am surface immediately instead of being discovered by opening
    /// the UI. Empty/None = no notification. Also settable via
    /// `KRONN_FAILURE_NOTIFY_URL`.
    #[serde(default)]
    pub failure_notify_url: Option<String>,
    /// Delete whole workflow runs older than N days. `0` (default) never
    /// deletes run history. Applies the same rules as the payload trim below
    /// (plain, terminal, unreferenced runs that own no worktree), in chunks.
    #[serde(default)]
    pub run_retention_days: u32,
    /// KT-984 — blank the step outputs of workflow runs finished more than N
    /// days ago, keeping every run row and its metadata. `0` keeps outputs
    /// forever. Off on every install until the user chooses a window (Settings
    /// suggests 30). See `db::run_retention` for what is never touched.
    #[serde(default)]
    pub run_payload_retention_days: u32,
    /// Encrypted execution-variable snapshot retention. `0` keeps metadata
    /// but disables value retention. Product default: 30 days.
    #[serde(default = "default_execution_variable_retention_days")]
    pub execution_variable_retention_days: u32,
    /// Days after which boot reclaims the clean worktree of an `Interrupted`
    /// run nobody resumed; its commits stay on a preserved branch. `0` = never.
    #[serde(default = "default_interrupted_worktree_ttl_days")]
    pub interrupted_worktree_ttl_days: u32,
    /// KT-373 — refuse to provision a worktree below this much free disk, in
    /// GiB. On 2026-08-21 the dev volume hit 100% with seven worktrees each
    /// holding its own Rust `target/`; provisioning kept going until nothing
    /// worked. A build that is refused costs a message, a build that fills the
    /// disk costs the machine.
    #[serde(default = "default_disk_critical_gib")]
    pub disk_critical_gib: u64,
    /// Warn — but still provision — below this much free disk, in GiB. Must
    /// stay above `disk_critical_gib` to mean anything; see
    /// `disk_thresholds()`, which is the only place they are read together.
    #[serde(default = "default_disk_warning_gib")]
    pub disk_warning_gib: u64,
    /// KT-1318 — free space, in GiB, a worker launch (or a full audit) needs on
    /// the volume holding the worktrees and lane build dirs. Below it Kronn
    /// sweeps its orphaned lane build dirs first, then refuses the launch with
    /// the reason and an alert in the principal room. `0` disables the guard.
    #[serde(default = "default_disk_launch_min_free_gib")]
    pub disk_launch_min_free_gib: u64,
    /// KT-1318 — hours a Kronn-created lane build dir must be unwritten, with no
    /// live worktree, before a sweep (boot, periodic, pressure) may remove it.
    #[serde(default = "default_disk_sweep_idle_hours")]
    pub disk_sweep_idle_hours: u64,
    /// KT-1691 — days a Blocked or Interrupted execution's lane build dir must
    /// have been unwritten before it becomes purgeable (by a confirmed
    /// per-entry delete, never by a sweep). Its worktree and branch stay.
    #[serde(default = "default_disk_stalled_build_days")]
    pub disk_stalled_build_days: u64,
    /// Maximum concurrent agent processes (default: 5)
    #[serde(default = "default_max_agents")]
    pub max_concurrent_agents: usize,
    /// Agent stall timeout in minutes — abort if no output for this long (default: 5)
    #[serde(default = "default_agent_stall_timeout")]
    pub agent_stall_timeout_min: u32,
    /// Absolute wall-clock limit for one agent execution, even while it keeps
    /// producing output (default: 30). This is distinct from the inactivity
    /// watchdog above and is read when each new run starts.
    #[serde(default = "default_agent_global_timeout")]
    pub agent_global_timeout_min: u32,
    /// Absolute wall-clock limit for locally-served HTTP agents (Ollama).
    /// Local inference is legitimately much slower than a hosted CLI, so it
    /// has an explicit, visible budget instead of a hidden runtime multiplier.
    #[serde(default = "default_local_agent_global_timeout")]
    pub local_agent_global_timeout_min: u32,
    /// KT-405 — per-model context override, persisted (unlike
    /// `KRONN_OLLAMA_NUM_CTX_CAP`, which is process-global and disappears on
    /// restart). Keyed by the exact Ollama model tag (`"qwen3.8:27b-mlx"`).
    /// The env var still wins when set — it is the break-glass escape hatch
    /// for an operator who cannot reach the UI; this is the persistent,
    /// per-model dial meant to be set once and forgotten. Bounds and warnings
    /// against the model's advertised window / this machine's RAM ceiling are
    /// enforced at the setter, not here — a deserialized config from an older
    /// Kronn or a smaller machine must still load.
    #[serde(default)]
    pub ollama_context_overrides: std::collections::HashMap<String, u64>,
    /// KT-1529 — hard ceiling, in bytes, on one discussion message or initial
    /// prompt. The everyday limit is derived from the target model's context
    /// window; this is the safety net above every window (default: 4 MB).
    #[serde(default = "default_max_prompt_bytes")]
    pub max_prompt_bytes: usize,
    /// User identity — displayed in messages and used for future multi-user
    #[serde(default)]
    pub pseudo: Option<String>,
    /// Email for Gravatar avatar (optional, decoupled from git)
    #[serde(default)]
    pub avatar_email: Option<String>,
    /// Short bio — who the user is, their role, expertise. Injected at the start of first message in a discussion.
    #[serde(default)]
    pub bio: Option<String>,
    /// Global context injected into discussions. Markdown content — glossary,
    /// company conventions, stack overview, etc. Supplements project-level
    /// `ai/` context. Stored in config.toml.
    #[serde(default)]
    pub global_context: Option<String>,
    /// When to inject global_context:
    /// - `"always"` (default) — every discussion
    /// - `"no_project"` — only discussions without a project
    /// - `"never"` — disabled
    #[serde(default = "default_global_context_mode")]
    pub global_context_mode: String,
    /// 0.8.7 anti-hallucination mode: `off` | `warn` | `enforce`.
    ///
    /// - `off` — feature disabled, nothing injected or linted.
    /// - `warn` (default) — P1 sourcing directive injected + P2 lint (heuristic + mechanical `[src:]` verification) surfaced as a non-blocking pill.
    /// - `enforce` — same as `warn` in 0.8.7; reserved for the Phase 3 write-time refusal of unverifiable citations.
    ///
    /// See `core::anti_halluc`. Mirrored into the process-global flag at load + save.
    #[serde(default = "default_anti_hallucination_mode")]
    pub anti_hallucination_mode: String,
    /// 0.10.0 — Continual Learning master toggle. **Default OFF (beta)**: the
    /// feature writes agent-proposed learnings into injected truth files
    /// (`docs/learnings.md` / user-context), so it ships opt-in to avoid a bug
    /// polluting a user's docs. Gates capture (`learning_propose`), the
    /// `kronn:section name="learnings"` doc pointer, and the UI badge/modal.
    /// Validating/rejecting EXISTING pending candidates stays allowed when off
    /// (drain, don't capture). See docs/research/continual-learning-0.10.0-spec.md §0.
    #[serde(default)]
    pub continual_learning_enabled: bool,
    /// Show the per-message out-of-context note control in discussion
    /// composers. Enabled by default; turning it off hides the authoring
    /// affordance without deleting existing notes.
    #[serde(default = "default_true")]
    pub discussion_notes_enabled: bool,
    /// Debug mode — when true, the tracing subscriber is initialized at
    /// `debug` level instead of `info`, producing significantly more
    /// output on stdout. Lets users diagnose agent detection / project
    /// scan issues themselves without needing to set `RUST_LOG` by hand.
    /// Persisted in config.toml so it survives restarts. Toggleable from
    /// the Settings UI or via `./kronn start --debug` (CLI flag wins for
    /// the duration of that run).
    #[serde(default)]
    pub debug_mode: bool,
    /// 0.8.6 phase 4 — default model tier applied to NEW creations
    /// (discussions, QP drafts, workflow Agent steps) when the user
    /// doesn't explicitly pick one in the form. STRICT semantic :
    /// only consulted by creation flows on `componentDidMount` ; never
    /// applied retroactively to existing items at execution time
    /// (otherwise a user flipping the default to `Reasoning` would
    /// silently 10x the cost of every legacy QP they launch).
    ///
    /// Persisted in `config.toml`. Defaults to `Default` for
    /// backwards-compat — existing configs without the field keep
    /// the prior hardcoded behaviour.
    #[serde(default)]
    pub default_model_tier: ModelTier,
    /// 0.8.6 phase 4 — default summary strategy applied to NEW
    /// discussions. Flipped from `Auto` to `Off` because most modern
    /// agents (Claude Code, Codex, Gemini-Pro) have large context
    /// windows AND can pull older history on-demand via the
    /// `disc_load_other` MCP tool — auto-summary just burns Economy
    /// tokens for no win in those cases. The `Off` default makes
    /// Kronn cheaper out of the box.
    ///
    /// Re-enable `Auto` (Settings) for small-context or HTTP agents when a
    /// proactive summary is preferable. HTTP agents have no arbitrary MCP
    /// bridge, but a discussion run may receive bounded Kronn-native history
    /// tools; the chosen strategy remains an operator trade-off.
    ///
    /// Strict semantic — only consulted on NEW disc creation. Existing
    /// discs keep their saved value (no retroactive change).
    #[serde(default = "default_summary_strategy_off")]
    pub default_summary_strategy: crate::models::SummaryStrategy,
    /// Allow an agent's final prose to explicitly hand work to another
    /// attached agent through a canonical `@alias`. Opt-in because one
    /// generated reply can otherwise start additional paid runs.
    #[serde(default)]
    pub agent_handoffs_enabled: bool,
    /// Maximum paid or cost-unknown handoffs spawned from one originating
    /// human turn. Ollama is local and uses a separate fixed safety ceiling.
    #[serde(default = "default_agent_handoff_paid_limit")]
    pub agent_handoff_paid_limit: u32,
    /// Remove the paid/unknown per-turn quota while keeping the structural
    /// loop guards (attached agents only and bounded delegation depth).
    #[serde(default)]
    pub agent_handoff_paid_unlimited: bool,
    /// Agents that cannot be started automatically from another agent's
    /// generated reply. Empty keeps the historical allow-all behaviour.
    #[serde(default)]
    pub agent_handoff_blocked_agents: Vec<AgentType>,
    /// Sidebar storage-weight indicator. Validation and fallback live in
    /// `models::discussion_weight`; this is only the persisted field.
    #[serde(default)]
    pub discussion_weight: crate::models::DiscussionWeightConfig,
    /// IANA zone crons, watches and `{{time.now}}` default to (KT-1103).
    /// `None` follows the machine's zone, detected at boot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl ServerConfig {
    /// Whether the auth middleware enforces something for the LAN boot guard:
    /// a token, or the locked state (which refuses all but local recovery).
    pub fn auth_token_or_lock(&self) -> bool {
        self.auth_token.is_some() || self.auth_locked
    }

    /// Actual listener port for this process; `port` remains the saved preference.
    pub fn listening_port(&self) -> u16 {
        self.runtime_port.unwrap_or(self.port)
    }
}

/// Serde default for [`ServerConfig::default_summary_strategy`].
/// Returns `Off` so a missing field in config.toml means "auto-summary
/// disabled" — the new safer default shipped 0.8.6 phase 4.
fn default_summary_strategy_off() -> crate::models::SummaryStrategy {
    crate::models::SummaryStrategy::Off
}

/// Serde default for [`ServerConfig::protect_vault_from_agents`]: on.
fn default_protect_vault_from_agents() -> bool {
    true
}

fn default_agent_handoff_paid_limit() -> u32 {
    1
}

fn default_global_context_mode() -> String {
    "always".to_string()
}
fn default_anti_hallucination_mode() -> String {
    crate::core::anti_halluc::DEFAULT_MODE_STR.to_string()
}
/// A cold `cargo build` of this workspace writes a few GiB; the floor is set
/// so a refused provisioning still leaves room to work, investigate and clean.
pub(crate) const DEFAULT_DISK_CRITICAL_GIB: u64 = 5;
pub(crate) const DEFAULT_DISK_WARNING_GIB: u64 = 20;
/// KT-1318 — a worker's first build alone writes several GiB and a lane has
/// been measured at 5-30 GiB; 30 GiB is the room a launch needs to finish.
pub(crate) const DEFAULT_DISK_LAUNCH_MIN_FREE_GIB: u64 = 30;
pub(crate) const DEFAULT_DISK_SWEEP_IDLE_HOURS: u64 = 6;
/// KT-1691 — a Blocked or Interrupted execution that left its build dir alone
/// this long is not coming back soon; the next resume rebuilds.
pub(crate) const DEFAULT_DISK_STALLED_BUILD_DAYS: u64 = 3;
fn default_disk_launch_min_free_gib() -> u64 {
    DEFAULT_DISK_LAUNCH_MIN_FREE_GIB
}
fn default_disk_sweep_idle_hours() -> u64 {
    DEFAULT_DISK_SWEEP_IDLE_HOURS
}
fn default_disk_stalled_build_days() -> u64 {
    DEFAULT_DISK_STALLED_BUILD_DAYS
}
fn default_disk_critical_gib() -> u64 {
    DEFAULT_DISK_CRITICAL_GIB
}

fn default_execution_variable_retention_days() -> u32 {
    30
}
pub(crate) const DEFAULT_INTERRUPTED_WORKTREE_TTL_DAYS: u32 = 7;
fn default_interrupted_worktree_ttl_days() -> u32 {
    DEFAULT_INTERRUPTED_WORKTREE_TTL_DAYS
}
fn default_disk_warning_gib() -> u64 {
    DEFAULT_DISK_WARNING_GIB
}
fn default_max_agents() -> usize {
    5
}
fn default_agent_stall_timeout() -> u32 {
    5
}
pub(crate) const DEFAULT_MAX_PROMPT_BYTES: usize = 4_000_000;
fn default_max_prompt_bytes() -> usize {
    DEFAULT_MAX_PROMPT_BYTES
}
// 240, raised from 120 (KT-403): a hard task on a locally-served 27B measured
// at ~2 h 40 of honest work. The old ceiling existed to stop hung runs from
// squatting the agent semaphore; since an abandoned tool loop now halts itself
// at the next round boundary, a long window no longer keeps a dead run alive.
pub(crate) const MAX_AGENT_GLOBAL_TIMEOUT_MIN: u32 = 240;
pub(crate) const DEFAULT_AGENT_GLOBAL_TIMEOUT_MIN: u32 = 30;
fn default_agent_global_timeout() -> u32 {
    DEFAULT_AGENT_GLOBAL_TIMEOUT_MIN
}

pub(crate) const DEFAULT_LOCAL_AGENT_GLOBAL_TIMEOUT_MIN: u32 = 240;
fn default_local_agent_global_timeout() -> u32 {
    DEFAULT_LOCAL_AGENT_GLOBAL_TIMEOUT_MIN
}

pub(crate) fn clamp_agent_global_timeout_min(input: u64) -> u32 {
    input.clamp(1, u64::from(MAX_AGENT_GLOBAL_TIMEOUT_MIN)) as u32
}

/// Default output language. Used by `AppConfig.language` AND by API
/// request types deserialized from the frontend (where the user may
/// omit the field). `pub(crate)` so other model sub-modules can keep
/// the `default = "..."` attribute working after extraction.
pub(crate) fn default_language() -> String {
    "fr".into()
}
pub(crate) fn default_ui_language() -> String {
    "fr".into()
}

// ─── Tokens / API keys ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokensConfig {
    /// Legacy fields — kept for backward compat when reading old config.toml
    #[serde(default, skip_serializing)]
    pub anthropic: Option<String>,
    #[serde(default, skip_serializing)]
    pub openai: Option<String>,
    #[serde(default, skip_serializing)]
    pub google: Option<String>,
    /// All API keys (new multi-key system)
    #[serde(default)]
    pub keys: Vec<ApiKey>,
    #[serde(default)]
    pub disabled_overrides: Vec<String>,
}

impl TokensConfig {
    /// Get the active key value for a provider, or None
    pub fn active_key_for(&self, provider: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|k| k.provider == provider && k.active)
            .map(|k| k.value.as_str())
    }

    /// Whether provider credential metadata declares an active key, without
    /// returning or inspecting the secret value itself.
    pub fn has_active_key_for(&self, provider: &str) -> bool {
        self.keys
            .iter()
            .any(|key| key.provider == provider && key.active)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub provider: String,
    #[ts(skip)]
    pub value: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiKeyDisplay {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub masked_value: String,
    pub active: bool,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SaveApiKeyRequest {
    pub id: Option<String>,
    pub name: String,
    pub provider: String,
    pub value: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ApiKeysResponse {
    pub keys: Vec<ApiKeyDisplay>,
    pub disabled_overrides: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscoveredKey {
    pub provider: String,
    pub source: String,
    pub suggested_name: String,
    pub already_exists: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscoverKeysResponse {
    pub discovered: Vec<DiscoveredKey>,
    pub imported_count: u32,
}

// ─── Scan ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScanConfig {
    pub paths: Vec<String>,
    pub ignore: Vec<String>,
    /// Max depth when scanning for git repos (2–10, default 4)
    #[serde(default = "default_scan_depth")]
    pub scan_depth: usize,
    /// Default AI doc folder, relative to each project root (KT-263). `None`
    /// keeps the historical detection (`docs/` → `doc/` → `ai/`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default_docs_dir: Option<String>,
}

fn default_scan_depth() -> usize {
    4
}

// ─── Agents ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentsConfig {
    pub claude_code: AgentConfig,
    pub codex: AgentConfig,
    #[serde(default)]
    pub open_code: AgentConfig,
    #[serde(default)]
    pub gemini_cli: AgentConfig,
    #[serde(default)]
    pub kiro: AgentConfig,
    #[serde(default)]
    pub vibe: AgentConfig,
    #[serde(default)]
    pub copilot_cli: AgentConfig,
    #[serde(default)]
    pub ollama: AgentConfig,
    #[serde(default)]
    pub lite_llm: AgentConfig,
    #[serde(default)]
    pub nvidia: AgentConfig,
    /// Per-agent model tier overrides (Economy/Reasoning model names).
    #[serde(default)]
    pub model_tiers: ModelTiersConfig,
    /// KT-1706 — what takes over a mode of a named HTTP connection (or the image or
    /// video mode of ComfyUI) when its provider cannot serve a request. Kept apart
    /// from the agents' `mode_fallbacks`: a connection is not an `AgentType`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connection_fallbacks: Vec<ConnectionFallback>,
}

/// Endpoint slots of the OpenAI-wire providers, carried as one value.
///
/// Both slots travel together deliberately. While they were two independent
/// fields on the spawn config, every call site wired LiteLLM's and none wired
/// NVIDIA's, so a configured NVIDIA endpoint never reached the runner and the
/// public default silently won (KT-337). One value means a call site cannot
/// wire one provider and forget the other.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HttpEndpoints {
    pub lite_llm: Option<String>,
    pub nvidia: Option<String>,
    /// The operator's tool-executor switch per HTTP agent (KT-1600), carried
    /// with the endpoints because every call site already hands them to the
    /// runner. `Some(false)` = off, anything else = on. A named `Custom`
    /// connection has its own on `ExternalHttpRuntime::tools_enabled`.
    pub tools: HttpToolSettings,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HttpToolSettings {
    pub ollama: Option<bool>,
    pub lite_llm: Option<bool>,
    pub nvidia: Option<bool>,
}

impl HttpEndpoints {
    pub fn from_agents(agents: &AgentsConfig) -> Self {
        Self {
            lite_llm: agents.lite_llm.base_url.clone(),
            nvidia: agents.nvidia.base_url.clone(),
            tools: HttpToolSettings {
                ollama: agents.ollama.tools_enabled,
                lite_llm: agents.lite_llm.tools_enabled,
                nvidia: agents.nvidia.tools_enabled,
            },
        }
    }

    /// The explicit tool setting for this agent, `None` when none is declared
    /// or the agent has no slot (a CLI agent, or `Custom`).
    pub fn tools_setting_for(&self, agent_type: &AgentType) -> Option<bool> {
        match agent_type {
            AgentType::Ollama => self.tools.ollama,
            AgentType::LiteLlm => self.tools.lite_llm,
            AgentType::Nvidia => self.tools.nvidia,
            _ => None,
        }
    }

    /// The endpoint slot this agent reads, `None` meaning "fall back to the
    /// provider's own default".
    pub fn for_agent(&self, agent_type: &AgentType) -> Option<&str> {
        // Deliberately EXHAUSTIVE, no catch-all. The previous `_ =>` arm handed
        // LiteLLM's proxy to every other agent, so a new OpenAI-wire provider
        // (OpenRouter, Together, …) would have inherited someone else's endpoint
        // silently — the comment warned about it without preventing it. Listing
        // every variant means the next provider fails to COMPILE here and has to
        // choose its slot, which is the only guard that cannot be forgotten.
        match agent_type {
            AgentType::Nvidia => self.nvidia.as_deref(),
            AgentType::LiteLlm => self.lite_llm.as_deref(),
            // Ollama shares the HTTP chat path but this production config has
            // no Ollama endpoint slot: the runner therefore falls back to
            // OLLAMA_HOST/Docker/localhost. CLI agents never reach that path.
            // `None` is the honest answer for both, not a fallback.
            AgentType::Ollama
            | AgentType::ClaudeCode
            | AgentType::Codex
            | AgentType::OpenCode
            | AgentType::Vibe
            | AgentType::GeminiCli
            | AgentType::Kiro
            | AgentType::CopilotCli
            | AgentType::Custom => None,
        }
    }
}

impl AgentsConfig {
    /// The stored settings of one agent. `Custom` is a bucket of named
    /// connections, not an agent with a card, so it has none.
    pub fn agent_config(&self, agent: &AgentType) -> Option<&AgentConfig> {
        Some(match agent {
            AgentType::ClaudeCode => &self.claude_code,
            AgentType::Codex => &self.codex,
            AgentType::OpenCode => &self.open_code,
            AgentType::GeminiCli => &self.gemini_cli,
            AgentType::Kiro => &self.kiro,
            AgentType::Vibe => &self.vibe,
            AgentType::CopilotCli => &self.copilot_cli,
            AgentType::Ollama => &self.ollama,
            AgentType::LiteLlm => &self.lite_llm,
            AgentType::Nvidia => &self.nvidia,
            AgentType::Custom => return None,
        })
    }

    /// Mutable twin of [`Self::agent_config`].
    pub fn agent_config_mut(&mut self, agent: &AgentType) -> Option<&mut AgentConfig> {
        Some(match agent {
            AgentType::ClaudeCode => &mut self.claude_code,
            AgentType::Codex => &mut self.codex,
            AgentType::OpenCode => &mut self.open_code,
            AgentType::GeminiCli => &mut self.gemini_cli,
            AgentType::Kiro => &mut self.kiro,
            AgentType::Vibe => &mut self.vibe,
            AgentType::CopilotCli => &mut self.copilot_cli,
            AgentType::Ollama => &mut self.ollama,
            AgentType::LiteLlm => &mut self.lite_llm,
            AgentType::Nvidia => &mut self.nvidia,
            AgentType::Custom => return None,
        })
    }

    /// Set `full_access` for a CLI agent. Every one of them reads it: Claude and
    /// Codex through their adapter's flag, the others by widening the ACP broker.
    pub fn set_full_access(&mut self, agent: &AgentType, value: bool) -> Result<(), &'static str> {
        let slot = match agent {
            AgentType::ClaudeCode => &mut self.claude_code,
            AgentType::Codex => &mut self.codex,
            AgentType::OpenCode => &mut self.open_code,
            AgentType::GeminiCli => &mut self.gemini_cli,
            AgentType::CopilotCli => &mut self.copilot_cli,
            AgentType::Kiro => &mut self.kiro,
            AgentType::Vibe => &mut self.vibe,
            _ => return Err("Agent does not support access flags"),
        };
        slot.full_access = value;
        Ok(())
    }

    /// What a launch really does, which is not always the stored setting: Codex's
    /// own sandbox cannot start inside the container, so there it always runs
    /// unsandboxed. The reason is a stable code for the UI.
    pub fn effective_full_access(
        &self,
        agent: &AgentType,
        in_container: bool,
    ) -> (bool, Option<&'static str>) {
        let setting = self.full_access_for(agent);
        if matches!(agent, AgentType::Codex) && in_container && !setting {
            return (true, Some("forced_in_container"));
        }
        (setting, None)
    }

    /// Get the full_access setting for a given agent type.
    pub fn full_access_for(&self, agent: &AgentType) -> bool {
        match agent {
            AgentType::ClaudeCode => self.claude_code.full_access,
            AgentType::Codex => self.codex.full_access,
            AgentType::OpenCode => self.open_code.full_access,
            AgentType::GeminiCli => self.gemini_cli.full_access,
            AgentType::Kiro => self.kiro.full_access,
            AgentType::Vibe => self.vibe.full_access,
            AgentType::CopilotCli => self.copilot_cli.full_access,
            AgentType::Ollama => self.ollama.full_access,
            AgentType::LiteLlm => self.lite_llm.full_access,
            AgentType::Nvidia => self.nvidia.full_access,
            _ => false,
        }
    }

    pub fn any_full_access(&self) -> bool {
        self.claude_code.full_access
            || self.codex.full_access
            || self.open_code.full_access
            || self.gemini_cli.full_access
            || self.kiro.full_access
            || self.vibe.full_access
            || self.copilot_cli.full_access
            || self.ollama.full_access
            || self.lite_llm.full_access
            || self.nvidia.full_access
    }

    /// Returns true if at least one agent is marked as installed.
    pub fn any_installed(&self) -> bool {
        self.claude_code.installed
            || self.codex.installed
            || self.open_code.installed
            || self.gemini_cli.installed
            || self.kiro.installed
            || self.vibe.installed
            || self.copilot_cli.installed
            || self.ollama.installed
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentConfig {
    pub path: Option<String>,
    #[serde(default)]
    pub installed: bool,
    pub version: Option<String>,
    #[serde(default)]
    pub full_access: bool,
    /// How many runs of THIS agent may execute at once. `None` takes the
    /// built-in default for its family. A remote HTTP provider parallelises
    /// fine and is left unlimited; a local one is not — Ollama serves a single
    /// inference slot, so extra runs only queue and throw away its KV cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    /// Optional UI color used for this agent's canonical `@mention`.
    /// `None` keeps the built-in frontend color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mention_color: Option<String>,
    /// Where to reach an agent that is a server rather than a binary. Only
    /// LiteLLM uses it today: its proxy can live anywhere, so the endpoint is
    /// the user's to declare. The matching credential lives in `TokensConfig`
    /// under the `litellm` provider, never here — this struct is serialised
    /// to the frontend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// KT-1693 — what takes over when THIS agent is unavailable on the provider
    /// side (rate limit, quota or credits spent, outage, CLI not authenticated),
    /// one row per mode (eco / standard / advanced / decision / image / video):
    /// at most one fallback per mode, off by default. When the fallback is itself
    /// unavailable, the fallback set on ITS own row applies. A discussion or a
    /// workflow step may override a mode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mode_fallbacks: Vec<ModeFallback>,
    /// KT-1518 — the agent-level chain this version replaced. Read once at
    /// start-up and carried onto the text mode rows
    /// (`AgentsConfig::migrate_fallback_chains`), then cleared; never written.
    #[serde(default, skip_serializing)]
    #[ts(skip)]
    pub fallback_chain: Vec<AgentFallback>,
    /// Operator switch for the Kronn tool executor on an HTTP agent (KT-1600).
    /// Tools are on by default: `Some(false)` withholds them from every model
    /// of this agent (for one that cannot take tool calls); `None` and
    /// `Some(true)` leave them on, Ollama's catalogue still withholding them
    /// from a model it lists without `tools`. Ignored by CLI agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_enabled: Option<bool>,
}

/// KT-1693 — a mode of an agent: the kind of work one model of it serves. The
/// three text modes are the quality tiers (`ModelTier`); decision, image and
/// video are capabilities, never tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FallbackMode {
    Eco,
    Standard,
    Advanced,
    Decision,
    Image,
    Video,
}

/// What a mode produces: a fallback only ever goes to the same kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackKind {
    Text,
    Decision,
    Image,
    Video,
}

impl FallbackMode {
    pub const ALL: [FallbackMode; 6] = [
        FallbackMode::Eco,
        FallbackMode::Standard,
        FallbackMode::Advanced,
        FallbackMode::Decision,
        FallbackMode::Image,
        FallbackMode::Video,
    ];

    /// The three text modes, which an agent-level chain of 0.15.3 is carried to.
    pub const TEXT: [FallbackMode; 3] = [
        FallbackMode::Eco,
        FallbackMode::Standard,
        FallbackMode::Advanced,
    ];

    pub fn kind(self) -> FallbackKind {
        match self {
            FallbackMode::Eco | FallbackMode::Standard | FallbackMode::Advanced => {
                FallbackKind::Text
            }
            FallbackMode::Decision => FallbackKind::Decision,
            FallbackMode::Image => FallbackKind::Image,
            FallbackMode::Video => FallbackKind::Video,
        }
    }

    pub fn from_tier(tier: ModelTier) -> Self {
        match tier {
            ModelTier::Economy => FallbackMode::Eco,
            ModelTier::Default => FallbackMode::Standard,
            ModelTier::Reasoning => FallbackMode::Advanced,
        }
    }

    /// The tier a text mode resolves to; `None` for decision, image and video.
    pub fn tier(self) -> Option<ModelTier> {
        match self {
            FallbackMode::Eco => Some(ModelTier::Economy),
            FallbackMode::Standard => Some(ModelTier::Default),
            FallbackMode::Advanced => Some(ModelTier::Reasoning),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FallbackMode::Eco => "eco",
            FallbackMode::Standard => "standard",
            FallbackMode::Advanced => "advanced",
            FallbackMode::Decision => "decision",
            FallbackMode::Image => "image",
            FallbackMode::Video => "video",
        }
    }
}

/// What a fallback cannot give that the agent it replaces could: a stable code,
/// so the UI translates the warning instead of showing an English sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FallbackGap {
    /// No MCP tools.
    NoMcpTools,
    /// Cannot write in the worktree.
    NoWriteAccess,
    /// Images are not guaranteed.
    NoImageInput,
    /// A capped context window.
    CappedContext,
}

/// How an agent is reached; shown as a badge on every fallback pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FallbackTransport {
    /// A CLI process on this machine (OpenCode's ACP included).
    Cli,
    /// A remote HTTP API (LiteLLM, NVIDIA, a named connection).
    Http,
    /// A local model server (Ollama).
    Local,
}

impl FallbackTransport {
    pub fn label(self) -> &'static str {
        match self {
            FallbackTransport::Cli => "CLI",
            FallbackTransport::Http => "HTTP",
            FallbackTransport::Local => "Local",
        }
    }
}

/// KT-1693 — one fallback: the agent and, optionally, the model that takes over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentFallback {
    pub agent: AgentType,
    /// A specific model on the fallback. `None`: the mode below (else the SAME
    /// mode as the interrupted turn), resolved on the fallback agent (a model
    /// name is provider-specific, so the failing agent's model is never carried
    /// over).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The mode of the model the fallback runs on. `None`: the mode of the row
    /// (or of the turn) it replaces. Must be of the same kind as that mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<FallbackMode>,
}

/// KT-1693 — the fallback set on one mode row of an agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModeFallback {
    pub row: FallbackMode,
    pub fallback: AgentFallback,
}

/// KT-1706 — where a connection-scoped fallback runs: a named HTTP connection (its
/// id) or, for image and video, ComfyUI (`local:comfyui`), and optionally the
/// model; without one, the model that target has in its own slot of the mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConnectionFallbackTarget {
    pub connection: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// KT-1706 — the fallback of one mode of an owner (a connection id, or
/// `local:comfyui` for `image` and `video`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConnectionFallback {
    pub owner: String,
    pub row: FallbackMode,
    pub fallback: ConnectionFallbackTarget,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetConnectionFallbackRequest {
    pub owner: String,
    pub mode: FallbackMode,
    /// The fallback of this row; `None` removes it.
    pub fallback: Option<ConnectionFallbackTarget>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetAgentFallbackRequest {
    pub agent: AgentType,
    pub mode: FallbackMode,
    /// The fallback of this mode row; `None` removes it.
    pub fallback: Option<AgentFallback>,
}

// ─── Model tiers ──────────────────────────────────────────────────────────

/// Abstract model capability tier. Kronn maps each tier to a concrete --model flag per agent.
/// Priority: AgentSettings.model (explicit) > ModelTier > Default (no flag).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ModelTier {
    /// Cheap/fast model (haiku, gpt-4.1-mini, flash). For summaries, bulk ops.
    Economy,
    /// Agent's built-in default. No --model flag passed.
    #[default]
    Default,
    /// Most capable model (opus, o4-mini, pro). For audits, complex analysis.
    Reasoning,
}

impl ModelTier {
    /// The label serde writes (`economy` / `default` / `reasoning`), as stored
    /// on an audit run.
    pub fn as_str(self) -> &'static str {
        match self {
            ModelTier::Economy => "economy",
            ModelTier::Default => "default",
            ModelTier::Reasoning => "reasoning",
        }
    }

    /// Inverse of [`as_str`]; `None` for anything else.
    pub fn parse(label: &str) -> Option<Self> {
        match label {
            "economy" => Some(ModelTier::Economy),
            "default" => Some(ModelTier::Default),
            "reasoning" => Some(ModelTier::Reasoning),
            _ => None,
        }
    }
}

/// Per-agent model tier configuration. Maps Economy/Reasoning to concrete model names.
/// Stored in config.toml under [agents.model_tiers].
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
#[ts(export)]
pub struct ModelTierConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub economy: Option<String>,
    /// User override for the `Default` tier — when set, takes precedence
    /// over the built-in fallback in `resolve_model_flag`. Lets the user
    /// pick e.g. their preferred Ollama model from the OllamaCard picker
    /// without having to edit config.toml. `None` = built-in default
    /// applies, preserving backward compatibility for users who never
    /// touched the setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// KT-1366 — the model Decision steps call (a decision model, not a chat
    /// model). Not a tier: no `ModelTier` resolves to it. Only Ollama sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// KT-646 — optional reasoning-effort preset for the `economy` slot.
    /// `None` means "pass no effort flag, the runtime's own CLI default
    /// applies" — never a silently assigned `high`/`max`. Only consumed for
    /// an agent `runner::agent_supports_reasoning_effort` returns true for;
    /// every other agent ignores it (see `runner::effective_reasoning_effort`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub economy_effort: Option<String>,
    /// Same as `economy_effort`, paired with the `default` tier slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// Same as `economy_effort`, paired with the `reasoning` tier slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// Global model tier overrides per agent.
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
#[ts(export)]
pub struct ModelTiersConfig {
    #[serde(default)]
    pub claude_code: ModelTierConfig,
    #[serde(default)]
    pub codex: ModelTierConfig,
    #[serde(default)]
    pub open_code: ModelTierConfig,
    #[serde(default)]
    pub gemini_cli: ModelTierConfig,
    #[serde(default)]
    pub kiro: ModelTierConfig,
    #[serde(default)]
    pub vibe: ModelTierConfig,
    #[serde(default)]
    pub copilot_cli: ModelTierConfig,
    #[serde(default)]
    pub ollama: ModelTierConfig,
    #[serde(default)]
    pub lite_llm: ModelTierConfig,
    #[serde(default)]
    pub nvidia: ModelTierConfig,
}

// ─── Setup wizard ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetupStatus {
    pub is_first_run: bool,
    pub current_step: SetupStep,
    pub agents_detected: Vec<AgentDetection>,
    pub scan_paths_set: bool,
    pub repos_detected: Vec<super::DetectedRepo>,
    pub default_scan_path: Option<String>,
    /// Paths actually scanned, exposed to diagnose empty results and missing mounts.
    pub scan_paths_explored: Vec<String>,
    /// config.toml could not be read at this start and was kept aside: the
    /// notice the wizard shows (where it is, key and data intact).
    pub config_set_aside: Option<String>,
}

/// Named server-visible root offered by the setup folder browser.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BrowseRoot {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BrowseEntry {
    pub name: String,
    pub path: String,
    /// This directory is itself a working copy — worth selecting as is.
    pub is_repository: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BrowseListing {
    /// The directory listed, as Kronn reaches it.
    pub path: String,
    /// Its parent, when the parent is still inside an allowed root.
    pub parent: Option<String>,
    /// Where browsing may start. Always sent, so the wizard can offer them
    /// again after a refusal.
    pub roots: Vec<BrowseRoot>,
    pub entries: Vec<BrowseEntry>,
    /// Subdirectories beyond the cap, not listed.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SetupStep {
    Agents,
    ScanPaths,
    Detection,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentDetection {
    pub name: String,
    pub agent_type: AgentType,
    pub installed: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub path: Option<String>,
    /// Resolved package-runner executable and argv when no installed CLI was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fallback_command: Option<Vec<String>>,
    pub version: Option<String>,
    pub latest_version: Option<String>,
    /// Time at which the official release source was last checked (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version_checked_at: Option<String>,
    /// Readable reason why the latest version is unknown or stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version_check_error: Option<String>,
    /// Official release endpoint consulted for this tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version_source_url: Option<String>,
    pub origin: String,
    pub install_command: Option<String>,
    #[serde(default)]
    pub host_managed: bool,
    #[serde(default)]
    pub host_label: Option<String>,
    /// Agent is runnable via npx/uvx fallback even when no local binary is found
    #[serde(default)]
    pub runtime_available: bool,
    /// Whether Kronn has the authentication material required by this runner.
    /// `None` is the backward-compatible wire value for older detection paths;
    /// API responses produced by the current backend always set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub auth_ready: Option<bool>,
    /// Local command that prepares authentication when `auth_ready == false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub auth_setup_command: Option<String>,
    /// `rtk` binary found on the host (PATH). Same value for every agent
    /// detection in a given sweep, but kept per-agent so the frontend can
    /// render the state inline without a separate endpoint.
    #[serde(default)]
    pub rtk_available: bool,
    /// The agent's own config file declares an RTK hook. Always `false` for
    /// agents that have no shell-exec (API-only agents like Vibe) or no
    /// hookable config (Ollama) — they're considered non-applicable.
    #[serde(default)]
    pub rtk_hook_configured: bool,
    /// Optional i18n key for a runtime-degradation warning the frontend
    /// should surface inline. Set per-agent at detect time.
    /// Examples:
    ///   - `"vibe.sdk_fallback"` — Vibe SDK signature mismatch detected
    ///     (sentinel file present); the runner falls back to direct API
    ///     mode, losing the local-tools (bash/file I/O) capability.
    ///   - `"vibe.project_config_untrusted"` — Vibe's workspace-trust store
    ///     rejects a `.vibe` directory Kronn manages, so it loads none of
    ///     the MCP servers Kronn wrote there.
    ///
    /// `None` means "no degradation detected, agent is healthy".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_warning: Option<String>,
    /// Other copies of this CLI on PATH, shadowed by `path`, whose version
    /// differs from the one Kronn runs. The CLI resolves model aliases itself,
    /// so a stale shadowing copy silently changes the model an alias serves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shadowed_installs: Option<Vec<ShadowedInstall>>,
}

/// A copy of an agent CLI that PATH order hides behind the one Kronn runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShadowedInstall {
    pub path: String,
    pub version: String,
}

fn default_true() -> bool {
    true
}

/// Readiness of one agent before a multi-agent launch (KT-1107).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentReadinessStatus {
    Ready,
    NotReady,
    /// Nothing failed, but a check could not be made without the model.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentReadinessReason {
    Ready,
    NotInstalled,
    FullAccessRequired,
    NotLoggedIn,
    /// The session opens; sign-in has no status command to ask.
    LoginUnverified,
    /// `initialize` or `session/new` did not answer within its bound.
    SessionTimeout,
    SessionFailed,
    /// HTTP model providers are not probed.
    NotProbed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentReadiness {
    pub agent_type: AgentType,
    pub status: AgentReadinessStatus,
    pub reason: AgentReadinessReason,
    /// The UI's i18n key for `reason`.
    pub message_key: String,
    /// Project MCP servers the session was starting when it stalled.
    #[serde(default)]
    pub servers: Vec<String>,
    /// The bound that elapsed, for a timeout.
    #[serde(default)]
    pub secs: Option<u64>,
    /// The runtime's own error, redacted and bounded.
    #[serde(default)]
    pub detail: Option<String>,
    pub cached: bool,
    pub checked_at: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct AgentReadinessRequest {
    #[serde(default)]
    pub project_id: Option<String>,
    pub agents: Vec<AgentType>,
    /// Ignore cached results.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub enum AgentType {
    /// 0.8.5 — picked as the serde default for `WorkflowStep.agent`. The
    /// field is required at runtime for agent-driven steps (Agent /
    /// BatchQuickPrompt) but irrelevant for non-LLM steps (ApiCall,
    /// Exec, Gate, …). Before this default the wizard had to invent a
    /// placeholder agent on every ApiCall step or the JSON payload
    /// failed to deserialize on `PUT /workflow-steps/test-api-call`
    /// with `missing field "agent"` (caught the user during the JIRA
    /// helper dogfooding on 2026-05-17). ClaudeCode is the safe pick
    /// because it's the only agent guaranteed to be installed by the
    /// onboarding flow.
    #[default]
    ClaudeCode,
    Codex,
    /// OpenCode uses the ACP transport; it is a CLI identity, never the
    /// generic `Custom` HTTP connection bucket.
    OpenCode,
    Vibe,
    GeminiCli,
    Kiro,
    CopilotCli,
    /// Local LLM via Ollama (0.4.0). Runs over the HTTP `/api/chat` path,
    /// not a CLI process. It has no host shell or arbitrary MCP bridge; Kronn
    /// exposes a bounded native workspace/Git/API catalogue server-side when
    /// the execution context provides the corresponding scope.
    Ollama,
    /// OpenAI-compatible proxy (LiteLLM). Same HTTP execution path as Ollama,
    /// different wire format (`OpenAiCodec`). "Installed" means the `litellm`
    /// binary is present; "reachable" means the proxy is actually running,
    /// which the health endpoint reports separately.
    LiteLlm,
    /// NVIDIA-hosted models (0.11.0). Same OpenAI-compatible HTTP path as
    /// LiteLLM — one API key serves the whole catalogue, so there is no local
    /// binary to install and "reachable" is the only meaningful health signal.
    /// The catalogue endpoint lists models the ACCOUNT may not be entitled to,
    /// so a model is only trusted once a real probe has answered.
    Nvidia,
    Custom,
}

// ─── Server / scan / agent-access settings requests ───────────────────────

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetScanPathsRequest {
    pub paths: Vec<String>,
}

/// One agent's access as a launch will really apply it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AgentEffectiveAccess {
    pub agent: AgentType,
    pub full_access: bool,
    /// Why it differs from the stored setting, when it does.
    #[ts(optional)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetAgentAccessRequest {
    pub agent: AgentType,
    pub full_access: bool,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetAgentConcurrencyRequest {
    pub agent: AgentType,
    /// `None` restores the family default: unlimited for a remote provider,
    /// 1 for Ollama, 5 for a CLI.
    pub concurrency: Option<u32>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetAgentToolsRequest {
    pub agent: AgentType,
    /// `None` clears the operator switch (tools on); see `AgentConfig::tools_enabled`.
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetAgentMentionColorRequest {
    pub agent: AgentType,
    /// `None` or an empty string restores the built-in color.
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ServerConfigPublic {
    pub host: String,
    pub port: u16,
    pub domain: Option<String>,
    pub max_concurrent_agents: usize,
    pub agent_stall_timeout_min: u32,
    pub agent_global_timeout_min: u32,
    pub local_agent_global_timeout_min: u32,
    pub auth_enabled: bool,
    pub pseudo: Option<String>,
    pub avatar_email: Option<String>,
    pub bio: Option<String>,
    pub debug_mode: bool,
    /// Whether discussion composers expose the out-of-context note control.
    pub discussion_notes_enabled: bool,
    /// 0.8.6 phase 4 — default model tier for new disc/QP/WF agent steps.
    /// Mirrored from `ServerConfig.default_model_tier` so the frontend
    /// can pre-fill the tier picker on creation forms without an extra
    /// round-trip. Strict semantic — never retroactive (see backing field
    /// rustdoc).
    pub default_model_tier: ModelTier,
    /// 0.8.6 phase 4 — default summary strategy for new discussions.
    /// `Off` by default in 0.8.6 onwards. UI surfaces an explanation of
    /// when to re-enable (small-context agents without MCP access).
    pub default_summary_strategy: crate::models::SummaryStrategy,
    pub agent_handoffs_enabled: bool,
    pub agent_handoff_paid_limit: u32,
    pub agent_handoff_paid_unlimited: bool,
    pub agent_handoff_blocked_agents: Vec<AgentType>,
    /// Sidebar storage-weight indicator: lets the frontend skip the batch
    /// call entirely when disabled, and grade colours without a round-trip.
    pub discussion_weight: crate::models::DiscussionWeightConfig,
    /// Default retention for encrypted execution-variable snapshots.
    /// Zero purges values as soon as the run reaches a terminal state.
    pub execution_variable_retention_days: u32,
    /// Days a finished workflow run keeps its step outputs. Zero keeps them.
    pub run_payload_retention_days: u32,
    /// KT-1318 - free GiB a worker launch / full audit needs (`0` = guard off).
    pub disk_launch_min_free_gib: u64,
    /// KT-1318 - hours a lane build dir must be idle before a sweep may take it.
    pub disk_sweep_idle_hours: u64,
    /// KT-1691 - days a Blocked/Interrupted execution's build dir must be idle
    /// before it becomes purgeable.
    pub disk_stalled_build_days: u64,
    pub p2p_enabled: bool,
    pub frontend_origins: Vec<String>,
    /// KT-1438 - copy lane seeds into task worktrees (default on).
    pub lane_seeds_copy_enabled: bool,
    /// KT-1438 - seed Kronn's secret-free suggestions for a project that declared none.
    pub lane_seeds_adopt_suggested: bool,
    /// KT-1438 - changing a project's lane seeds asks for the strong confirmation.
    pub lane_seeds_strong_confirm: bool,
    /// The zone set in Settings; `None` follows the machine.
    pub timezone: Option<String>,
    /// The zone in effect: `timezone`, else `timezone_detected`.
    pub timezone_effective: String,
    /// The machine's zone (`TZ`, then the OS setting, then UTC).
    pub timezone_detected: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateServerConfigRequest {
    pub domain: Option<String>,
    pub max_concurrent_agents: Option<usize>,
    pub agent_stall_timeout_min: Option<u64>,
    pub agent_global_timeout_min: Option<u64>,
    pub local_agent_global_timeout_min: Option<u64>,
    pub pseudo: Option<String>,
    pub avatar_email: Option<String>,
    pub bio: Option<String>,
    pub debug_mode: Option<bool>,
    #[serde(default)]
    pub discussion_notes_enabled: Option<bool>,
    /// 0.8.6 phase 4 — `Some(tier)` writes the new default ; `None`
    /// keeps the existing value (standard PATCH semantic across this
    /// struct).
    #[serde(default)]
    pub default_model_tier: Option<ModelTier>,
    /// 0.8.6 phase 4 — `Some(strategy)` writes the new default ;
    /// `None` keeps the existing value.
    #[serde(default)]
    pub default_summary_strategy: Option<crate::models::SummaryStrategy>,
    #[serde(default)]
    pub agent_handoffs_enabled: Option<bool>,
    #[serde(default)]
    pub agent_handoff_paid_limit: Option<u32>,
    #[serde(default)]
    pub agent_handoff_paid_unlimited: Option<bool>,
    #[serde(default)]
    pub agent_handoff_blocked_agents: Option<Vec<AgentType>>,
    /// Whole section at once: the toggle and the pair are persisted together
    /// so a rejected pair can never leave a half-applied state.
    pub discussion_weight: Option<crate::models::DiscussionWeightConfig>,
    /// Default encrypted execution-variable retention. Zero means that values
    /// exist only for the lifetime of the active execution.
    #[serde(default)]
    pub execution_variable_retention_days: Option<u32>,
    /// Days a finished workflow run keeps its step outputs; zero keeps them.
    #[serde(default)]
    pub run_payload_retention_days: Option<u32>,
    /// KT-1318 - `server.disk_launch_min_free_gib`; zero turns the guard off.
    #[serde(default)]
    pub disk_launch_min_free_gib: Option<u64>,
    /// KT-1318 - `server.disk_sweep_idle_hours`.
    #[serde(default)]
    pub disk_sweep_idle_hours: Option<u64>,
    /// KT-1691 - `server.disk_stalled_build_days`.
    #[serde(default)]
    pub disk_stalled_build_days: Option<u64>,
    /// KT-1438 - `server.lane_seeds_copy_enabled`.
    #[serde(default)]
    pub lane_seeds_copy_enabled: Option<bool>,
    /// KT-1438 - `server.lane_seeds_adopt_suggested`.
    #[serde(default)]
    pub lane_seeds_adopt_suggested: Option<bool>,
    /// KT-1438 - `server.lane_seeds_strong_confirm`.
    #[serde(default)]
    pub lane_seeds_strong_confirm: Option<bool>,
    #[serde(default)]
    pub p2p_enabled: Option<bool>,
    /// Replaces the whole list; every entry must be an exact origin.
    #[serde(default)]
    pub frontend_origins: Option<Vec<String>>,
    /// An IANA name, or an empty string to follow the machine's zone again.
    #[serde(default)]
    pub timezone: Option<String>,
}
