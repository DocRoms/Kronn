//! Step 8 baseline checks that need no judgement (KT-152): Dockerfile,
//! compose, CI, tracked env files, templates and OSS community standards.
//!
//! Same contract as every detector in `audit_detectors`: a signal with a
//! `file:line` (or path / count) anchor, injected into Step 8, that the agent
//! must dispose of; `validation::check_detector_disposition` enforces it.
//! Reads are read-only, size-capped, never follow a symlink (the walk skips
//! them, root-level reads go through `fs_guard`) and the walk is count-capped.

use std::path::{Path, PathBuf};

use crate::core::audit_detectors::{
    is_test_path, walk, DetectedSignal, Severity, MARKUP_EXTS, MAX_PER_DETECTOR,
};

/// Files above this size are skipped: a baseline config is never that large.
const MAX_READ_BYTES: u64 = 512 * 1024;
/// Upper bound on walked files, so a huge monorepo cannot stall the audit.
const MAX_WALKED_FILES: usize = 50_000;
/// Inline `<script>`/`<style>` bodies longer than this are flagged.
const MAX_INLINE_LINES: usize = 50;

/// Baseline-relevant files, collected in one bounded walk.
#[derive(Debug, Default)]
pub struct Inventory {
    pub dockerfiles: Vec<PathBuf>,
    pub compose_files: Vec<PathBuf>,
    pub ci_files: Vec<PathBuf>,
    pub templates: Vec<PathBuf>,
}

impl Inventory {
    pub fn collect(root: &Path) -> Self {
        let mut inv = Inventory::default();
        for entry in walk(root).take(MAX_WALKED_FILES) {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(path);
            let rel_lower = rel.to_string_lossy().replace('\\', "/").to_lowercase();
            let name = rel_lower.rsplit('/').next().unwrap_or_default();
            if is_dockerfile_name(name) {
                inv.dockerfiles.push(path.to_path_buf());
            } else if is_compose_name(name) {
                inv.compose_files.push(path.to_path_buf());
            } else if is_ci_path(&rel_lower) {
                inv.ci_files.push(path.to_path_buf());
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if MARKUP_EXTS.contains(&ext) && !is_test_path(path) {
                inv.templates.push(path.to_path_buf());
            }
        }
        for list in [
            &mut inv.dockerfiles,
            &mut inv.compose_files,
            &mut inv.ci_files,
            &mut inv.templates,
        ] {
            list.sort();
        }
        inv
    }
}

fn is_dockerfile_name(name: &str) -> bool {
    name == "dockerfile"
        || name.starts_with("dockerfile.")
        || name.ends_with(".dockerfile")
        || name == "containerfile"
}

fn is_compose_name(name: &str) -> bool {
    let yaml = name.ends_with(".yml") || name.ends_with(".yaml");
    yaml && (name.starts_with("docker-compose") || name.starts_with("compose."))
}

fn is_ci_path(rel_lower: &str) -> bool {
    let yaml = rel_lower.ends_with(".yml") || rel_lower.ends_with(".yaml");
    (yaml && rel_lower.starts_with(".github/workflows/"))
        || (yaml && is_github_action_manifest(rel_lower))
        || matches!(
            rel_lower,
            ".gitlab-ci.yml"
                | "bitbucket-pipelines.yml"
                | ".circleci/config.yml"
                | "jenkinsfile"
                | ".drone.yml"
                | "azure-pipelines.yml"
        )
}

fn is_github_action_manifest(rel_lower: &str) -> bool {
    rel_lower.starts_with(".github/actions/")
        && (rel_lower.ends_with("/action.yml") || rel_lower.ends_with("/action.yaml"))
}

fn is_github_workflow(root: &Path, path: &Path) -> bool {
    let rel = rel_display(root, path).to_lowercase();
    rel.starts_with(".github/workflows/") || is_github_action_manifest(&rel)
}

/// Bounded read of a walked file. Walk entries are never symlinks.
fn read_walked(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_READ_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Bounded read of a root-relative file, refused when it resolves outside.
fn read_root_file(root: &Path, rel: &str) -> Option<String> {
    let resolved = crate::core::fs_guard::resolve_contained_read(root, Path::new(rel)).ok()?;
    read_walked(&resolved)
}

fn rel_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Pushes signals with the shared per-detector cap and a remainder summary.
struct Capped {
    id: &'static str,
    dimension: &'static str,
    severity: Severity,
    out: Vec<DetectedSignal>,
    truncated: usize,
}

impl Capped {
    fn new(id: &'static str, dimension: &'static str, severity: Severity) -> Self {
        Self {
            id,
            dimension,
            severity,
            out: Vec::new(),
            truncated: 0,
        }
    }

    fn push(&mut self, title: String, evidence: String) {
        self.push_with(self.severity, title, evidence);
    }

    fn push_with(&mut self, severity: Severity, title: String, evidence: String) {
        if self.out.len() >= MAX_PER_DETECTOR {
            self.truncated += 1;
            return;
        }
        self.out.push(DetectedSignal {
            detector_id: self.id,
            dimension: self.dimension,
            severity,
            title,
            evidence,
        });
    }

    fn finish(mut self) -> Vec<DetectedSignal> {
        if self.truncated > 0 {
            self.out.push(DetectedSignal {
                detector_id: self.id,
                dimension: self.dimension,
                severity: self.severity,
                title: format!("more `{}` hits not shown (cap reached)", self.id),
                evidence: format!(
                    "+{} additional hits beyond the first {MAX_PER_DETECTOR}",
                    self.truncated
                ),
            });
        }
        self.out
    }
}

/// Every baseline detector over a collected inventory.
pub fn run_baseline(root: &Path, inv: &Inventory) -> Vec<DetectedSignal> {
    let dockerfiles: Vec<(String, Vec<Instr>)> = inv
        .dockerfiles
        .iter()
        .filter_map(|p| read_walked(p).map(|c| (rel_display(root, p), dockerfile_instructions(&c))))
        .collect();
    let mut out = Vec::new();
    out.extend(detect_dockerfile_root_user(&dockerfiles));
    out.extend(detect_dockerfile_no_healthcheck(&dockerfiles));
    out.extend(detect_dockerfile_apt_hygiene(&dockerfiles));
    out.extend(detect_dockerfile_unpinned_base(&dockerfiles));
    out.extend(detect_dockerfile_add_remote(&dockerfiles));
    out.extend(detect_dockerfile_secret_arg(&dockerfiles));
    out.extend(detect_compose_unpinned_image(root, &inv.compose_files));
    out.extend(detect_ci_ssh_hostkey_off(root, &inv.ci_files));
    out.extend(detect_ci_pull_request_target(root, &inv.ci_files));
    out.extend(detect_ci_action_moving_ref(root, &inv.ci_files));
    out.extend(detect_img_missing_alt(root, &inv.templates));
    out.extend(detect_inline_block_oversize(root, &inv.templates));
    out.extend(detect_oss_community_standards(root));
    out
}

// ─── Dockerfile parsing ─────────────────────────────────────────────────

/// One logical Dockerfile instruction (continuations joined).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instr {
    pub line: usize,
    pub keyword: String,
    pub args: String,
}

/// Joins `\` continuations and drops comments. Heredoc bodies are not
/// interpreted (documented limit).
pub fn dockerfile_instructions(content: &str) -> Vec<Instr> {
    let mut out = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (idx, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with('#') || (current.is_none() && line.is_empty()) {
            continue;
        }
        let (continued, body) = match line.strip_suffix('\\') {
            Some(body) => (true, body.trim()),
            None => (false, line),
        };
        match current.as_mut() {
            Some((_, buf)) => {
                buf.push(' ');
                buf.push_str(body);
            }
            None => current = Some((idx + 1, body.to_string())),
        }
        if !continued {
            if let Some((line, text)) = current.take() {
                out.extend(parse_instr(line, &text));
            }
        }
    }
    if let Some((line, text)) = current.take() {
        out.extend(parse_instr(line, &text));
    }
    out
}

fn parse_instr(line: usize, text: &str) -> Option<Instr> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (keyword, args) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    Some(Instr {
        line,
        keyword: keyword.to_ascii_uppercase(),
        args: args.trim().to_string(),
    })
}

/// `FROM` parts: image reference and optional stage alias, flags skipped.
fn from_parts(args: &str) -> (Option<&str>, Option<&str>) {
    let tokens: Vec<&str> = args
        .split_whitespace()
        .filter(|t| !t.starts_with("--"))
        .collect();
    let image = tokens.first().copied();
    let alias = tokens
        .iter()
        .position(|t| t.eq_ignore_ascii_case("as"))
        .and_then(|i| tokens.get(i + 1).copied());
    (image, alias)
}

/// Why an image reference is not reproducible, if it is not.
fn unpinned_reason(image: &str) -> Option<&'static str> {
    if image.contains('$') || image.contains("@sha256:") {
        return None;
    }
    let last = image.rsplit('/').next().unwrap_or(image);
    match last.split_once(':') {
        None => Some("untagged (implicit `latest`)"),
        Some((_, tag)) if tag.eq_ignore_ascii_case("latest") => Some("tagged `latest`"),
        Some(_) => None,
    }
}

fn final_stage(instrs: &[Instr]) -> &[Instr] {
    let start = instrs
        .iter()
        .rposition(|i| i.keyword == "FROM")
        .unwrap_or(0);
    &instrs[start..]
}

// ─── Dockerfile detectors ───────────────────────────────────────────────

/// Final stage without `USER`, or with `USER root`/`0`: the container runs as root.
fn detect_dockerfile_root_user(files: &[(String, Vec<Instr>)]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("dockerfile-root-user", "Security", Severity::High);
    for (rel, instrs) in files {
        let stage = final_stage(instrs);
        let Some(from) = stage.first().filter(|i| i.keyword == "FROM") else {
            continue;
        };
        let image = from_parts(&from.args).0.unwrap_or_default().to_lowercase();
        // Distroless `:nonroot` images already switch to an unprivileged uid.
        if image.contains("nonroot") || image.contains("rootless") {
            continue;
        }
        match stage.iter().rev().find(|i| i.keyword == "USER") {
            None => sink.push(
                "Final Dockerfile stage has no `USER`: the container runs as root".into(),
                format!("{rel}:{}", from.line),
            ),
            Some(user) => {
                let name = user.args.split(':').next().unwrap_or_default().trim();
                if name.eq_ignore_ascii_case("root") || name == "0" {
                    sink.push(
                        "Final Dockerfile stage sets `USER root`".into(),
                        format!("{rel}:{}", user.line),
                    );
                }
            }
        }
    }
    sink.finish()
}

/// No `HEALTHCHECK` instruction (or only `HEALTHCHECK NONE`).
fn detect_dockerfile_no_healthcheck(files: &[(String, Vec<Instr>)]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new(
        "dockerfile-no-healthcheck",
        "Observability",
        Severity::Medium,
    );
    for (rel, instrs) in files {
        let has = instrs
            .iter()
            .any(|i| i.keyword == "HEALTHCHECK" && !i.args.eq_ignore_ascii_case("none"));
        if has {
            continue;
        }
        if let Some(from) = instrs.iter().rev().find(|i| i.keyword == "FROM") {
            sink.push(
                "Dockerfile has no `HEALTHCHECK`".into(),
                format!("{rel}:{}", from.line),
            );
        }
    }
    sink.finish()
}

/// `apt-get install` without `--no-install-recommends` or list cleanup.
fn detect_dockerfile_apt_hygiene(files: &[(String, Vec<Instr>)]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("dockerfile-apt-hygiene", "Maintainability", Severity::Low);
    for (rel, instrs) in files {
        for instr in instrs.iter().filter(|i| i.keyword == "RUN") {
            let args = instr.args.to_lowercase();
            if !(args.contains("apt-get install") || args.contains("apt install")) {
                continue;
            }
            let mut missing = Vec::new();
            if !args.contains("--no-install-recommends") {
                missing.push("`--no-install-recommends`");
            }
            // A cache mount on /var/lib/apt makes the cleanup unnecessary.
            if !args.contains("/var/lib/apt/lists") && !args.contains("target=/var/lib/apt") {
                missing.push("`rm -rf /var/lib/apt/lists/*`");
            }
            if !missing.is_empty() {
                sink.push(
                    format!("`apt-get install` without {}", missing.join(" and ")),
                    format!("{rel}:{}", instr.line),
                );
            }
        }
    }
    sink.finish()
}

/// Base image untagged or on `latest`, without a digest.
fn detect_dockerfile_unpinned_base(files: &[(String, Vec<Instr>)]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("dockerfile-unpinned-base", "Security", Severity::Medium);
    for (rel, instrs) in files {
        let mut stages: Vec<String> = Vec::new();
        for instr in instrs.iter().filter(|i| i.keyword == "FROM") {
            let (image, alias) = from_parts(&instr.args);
            if let Some(image) = image {
                let lower = image.to_lowercase();
                let is_stage = stages.contains(&lower);
                if lower != "scratch" && !is_stage {
                    if let Some(reason) = unpinned_reason(image) {
                        sink.push(
                            format!("Base image `{}` is {reason}", short(image)),
                            format!("{rel}:{}", instr.line),
                        );
                    }
                }
            }
            if let Some(alias) = alias {
                stages.push(alias.to_lowercase());
            }
        }
    }
    sink.finish()
}

/// `ADD <http(s) url>` without a BuildKit `--checksum`.
fn detect_dockerfile_add_remote(files: &[(String, Vec<Instr>)]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("dockerfile-add-remote", "Security", Severity::Medium);
    for (rel, instrs) in files {
        for instr in instrs.iter().filter(|i| i.keyword == "ADD") {
            let tokens: Vec<&str> = instr.args.split_whitespace().collect();
            if tokens.iter().any(|t| t.starts_with("--checksum=")) {
                continue;
            }
            let operands: Vec<&str> = tokens
                .into_iter()
                .filter(|t| !t.starts_with("--"))
                .collect();
            let sources = &operands[..operands.len().saturating_sub(1)];
            let remote = sources.iter().any(|s| {
                let s = s
                    .trim_matches(|c| c == '"' || c == '[' || c == ',')
                    .to_lowercase();
                s.starts_with("http://") || s.starts_with("https://")
            });
            if remote {
                sink.push(
                    "`ADD` fetches a remote URL without checksum verification".into(),
                    format!("{rel}:{}", instr.line),
                );
            }
        }
    }
    sink.finish()
}

static SECRET_NAME_PARTS: &[&str] = &[
    "SECRET",
    "TOKEN",
    "PASSWORD",
    "PASSWD",
    "API_KEY",
    "APIKEY",
    "PRIVATE_KEY",
    "ACCESS_KEY",
    "CREDENTIAL",
];

/// A secret-named build `ARG`: its value stays in `docker history`. Only the
/// name is reported, never the default value.
fn detect_dockerfile_secret_arg(files: &[(String, Vec<Instr>)]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("dockerfile-secret-arg", "Security", Severity::High);
    for (rel, instrs) in files {
        for instr in instrs.iter().filter(|i| i.keyword == "ARG") {
            let name: String = instr
                .args
                .split(['=', ' '])
                .next()
                .unwrap_or_default()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(64)
                .collect();
            let upper = name.to_ascii_uppercase();
            if upper.ends_with("_FILE") || upper.ends_with("_PATH") {
                continue;
            }
            if SECRET_NAME_PARTS.iter().any(|part| upper.contains(part)) {
                sink.push(
                    format!("Build `ARG {name}` looks like a secret (visible in `docker history`)"),
                    format!("{rel}:{}", instr.line),
                );
            }
        }
    }
    sink.finish()
}

fn short(value: &str) -> String {
    value.chars().take(80).collect()
}

// ─── Compose ────────────────────────────────────────────────────────────

/// Compose `image:` untagged or on `latest`, without a digest.
fn detect_compose_unpinned_image(root: &Path, files: &[PathBuf]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("compose-unpinned-image", "Security", Severity::Medium);
    for path in files {
        let Some(content) = read_walked(path) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            let Some(value) = line.trim().strip_prefix("image:") else {
                continue;
            };
            let value = value.split(" #").next().unwrap_or_default();
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            if value.is_empty() {
                continue;
            }
            if let Some(reason) = unpinned_reason(value) {
                sink.push(
                    format!("Compose image `{}` is {reason}", short(value)),
                    format!("{}:{}", rel_display(root, path), idx + 1),
                );
            }
        }
    }
    sink.finish()
}

// ─── CI ─────────────────────────────────────────────────────────────────

/// `StrictHostKeyChecking=no|off` in a CI file: host keys are not verified.
fn detect_ci_ssh_hostkey_off(root: &Path, files: &[PathBuf]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("ci-ssh-hostkey-off", "Security", Severity::High);
    for path in files {
        let Some(content) = read_walked(path) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if line_disables_host_key_check(line) {
                sink.push(
                    "CI disables SSH host key checking (`StrictHostKeyChecking=no`)".into(),
                    format!("{}:{}", rel_display(root, path), idx + 1),
                );
            }
        }
    }
    sink.finish()
}

fn line_disables_host_key_check(line: &str) -> bool {
    let lower = line.to_lowercase();
    let Some(pos) = lower.find("stricthostkeychecking") else {
        return false;
    };
    let value = lower[pos + "stricthostkeychecking".len()..]
        .trim_start_matches(|c: char| c == '=' || c == '"' || c == '\'' || c.is_whitespace());
    value.starts_with("no") || value.starts_with("off")
}

/// `pull_request_target` runs with secrets on fork PRs; High when the
/// workflow also references the PR head.
fn detect_ci_pull_request_target(root: &Path, files: &[PathBuf]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("ci-pull-request-target", "Security", Severity::Medium);
    for path in files.iter().filter(|p| is_github_workflow(root, p)) {
        let Some(content) = read_walked(path) else {
            continue;
        };
        let Some(idx) = content
            .lines()
            .position(|l| !l.trim_start().starts_with('#') && l.contains("pull_request_target"))
        else {
            continue;
        };
        let evidence = format!("{}:{}", rel_display(root, path), idx + 1);
        if content.contains("github.event.pull_request.head") || content.contains("github.head_ref")
        {
            sink.push_with(
                Severity::High,
                "`pull_request_target` workflow references the PR head (untrusted code with secrets)"
                    .into(),
                evidence,
            );
        } else {
            sink.push(
                "`pull_request_target` trigger: confirm only base-branch code runs".into(),
                evidence,
            );
        }
    }
    sink.finish()
}

static MOVING_REFS: &[&str] = &[
    "master", "main", "latest", "head", "develop", "dev", "trunk",
];

/// `uses:` on a moving branch or with no ref at all.
fn detect_ci_action_moving_ref(root: &Path, files: &[PathBuf]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("ci-action-moving-ref", "Security", Severity::Low);
    for path in files.iter().filter(|p| is_github_workflow(root, p)) {
        let Some(content) = read_walked(path) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            let trimmed = line.trim().trim_start_matches("- ").trim();
            let Some(value) = trimmed.strip_prefix("uses:") else {
                continue;
            };
            let value = value.split('#').next().unwrap_or_default();
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            if value.is_empty() || value.starts_with("./") || value.starts_with("docker://") {
                continue;
            }
            let reason = match value.rsplit_once('@') {
                None => Some("has no `@ref`".to_string()),
                Some((_, r)) if MOVING_REFS.contains(&r.to_lowercase().as_str()) => {
                    Some(format!("is pinned to the moving ref `{}`", short(r)))
                }
                Some(_) => None,
            };
            if let Some(reason) = reason {
                sink.push(
                    format!("Action `{}` {reason}", short(value)),
                    format!("{}:{}", rel_display(root, path), idx + 1),
                );
            }
        }
    }
    sink.finish()
}

// ─── Tracked env files ──────────────────────────────────────────────────

static ENV_TEMPLATE_SUFFIXES: &[&str] =
    &["dist", "example", "sample", "template", "tpl", "defaults"];

/// A tracked `.env` / `.env.<name>` / `<name>.env` that is not a template.
/// Filename only: the content is never opened.
pub fn detect_tracked_dotenv(listing: &[u8]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("tracked-dotenv", "Security", Severity::High);
    for raw in listing.split(|b| *b == 0) {
        let Ok(path) = std::str::from_utf8(raw) else {
            continue;
        };
        if is_tracked_env_file(path) {
            sink.push(
                "Tracked env file (only templates such as `.env.dist` belong in git)".into(),
                path.to_string(),
            );
        }
    }
    sink.finish()
}

fn is_tracked_env_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or_default().to_lowercase();
    let suffix = if name == ".env" {
        return true;
    } else if let Some(rest) = name.strip_prefix(".env.") {
        rest
    } else if let Some(stem) = name.strip_suffix(".env") {
        if stem.is_empty() {
            return false;
        }
        stem.rsplit('.').next().unwrap_or(stem)
    } else {
        return false;
    };
    !ENV_TEMPLATE_SUFFIXES
        .iter()
        .any(|t| suffix == *t || suffix.ends_with(&format!(".{t}")))
}

// ─── Templates ──────────────────────────────────────────────────────────

/// `<img>` without `alt`, on projects with more than 3 templates. A tag split
/// over several lines or spreading props is skipped, not guessed.
fn detect_img_missing_alt(root: &Path, templates: &[PathBuf]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("img-missing-alt", "Accessibility (a11y)", Severity::Low);
    if templates.len() <= 3 {
        return sink.finish();
    }
    for path in templates {
        let Some(content) = read_walked(path) else {
            continue;
        };
        if !content.contains("<img") {
            continue;
        }
        for (idx, line) in content.lines().enumerate() {
            if line_has_img_without_alt(line) {
                sink.push(
                    "`<img>` without an `alt` attribute".into(),
                    format!("{}:{}", rel_display(root, path), idx + 1),
                );
            }
        }
    }
    sink.finish()
}

fn line_has_img_without_alt(line: &str) -> bool {
    let lower = line.to_lowercase();
    let mut rest = lower.as_str();
    while let Some(pos) = rest.find("<img") {
        let after = &rest[pos + 4..];
        let boundary = after
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || c == '>' || c == '/');
        if boundary {
            let Some(end) = after.find('>') else {
                return false;
            };
            let tag = &after[..end];
            if !tag.contains("alt=") && !tag.contains("{...") {
                return true;
            }
        }
        rest = after;
    }
    false
}

/// Inline `<script>`/`<style>` body longer than 50 lines in a server template.
/// Vue/Svelte/JSX files are excluded: there those blocks are the component.
fn detect_inline_block_oversize(root: &Path, templates: &[PathBuf]) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("inline-block-oversize", "Security", Severity::Low);
    for path in templates {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        if !matches!(ext, "html" | "htm" | "twig" | "erb" | "blade" | "php") {
            continue;
        }
        let Some(content) = read_walked(path) else {
            continue;
        };
        for (line, kind, len) in oversized_inline_blocks(&content) {
            sink.push(
                format!("Inline `<{kind}>` block of {len} lines (CSP and caching cost)"),
                format!("{}:{line}", rel_display(root, path)),
            );
        }
    }
    sink.finish()
}

/// `(opening line, tag, body lines)` for each inline block over the limit.
fn oversized_inline_blocks(content: &str) -> Vec<(usize, &'static str, usize)> {
    let mut out = Vec::new();
    let mut open: Option<(usize, &'static str)> = None;
    for (idx, line) in content.lines().enumerate() {
        let lower = line.to_lowercase();
        match open {
            None => {
                for kind in ["script", "style"] {
                    let Some(pos) = lower.find(&format!("<{kind}")) else {
                        continue;
                    };
                    let tag_end = lower[pos..].find('>').map(|e| pos + e);
                    let tag = &lower[pos..tag_end.unwrap_or(lower.len())];
                    if kind == "script" && tag.contains("src=") {
                        continue;
                    }
                    if !lower[pos..].contains(&format!("</{kind}")) {
                        open = Some((idx, kind));
                    }
                    break;
                }
            }
            Some((start, kind)) => {
                if lower.contains(&format!("</{kind}")) {
                    let body = idx - start - 1;
                    if body > MAX_INLINE_LINES {
                        out.push((start + 1, kind, body));
                    }
                    open = None;
                }
            }
        }
    }
    out
}

// ─── OSS community standards ────────────────────────────────────────────

/// Anchor keyword, title and severity of each OSS community-standard item.
static OSS_ITEMS: &[(&str, &str, Severity)] = &[
    (
        "license",
        "No LICENSE file at repo root (OSS intent detected)",
        Severity::Medium,
    ),
    (
        "license",
        "LICENSE body is not a recognized license",
        Severity::Low,
    ),
    (
        "readme",
        "README has no description paragraph after its title",
        Severity::Low,
    ),
    (
        "issue_template",
        "No issue template (`.github/ISSUE_TEMPLATE/`)",
        Severity::Low,
    ),
    (
        "pull_request_template",
        "No pull request template (`pull_request_template.md`)",
        Severity::Low,
    ),
    ("contributing", "No CONTRIBUTING.md", Severity::Low),
    ("code_of_conduct", "No CODE_OF_CONDUCT.md", Severity::Low),
];

/// Disposition keyword for an `oss-community-standards` signal title.
pub fn oss_anchor(title: &str) -> Option<&'static str> {
    OSS_ITEMS
        .iter()
        .find(|(_, t, _)| *t == title)
        .map(|(kw, _, _)| *kw)
}

static PUBLIC_HOSTS: &[&str] = &["github.com", "gitlab.com", "codeberg.org", "git.sr.ht"];
static LICENSE_MARKERS: &[&str] = &[
    "mit license",
    "permission is hereby granted, free of charge",
    "apache license",
    "gnu general public license",
    "gnu lesser general public license",
    "gnu affero general public license",
    "redistribution and use in source and binary forms",
    "mozilla public license",
    "this is free and unencumbered software",
    "isc license",
];

/// Why the project shows OSS intent, or `None` (private projects are skipped).
fn oss_intent(root: &Path, license: Option<&str>) -> Option<String> {
    if let Some(name) = license {
        return Some(format!("`{name}` present"));
    }
    // Without its own `.git`, `git remote` would answer for an enclosing repo.
    let own_repo = std::fs::symlink_metadata(root.join(".git")).is_ok();
    let remote_output = if own_repo {
        crate::core::cmd::git_cmd()
            .args(["-C"])
            .arg(root)
            .args(["remote", "-v"])
            .output()
            .ok()
    } else {
        None
    };
    if let Some(output) = remote_output {
        let remotes = String::from_utf8_lossy(&output.stdout).to_lowercase();
        if let Some(host) = PUBLIC_HOSTS.iter().find(|h| remotes.contains(*h)) {
            return Some(format!("remote on {host}"));
        }
    }
    let readme = read_root_file(root, "README.md")?.to_lowercase();
    ["contribut", "open source", "open-source"]
        .iter()
        .any(|kw| readme.contains(kw))
        .then(|| "README invites contributions".to_string())
}

/// Case-insensitive lookup of a root-relative entry that never follows a
/// symlink (`DirEntry::file_type` is the entry's own type).
fn root_entry(root: &Path, rel: &str, want_dir: bool) -> Option<PathBuf> {
    let parts: Vec<&str> = rel.split('/').collect();
    let mut cur = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        let dir_expected = want_dir || i < parts.len() - 1;
        let found = std::fs::read_dir(&cur).ok()?.flatten().find(|e| {
            let kind_ok = e.file_type().is_ok_and(|t| {
                if dir_expected {
                    t.is_dir()
                } else {
                    t.is_file()
                }
            });
            kind_ok
                && e.file_name()
                    .to_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(part))
        })?;
        cur = found.path();
    }
    Some(cur)
}

fn root_has_file(root: &Path, rel: &str) -> bool {
    root_entry(root, rel, false).is_some()
}

fn first_root_file(root: &Path, names: &[&'static str]) -> Option<&'static str> {
    names.iter().copied().find(|n| root_has_file(root, n))
}

fn root_dir_has_files(root: &Path, rel: &str) -> bool {
    root_entry(root, rel, true)
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .is_some_and(|mut entries| entries.any(|e| e.is_ok()))
}

fn readme_is_title_only(readme: &str) -> bool {
    let mut lines = readme.lines().map(str::trim).filter(|l| !l.is_empty());
    if !lines.next().is_some_and(|l| l.starts_with('#')) {
        return false;
    }
    for line in lines {
        if line.starts_with("[![") || line.starts_with("![") || line.starts_with('<') {
            continue;
        }
        return line.starts_with('#');
    }
    true
}

fn detect_oss_community_standards(root: &Path) -> Vec<DetectedSignal> {
    let mut sink = Capped::new("oss-community-standards", "Maintainability", Severity::Low);
    let license = first_root_file(root, &["LICENSE", "LICENSE.md", "LICENSE.txt"]);
    let Some(intent) = oss_intent(root, license) else {
        return sink.finish();
    };
    let mut emit = |index: usize, evidence: String| {
        let (_, title, severity) = OSS_ITEMS[index];
        sink.push_with(
            severity,
            title.to_string(),
            format!("OSS intent: {intent}; {evidence}"),
        );
    };
    match license {
        None => emit(0, "no `LICENSE`, `LICENSE.md` or `LICENSE.txt`".into()),
        Some(name) => {
            let body = read_root_file(root, name)
                .unwrap_or_default()
                .to_lowercase();
            if !LICENSE_MARKERS.iter().any(|m| body.contains(m)) {
                emit(
                    1,
                    format!("`{name}` has no MIT/Apache/GPL/BSD/MPL/Unlicense/ISC text"),
                );
            }
        }
    }
    if let Some(readme) = read_root_file(root, "README.md") {
        if readme_is_title_only(&readme) {
            emit(
                2,
                "`README.md` goes from its title straight to a section".into(),
            );
        }
    }
    let has_issue_template = root_dir_has_files(root, ".github/ISSUE_TEMPLATE")
        || root_has_file(root, ".github/issue_template.md");
    if !has_issue_template {
        emit(
            3,
            "no `.github/ISSUE_TEMPLATE/` nor `.github/issue_template.md`".into(),
        );
    }
    let has_pr_template = [
        "pull_request_template.md",
        ".github/pull_request_template.md",
        "docs/pull_request_template.md",
    ]
    .iter()
    .any(|p| root_has_file(root, p))
        || root_dir_has_files(root, ".github/PULL_REQUEST_TEMPLATE");
    if !has_pr_template {
        emit(
            4,
            "no `pull_request_template.md` at root, `.github/` or `docs/`".into(),
        );
    }
    for (index, file) in [(5, "contributing.md"), (6, "code_of_conduct.md")] {
        let present = [
            file.to_string(),
            format!(".github/{file}"),
            format!("docs/{file}"),
        ]
        .iter()
        .any(|p| root_has_file(root, p));
        if !present {
            emit(
                index,
                format!(
                    "no `{}` at root, `.github/` or `docs/`",
                    file.to_uppercase().replace(".MD", ".md")
                ),
            );
        }
    }
    sink.finish()
}

#[cfg(test)]
#[path = "audit_baseline_test.rs"]
mod tests;
