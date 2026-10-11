//! Where a project keeps its AI documentation (KT-263).
//!
//! Precedence: the project's own override, then the global default from the
//! settings, then the historical detection order (`docs/` → `doc/` → `ai/`).
//! With nothing configured the result is exactly the historical one.
//!
//! A configured folder is also a write target (template install, kronn-state,
//! checksums), so it must stay inside the project root with no symlink on the
//! way: checked when it is saved and again every time it is resolved.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use serde::Serialize;
use ts_rs::TS;

/// Longest accepted setting, in bytes.
const MAX_SETTING_BYTES: usize = 255;

/// Historical candidates and the entry file that marks each as audited.
const LEGACY_CANDIDATES: &[(&str, &str)] = &[
    ("docs", "AGENTS.md"),
    ("doc", "AGENTS.md"),
    ("ai", "index.md"),
];

/// Returned when a configured folder fails containment. The NUL byte makes
/// every filesystem call on it fail, so nothing is read or written anywhere.
const REFUSED_SENTINEL: &str = "\0kronn-refused-docs-location";

/// Which rule picked the folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DocsDirSource {
    ProjectOverride,
    GlobalDefault,
    Detected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub dir: PathBuf,
    pub source: DocsDirSource,
    /// Why a configured folder was not used. With a refused project override
    /// `dir` is unusable on purpose; a refused global default is skipped.
    pub refused: Option<String>,
}

impl Resolution {
    /// The folder relative to `root`, `/`-separated; `None` when refused.
    pub fn relative(&self, root: &Path) -> Option<String> {
        if self.is_refused_override() {
            return None;
        }
        let rel = self.dir.strip_prefix(root).ok()?;
        Some(
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
        )
    }

    fn is_refused_override(&self) -> bool {
        self.source == DocsDirSource::ProjectOverride && self.refused.is_some()
    }
}

/// Normalize a user-supplied folder: relative, `/`-separated, no `.`/empty
/// segments, no `..`, not the root itself, never inside `.git`.
pub fn normalize_relative(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("the AI doc folder is empty".into());
    }
    if trimmed.len() > MAX_SETTING_BYTES {
        return Err(format!(
            "the AI doc folder is longer than {MAX_SETTING_BYTES} bytes"
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err("the AI doc folder contains a control character".into());
    }
    let unified = trimmed.replace('\\', "/");
    let bytes = unified.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if unified.starts_with('/') || drive || Path::new(&unified).is_absolute() {
        return Err(format!(
            "`{trimmed}` is absolute — the AI doc folder must be relative to the project root"
        ));
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in unified.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                return Err(format!(
                    "`{trimmed}` contains `..` — the AI doc folder must stay inside the project"
                ))
            }
            ".git" => {
                return Err(format!(
                    "`{trimmed}` points into `.git` — choose a folder of the working tree"
                ))
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return Err("the AI doc folder cannot be the project root itself".into());
    }
    Ok(parts.join("/"))
}

/// `root/rel` when it stays inside `root`: no symlink component (lstat walk)
/// and, canonicalized like `quick_exec::validate_cwd`, still under the root.
pub fn contained_dir(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = normalize_relative(rel)?;
    let target = root.join(&rel);
    crate::core::fs_guard::assert_contained_no_symlink(root, &target)
        .map_err(|reason| format!("AI doc folder `{rel}` refused: {reason}"))?;
    let real_root = root
        .canonicalize()
        .map_err(|e| format!("project folder {} cannot be resolved: {e}", root.display()))?;
    let mut probe = target.clone();
    while !probe.exists() {
        if !probe.pop() {
            break;
        }
    }
    let real = probe
        .canonicalize()
        .map_err(|e| format!("AI doc folder `{rel}` cannot be resolved: {e}"))?;
    if !real.starts_with(&real_root) {
        return Err(format!(
            "AI doc folder `{rel}` resolves outside the project ({})",
            real.display()
        ));
    }
    if target.exists() && !target.is_dir() {
        return Err(format!(
            "AI doc folder `{rel}` exists but is not a directory"
        ));
    }
    Ok(target)
}

/// Save-time check of a project override against the project's root.
pub fn validate_for_project(root: &Path, input: &str) -> Result<String, String> {
    let rel = normalize_relative(input)?;
    if !root.is_dir() {
        return Err(format!(
            "project folder {} not found — the AI doc folder cannot be checked",
            root.display()
        ));
    }
    contained_dir(root, &rel)?;
    Ok(rel)
}

fn entry_name(dir_rel: &str) -> &'static str {
    // Same rule as `scanner::detect_docs_entry`: only an `ai` folder keeps index.md.
    if dir_rel.rsplit('/').next() == Some("ai") {
        "index.md"
    } else {
        "AGENTS.md"
    }
}

/// Pure resolution, given the configuration explicitly.
pub fn resolve(
    root: &Path,
    project_override: Option<&str>,
    global_default: Option<&str>,
) -> Resolution {
    if let Some(rel) = project_override {
        return match contained_dir(root, rel) {
            Ok(dir) => Resolution {
                dir,
                source: DocsDirSource::ProjectOverride,
                refused: None,
            },
            Err(reason) => Resolution {
                dir: root.join(REFUSED_SENTINEL),
                source: DocsDirSource::ProjectOverride,
                refused: Some(reason),
            },
        };
    }

    let mut refused = None;
    let mut candidates: Vec<(String, &'static str)> = Vec::new();
    let mut default_dir = None;
    if let Some(rel) = global_default {
        match contained_dir(root, rel).and_then(|dir| normalize_relative(rel).map(|r| (dir, r))) {
            Ok((dir, rel)) => {
                candidates.push((rel.clone(), entry_name(&rel)));
                default_dir = Some(dir);
            }
            Err(reason) => refused = Some(reason),
        }
    }
    for (rel, entry) in LEGACY_CANDIDATES {
        if !candidates.iter().any(|(existing, _)| existing == rel) {
            candidates.push(((*rel).to_string(), entry));
        }
    }

    let pick = |dir: PathBuf| {
        let source = if default_dir.as_ref() == Some(&dir) {
            DocsDirSource::GlobalDefault
        } else {
            DocsDirSource::Detected
        };
        Resolution {
            dir,
            source,
            refused: refused.clone(),
        }
    };
    // An entry file is the strongest signal that an audit completed there.
    for (rel, entry) in &candidates {
        if root.join(rel).join(entry).is_file() {
            return pick(root.join(rel));
        }
    }
    // Then a folder that merely exists (half-finished bootstrap).
    for (rel, _) in &candidates {
        if root.join(rel).is_dir() {
            return pick(root.join(rel));
        }
    }
    pick(default_dir.clone().unwrap_or_else(|| root.join("docs")))
}

// ─── Process-wide configuration ─────────────────────────────────────────────
//
// Dozens of callers only hold a path, so the configuration lives here: the
// global default from the settings, and overrides keyed by project root.

static OVERRIDES: LazyLock<RwLock<HashMap<PathBuf, String>>> = LazyLock::new(Default::default);

#[cfg(not(test))]
static GLOBAL_DEFAULT: RwLock<Option<String>> = RwLock::new(None);

// Per test thread: a default set by one test must not move another test's
// fresh-project fallback.
#[cfg(test)]
thread_local! {
    static GLOBAL_DEFAULT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Arm the global default (startup and settings save). An invalid value is
/// ignored with a warning rather than half-applied.
pub fn set_global_default(value: Option<&str>) {
    let normalized = match value.map(normalize_relative).transpose() {
        Ok(v) => v,
        Err(reason) => {
            tracing::warn!("default AI doc folder ignored: {reason}");
            None
        }
    };
    #[cfg(not(test))]
    {
        *GLOBAL_DEFAULT.write().unwrap_or_else(|e| e.into_inner()) = normalized;
    }
    #[cfg(test)]
    GLOBAL_DEFAULT.with(|cell| *cell.borrow_mut() = normalized);
}

pub fn global_default() -> Option<String> {
    #[cfg(not(test))]
    {
        GLOBAL_DEFAULT
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    #[cfg(test)]
    {
        GLOBAL_DEFAULT.with(|cell| cell.borrow().clone())
    }
}

/// Every spelling a caller may hand us for one project: as stored, as
/// resolved for Docker, and both canonicalized.
fn keys_for(project_path: &str) -> Vec<PathBuf> {
    let raw = PathBuf::from(project_path);
    let resolved = crate::core::scanner::resolve_host_path(project_path);
    let mut keys = vec![raw.clone(), resolved.clone()];
    for path in [&raw, &resolved] {
        if let Ok(real) = path.canonicalize() {
            keys.push(real);
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

/// Load saved overrides at startup. Additive: a project removed or moved
/// later is re-keyed through `set_project_override`.
pub fn load_project_overrides<I>(rows: I)
where
    I: IntoIterator<Item = (String, String)>,
{
    for (project_path, dir) in rows {
        if normalize_relative(&dir).is_err() {
            tracing::warn!("AI doc folder of {project_path} ignored: invalid stored value");
            continue;
        }
        set_project_override(&project_path, Some(&dir));
    }
}

/// Set or clear one project's override after a save.
pub fn set_project_override(project_path: &str, dir: Option<&str>) {
    let keys = keys_for(project_path);
    let mut map = OVERRIDES.write().unwrap_or_else(|e| e.into_inner());
    for key in &keys {
        map.remove(key);
    }
    if let Some(dir) = dir.and_then(|d| normalize_relative(d).ok()) {
        for key in keys {
            map.insert(key, dir.clone());
        }
    }
}

/// `<project>` for a managed lane `<project>/.kronn/worktrees/<lane>`.
fn managed_worktree_owner(path: &Path) -> Option<&Path> {
    let lanes = path.parent()?;
    let kronn = lanes.parent()?;
    if lanes.file_name()? == "worktrees" && kronn.file_name()? == ".kronn" {
        kronn.parent()
    } else {
        None
    }
}

fn override_for(root: &Path) -> Option<String> {
    let map = OVERRIDES.read().unwrap_or_else(|e| e.into_inner());
    // No syscall at all for the common, unconfigured case.
    if map.is_empty() {
        return None;
    }
    let mut probes = vec![root.to_path_buf()];
    if let Ok(real) = root.canonicalize() {
        probes.push(real);
    }
    for probe in &probes {
        if let Some(dir) = map.get(probe) {
            return Some(dir.clone());
        }
        if let Some(dir) = managed_worktree_owner(probe).and_then(|owner| map.get(owner)) {
            return Some(dir.clone());
        }
    }
    None
}

/// Resolve `root` against the armed configuration.
pub fn resolution(root: &Path) -> Resolution {
    let project_override = override_for(root);
    let default = global_default();
    resolve(root, project_override.as_deref(), default.as_deref())
}

/// The project's AI doc folder. A refused override yields a path no
/// filesystem call can use, so nothing is read from or written outside.
pub fn docs_dir(root: &Path) -> PathBuf {
    let resolved = resolution(root);
    if let Some(reason) = &resolved.refused {
        tracing::debug!("AI doc location for {}: {reason}", root.display());
    }
    resolved.dir
}

/// For a flow about to write documentation: refuses a project override that
/// fails containment instead of writing anywhere else.
pub fn guard_write(root: &Path) -> Result<PathBuf, String> {
    let resolved = resolution(root);
    match resolved.refused {
        Some(reason) if resolved.source == DocsDirSource::ProjectOverride => Err(reason),
        _ => Ok(resolved.dir),
    }
}

// ─── The audit pipeline's root (KT-1192) ────────────────────────────────────

/// The folder every audit artefact derives from: step targets, TD sheets,
/// `decisions.md`, checksums, template install, redirectors and the paths the
/// prompts quote. Always relative to the project root, resolved per use.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocsRoot {
    rel: String,
}

/// Interned relocations of `'static` prompt text: one leak per (root, text),
/// bounded by the number of distinct roots times Kronn's own step strings.
static RELOCATED: LazyLock<std::sync::Mutex<HashMap<(String, &'static str), &'static str>>> =
    LazyLock::new(Default::default);

impl DocsRoot {
    pub const DEFAULT: &'static str = "docs";

    /// `docs/`, the root of every project without an override.
    pub fn default_root() -> Self {
        Self {
            rel: Self::DEFAULT.to_string(),
        }
    }

    /// A root from an already-normalized relative folder (tests, fixtures).
    pub fn from_rel(rel: &str) -> Result<Self, String> {
        Ok(Self {
            rel: normalize_relative(rel)?,
        })
    }

    /// The project's configured folder, else `docs/` (what the pipeline wrote
    /// before KT-1192). A configured folder failing containment is an error:
    /// the run is refused rather than written anywhere else.
    pub fn for_project(root: &Path) -> Result<Self, String> {
        let resolved = resolution(root);
        if let Some(reason) = resolved
            .refused
            .filter(|_| resolved.source == DocsDirSource::ProjectOverride)
        {
            return Err(reason);
        }
        if resolved.source == DocsDirSource::Detected {
            return Ok(Self::default_root());
        }
        let rel = Resolution {
            dir: resolved.dir,
            source: resolved.source,
            refused: None,
        }
        .relative(root)
        .ok_or_else(|| "the AI doc folder cannot be resolved under the project".to_string())?;
        Ok(Self { rel })
    }

    /// `for_project` for helpers deep in a run: the run's entry already
    /// refused a broken override, so a refusal here falls back inside the
    /// project rather than outside it.
    pub fn of(root: &Path) -> Self {
        Self::for_project(root).unwrap_or_default()
    }

    pub fn rel(&self) -> &str {
        &self.rel
    }

    pub fn is_default(&self) -> bool {
        self.rel == Self::DEFAULT
    }

    /// `<project>/<root>`.
    pub fn dir(&self, project: &Path) -> PathBuf {
        project.join(&self.rel)
    }

    /// `<root>/<sub>`, repository-relative.
    pub fn path(&self, sub: &str) -> String {
        format!("{}/{sub}", self.rel)
    }

    /// `<project>/<root>/<sub>`.
    pub fn file(&self, project: &Path, sub: &str) -> PathBuf {
        self.dir(project).join(sub)
    }

    /// Whether a repository-relative path lies under the root.
    pub fn contains(&self, rel_path: &str) -> bool {
        self.strip(rel_path).is_some()
    }

    /// `x` for `<root>/x`.
    pub fn strip<'a>(&self, rel_path: &'a str) -> Option<&'a str> {
        rel_path.strip_prefix(self.rel.as_str())?.strip_prefix('/')
    }

    /// Kronn's own text with every `docs/` path rewritten to this root. A
    /// `docs/` glued to a word or a path (`mydocs/`, `/repo/docs/`, a URL) is
    /// not a reference to the root and stays. Identity for `docs/`.
    pub fn relocate<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        if self.is_default() {
            return std::borrow::Cow::Borrowed(text);
        }
        let mut out = String::new();
        let mut last = 0;
        for (at, _) in text.match_indices("docs/") {
            let glued = text[..at].chars().next_back().is_some_and(|c| {
                c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '~' | '$' | '@' | ':')
            });
            if glued {
                continue;
            }
            out.push_str(&text[last..at]);
            out.push_str(&self.rel);
            out.push('/');
            last = at + "docs/".len();
        }
        if last == 0 {
            return std::borrow::Cow::Borrowed(text);
        }
        out.push_str(&text[last..]);
        std::borrow::Cow::Owned(out)
    }

    /// `relocate` for `'static` text, interned so step definitions keep their
    /// `&'static str` shape.
    pub fn relocate_static(&self, text: &'static str) -> &'static str {
        if self.is_default() {
            return text;
        }
        let key = (self.rel.clone(), text);
        let mut cache = RELOCATED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = cache.get(&key) {
            return found;
        }
        let relocated: &'static str = match self.relocate(text) {
            std::borrow::Cow::Borrowed(same) => same,
            std::borrow::Cow::Owned(owned) => Box::leak(owned.into_boxed_str()),
        };
        cache.insert(key, relocated);
        relocated
    }
}

/// The repository-relative name of a docs directory, for text that quotes
/// it (`.ai`, `packages/x/docs`): the project is the nearest ancestor whose
/// root resolves to `docs_dir`. `docs` when none does.
pub fn rel_label_for_dir(docs_dir: &Path) -> String {
    for project in docs_dir.ancestors().skip(1) {
        let docs_root = DocsRoot::of(project);
        if docs_root.dir(project) == docs_dir {
            return docs_root.rel().to_string();
        }
    }
    DocsRoot::DEFAULT.to_string()
}

impl Default for DocsRoot {
    fn default() -> Self {
        Self::default_root()
    }
}

impl std::fmt::Display for DocsRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn mkfile(root: &Path, rel: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "# entry\n").unwrap();
    }

    #[test]
    fn normalize_accepts_relative_folders_and_cleans_them() {
        assert_eq!(normalize_relative(".ai").unwrap(), ".ai");
        assert_eq!(normalize_relative(" ./context/ ").unwrap(), "context");
        assert_eq!(
            normalize_relative("packages\\x//docs/.").unwrap(),
            "packages/x/docs"
        );
    }

    #[test]
    fn normalize_refuses_absolute_parent_root_and_git() {
        for bad in [
            "/etc",
            "C:\\docs",
            "c:/docs",
            "../x",
            "a/../../b",
            "",
            "  ",
            ".",
            "./",
            ".git/x",
            "a\u{7}b",
        ] {
            assert!(normalize_relative(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(normalize_relative(&"a".repeat(300)).is_err());
    }

    #[test]
    fn unconfigured_resolution_is_the_historical_order() {
        let t = tmp();
        let root = t.path();
        let r = resolve(root, None, None);
        assert_eq!(
            (r.dir, r.source),
            (root.join("docs"), DocsDirSource::Detected)
        );

        std::fs::create_dir_all(root.join("ai")).unwrap();
        assert_eq!(resolve(root, None, None).dir, root.join("ai"));
        std::fs::create_dir_all(root.join("doc")).unwrap();
        assert_eq!(resolve(root, None, None).dir, root.join("doc"));
        mkfile(root, "ai/index.md");
        assert_eq!(resolve(root, None, None).dir, root.join("ai"));
        mkfile(root, "doc/AGENTS.md");
        assert_eq!(resolve(root, None, None).dir, root.join("doc"));
        mkfile(root, "docs/AGENTS.md");
        let r = resolve(root, None, None);
        assert_eq!(r.dir, root.join("docs"));
        assert_eq!(r.refused, None);
    }

    #[test]
    fn project_override_beats_global_default_and_detection() {
        let t = tmp();
        let root = t.path();
        mkfile(root, "docs/AGENTS.md");
        mkfile(root, "handbook/AGENTS.md");
        let r = resolve(root, Some(".ai"), Some("handbook"));
        assert_eq!(r.dir, root.join(".ai"));
        assert_eq!(r.source, DocsDirSource::ProjectOverride);
        assert_eq!(r.relative(root).as_deref(), Some(".ai"));
    }

    #[test]
    fn global_default_beats_detection_and_becomes_the_fresh_fallback() {
        let t = tmp();
        let root = t.path();
        let r = resolve(root, None, Some("handbook"));
        assert_eq!(
            (r.dir, r.source),
            (root.join("handbook"), DocsDirSource::GlobalDefault)
        );

        mkfile(root, "docs/AGENTS.md");
        mkfile(root, "handbook/AGENTS.md");
        assert_eq!(
            resolve(root, None, Some("handbook")).dir,
            root.join("handbook")
        );
    }

    #[test]
    fn global_default_does_not_hide_an_audited_legacy_folder() {
        let t = tmp();
        let root = t.path();
        mkfile(root, "ai/index.md");
        let r = resolve(root, None, Some("handbook"));
        assert_eq!(
            (r.dir, r.source),
            (root.join("ai"), DocsDirSource::Detected)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_override_is_refused_and_unusable() {
        let t = tmp();
        let outside = tmp();
        let root = t.path();
        std::os::unix::fs::symlink(outside.path(), root.join(".ai")).unwrap();
        let r = resolve(root, Some(".ai"), None);
        assert!(r.refused.as_deref().unwrap().contains("symlink"), "{r:?}");
        assert_eq!(r.relative(root), None);
        assert!(!r.dir.exists());
        assert!(std::fs::create_dir_all(r.dir.join("x")).is_err());
        assert!(std::fs::write(r.dir.join("AGENTS.md"), "x").is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_in_a_parent_component_is_refused_too() {
        let t = tmp();
        let outside = tmp();
        let root = t.path();
        std::os::unix::fs::symlink(outside.path(), root.join("packages")).unwrap();
        assert!(contained_dir(root, "packages/x/docs").is_err());
        assert!(validate_for_project(root, "packages/x/docs").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_global_default_is_skipped_for_the_historical_order() {
        let t = tmp();
        let outside = tmp();
        let root = t.path();
        std::os::unix::fs::symlink(outside.path(), root.join("handbook")).unwrap();
        mkfile(outside.path(), "AGENTS.md");
        let r = resolve(root, None, Some("handbook"));
        assert_eq!(
            (r.dir.clone(), r.source),
            (root.join("docs"), DocsDirSource::Detected)
        );
        assert!(r.refused.is_some());
    }

    #[test]
    fn validate_for_project_refuses_escapes_files_and_missing_roots() {
        let t = tmp();
        let root = t.path();
        assert_eq!(validate_for_project(root, "./.ai/").unwrap(), ".ai");
        assert!(validate_for_project(root, "../sibling").is_err());
        assert!(validate_for_project(root, "/abs").is_err());
        std::fs::write(root.join("notes"), "x").unwrap();
        assert!(validate_for_project(root, "notes").is_err());
        assert!(validate_for_project(&root.join("missing"), ".ai").is_err());
    }

    #[test]
    fn overrides_follow_the_project_and_its_managed_worktrees() {
        let t = tmp();
        let root = t.path().join("repo");
        let lane = root.join(".kronn/worktrees/lane-1");
        std::fs::create_dir_all(&lane).unwrap();
        let path = root.to_string_lossy().into_owned();
        set_project_override(&path, Some(".ai"));
        assert_eq!(docs_dir(&root), root.join(".ai"));
        assert_eq!(docs_dir(&lane), lane.join(".ai"));
        assert_eq!(resolution(&root).source, DocsDirSource::ProjectOverride);
        set_project_override(&path, None);
        assert_eq!(docs_dir(&root), root.join("docs"));
    }

    #[test]
    fn loading_overrides_skips_invalid_rows_and_keeps_the_others() {
        let t = tmp();
        let a = t.path().join("a");
        let b = t.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        load_project_overrides(vec![
            (a.to_string_lossy().into_owned(), "../escape".into()),
            (b.to_string_lossy().into_owned(), "./ctx/".into()),
        ]);
        assert_eq!(docs_dir(&a), a.join("docs"));
        assert_eq!(docs_dir(&b), b.join("ctx"));
        set_project_override(&b.to_string_lossy(), None);
    }

    #[test]
    fn the_armed_global_default_is_read_by_the_resolver() {
        let t = tmp();
        let root = t.path();
        set_global_default(Some("./handbook/"));
        assert_eq!(global_default().as_deref(), Some("handbook"));
        assert_eq!(docs_dir(root), root.join("handbook"));
        set_global_default(Some("../escape"));
        assert_eq!(global_default(), None);
        set_global_default(None);
        assert_eq!(docs_dir(root), root.join("docs"));
    }

    #[test]
    fn the_pipeline_root_follows_the_override_and_defaults_to_docs() {
        let t = tmp();
        let root = t.path().join("repo");
        std::fs::create_dir_all(root.join("ai")).unwrap();
        std::fs::write(root.join("ai/index.md"), "# legacy\n").unwrap();
        let path = root.to_string_lossy().into_owned();
        // A detected legacy layout keeps the historical `docs/` pipeline.
        assert_eq!(
            DocsRoot::for_project(&root).unwrap(),
            DocsRoot::default_root()
        );

        set_project_override(&path, Some(".ai"));
        let docs_root = DocsRoot::for_project(&root).unwrap();
        set_project_override(&path, None);
        assert_eq!(docs_root.rel(), ".ai");
        assert_eq!(docs_root.dir(&root), root.join(".ai"));
        assert_eq!(docs_root.path("tech-debt/TD-1.md"), ".ai/tech-debt/TD-1.md");
        assert_eq!(docs_root.strip(".ai/decisions.md"), Some("decisions.md"));
        assert!(!docs_root.contains(".aix/decisions.md"));
        assert!(!docs_root.contains("docs/decisions.md"));
    }

    #[test]
    fn relocation_rewrites_root_references_only() {
        let ai = DocsRoot::from_rel(".ai").unwrap();
        assert_eq!(
            ai.relocate("Read `docs/AGENTS.md`, then [x](docs/x.md) and docs/tech-debt/."),
            "Read `.ai/AGENTS.md`, then [x](.ai/x.md) and .ai/tech-debt/."
        );
        for kept in [
            "https://github.com/o/r/blob/main/docs/x.md",
            "<repo-path>/docs/AGENTS.md",
            "mydocs/x",
            "../docs/x",
            "api-docs/x",
        ] {
            assert_eq!(ai.relocate(kept), kept, "{kept}");
        }
        let docs = DocsRoot::default_root();
        let text = "Read `docs/AGENTS.md`.";
        assert!(
            matches!(docs.relocate(text), std::borrow::Cow::Borrowed(t) if std::ptr::eq(t, text))
        );
        assert!(std::ptr::eq(docs.relocate_static("docs/a"), "docs/a"));
        let interned = ai.relocate_static("docs/AGENTS.md");
        assert_eq!(interned, ".ai/AGENTS.md");
        assert!(std::ptr::eq(interned, ai.relocate_static("docs/AGENTS.md")));
    }

    #[cfg(unix)]
    #[test]
    fn a_refused_override_refuses_the_pipeline_root() {
        let t = tmp();
        let outside = tmp();
        let root = t.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().into_owned();
        std::os::unix::fs::symlink(outside.path(), root.join(".ai")).unwrap();
        set_project_override(&path, Some(".ai"));
        let refused = DocsRoot::for_project(&root);
        set_project_override(&path, None);
        assert!(refused.unwrap_err().contains("symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn guard_write_fails_closed_on_a_symlink_created_after_save() {
        let t = tmp();
        let outside = tmp();
        let root = t.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().into_owned();
        set_project_override(&path, Some(".ai"));
        std::os::unix::fs::symlink(outside.path(), root.join(".ai")).unwrap();
        assert!(guard_write(&root).unwrap_err().contains("symlink"));
        set_project_override(&path, None);
    }
}
