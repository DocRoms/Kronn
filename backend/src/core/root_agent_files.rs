//! Inject the Kronn-managed block into root-level agent context
//! files (`CLAUDE.md`, `.cursorrules`, `.windsurfrules`, `.clinerules`).
//!
//! Problem solved: before 0.8.3 the audit Phase 1 copied template
//! files only when they did NOT exist (`if src.exists() && !dst.exists()`).
//! A user with their own hand-curated `CLAUDE.md` got the template
//! skipped SILENTLY — meaning the agents that read `CLAUDE.md` first
//! never learned that Kronn put a structured `docs/AGENTS.md` in
//! place. The user's careful workflow rules survived (good) but
//! Kronn was invisible to the agent (bad).
//!
//! The fix:
//!   - File MISSING → write the template verbatim (existing behavior).
//!   - File EXISTS, no Kronn markers → **prepend** a small managed
//!     block (`<!-- KRONN-MANAGED-BLOCK:START/END -->`) that points
//!     to `docs/AGENTS.md`. User content stays untouched.
//!   - File EXISTS, markers present → re-render ONLY between the
//!     markers (idempotent on re-audit). User content around the
//!     block is preserved byte-identical.
//!
//! Data-safety contract:
//!   - User content NEVER lost (verified per-byte by tests).
//!   - Operations are atomic via tmp-file + rename so a crash mid-
//!     write doesn't truncate the user file.
//!   - Re-run on the same file is a no-op when the block content
//!     hasn't changed (no spurious writes).

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Agent-specific instruction families supported by the template bundle.
/// `AGENTS.md` is deliberately absent: it is the shared, vendor-neutral entry
/// point and is always installed, even when no agent adapter is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentAdapter {
    ClaudeCode,
    GeminiCli,
    Cursor,
    Windsurf,
    Cline,
    Copilot,
    Kiro,
    Vibe,
}

/// Translate a Kronn launch choice into the repository adapter that launch
/// explicitly configures. Codex consumes the shared `AGENTS.md`; API-only and
/// generic providers have no vendor-specific root template.
pub fn adapter_for_agent_type(agent: &crate::models::AgentType) -> Option<AgentAdapter> {
    match agent {
        crate::models::AgentType::ClaudeCode => Some(AgentAdapter::ClaudeCode),
        crate::models::AgentType::GeminiCli => Some(AgentAdapter::GeminiCli),
        crate::models::AgentType::Kiro => Some(AgentAdapter::Kiro),
        crate::models::AgentType::Vibe => Some(AgentAdapter::Vibe),
        crate::models::AgentType::CopilotCli => Some(AgentAdapter::Copilot),
        crate::models::AgentType::Codex
        | crate::models::AgentType::OpenCode
        | crate::models::AgentType::Ollama
        | crate::models::AgentType::LiteLlm
        | crate::models::AgentType::Nvidia
        | crate::models::AgentType::Custom => None,
    }
}

/// Result of a non-destructive instruction-file install.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AgentFileInstallReport {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub already_present: Vec<String>,
    pub failed: Vec<String>,
}

fn any_signal(project_root: &Path, signals: &[&str]) -> bool {
    signals
        .iter()
        .any(|relative| std::fs::symlink_metadata(project_root.join(relative)).is_ok())
}

/// Detect only adapters that the target repository already declares through
/// an agent instruction file/directory. Generic provider settings written by
/// Kronn itself (for example `.kiro/settings/mcp.json`) are intentionally not
/// signals: otherwise an MCP sync would manufacture a Kiro adapter in every
/// repository.
pub fn detect_agent_adapters(project_root: &Path) -> BTreeSet<AgentAdapter> {
    let mut detected = BTreeSet::new();
    for (adapter, signals) in [
        (
            AgentAdapter::ClaudeCode,
            &["CLAUDE.md", "CLAUDE.local.md", ".claude/CLAUDE.md"][..],
        ),
        (
            AgentAdapter::GeminiCli,
            &["GEMINI.md", ".gemini/GEMINI.md"][..],
        ),
        (AgentAdapter::Cursor, &[".cursorrules", ".cursor/rules"][..]),
        (AgentAdapter::Windsurf, &[".windsurfrules"][..]),
        (AgentAdapter::Cline, &[".clinerules"][..]),
        (
            AgentAdapter::Copilot,
            &[".github/copilot-instructions.md", ".github/instructions"][..],
        ),
        (AgentAdapter::Kiro, &[".kiro/steering"][..]),
        (
            AgentAdapter::Vibe,
            &[".vibe/instructions.md", ".vibe/AGENTS.md"][..],
        ),
    ] {
        if any_signal(project_root, signals) {
            detected.insert(adapter);
        }
    }
    detected
}

/// Canonical template paths for one immutable detection snapshot. Order is
/// stable so API receipts and tests stay deterministic.
pub fn desired_agent_template_paths(detected: &BTreeSet<AgentAdapter>) -> Vec<&'static str> {
    let mut paths = vec!["AGENTS.md"];
    for adapter in detected {
        match adapter {
            AgentAdapter::ClaudeCode => paths.push("CLAUDE.md"),
            AgentAdapter::GeminiCli => paths.push("GEMINI.md"),
            AgentAdapter::Cursor => {
                paths.push(".cursorrules");
                paths.push(".cursor/rules/repo-instructions.mdc");
            }
            AgentAdapter::Windsurf => paths.push(".windsurfrules"),
            AgentAdapter::Cline => paths.push(".clinerules"),
            AgentAdapter::Copilot => paths.push(".github/copilot-instructions.md"),
            AgentAdapter::Kiro => paths.push(".kiro/steering/instructions.md"),
            AgentAdapter::Vibe => paths.push(".vibe/instructions.md"),
        }
    }
    paths
}

/// Render a root instruction template with only facts Kronn can prove from
/// the repository. Unlike the docs skeleton, emitted adapter files must never
/// carry raw `{{...}}` placeholders: many agents load them before the audit
/// gets a chance to refine `docs/AGENTS.md`.
pub fn render_agent_template(project_root: &Path, template: &str) -> Result<String, String> {
    let mut rendered = template.to_string();
    for (token, value) in crate::core::docs_migration::compute_replacements(project_root) {
        rendered = rendered.replace(token, &value);
    }
    for (token, fallback) in [
        ("{{PROJECT_NAME}}", "Project"),
        (
            "{{STACK_SUMMARY}}",
            "See docs/AGENTS.md for the verified stack",
        ),
        (
            "{{TEST_CMD}}",
            "See docs/AGENTS.md for the verified test command",
        ),
        (
            "{{LINT_CMD}}",
            "See docs/AGENTS.md for the verified lint command",
        ),
        ("{{PROJECT_LANGUAGE}}", "English"),
        (
            "{{DO_NOT_1}}",
            "DO NOT bypass project-specific instructions in docs/AGENTS.md.",
        ),
        (
            "{{DO_NOT_2}}",
            "DO NOT change code before locating the repository's documented checks.",
        ),
    ] {
        rendered = rendered.replace(token, fallback);
    }
    rendered = rendered
        .replace(" [ex: \"cargo test && npm test\"]", "")
        .replace(" [ex: \"cargo clippy && npx tsc --noEmit\"]", "");
    if rendered.contains("{{") || rendered.contains("}}") {
        return Err("instruction template contains an unresolved placeholder".into());
    }
    Ok(rendered)
}

/// Upgrade a file that still has the exact structural markers of Kronn's old
/// instruction template. Only the template header/facts/critical-rules ranges
/// are rendered; bytes before the template header and from `## More context`
/// onward are preserved, including user-authored suffixes.
fn repair_generated_agent_file(project_root: &Path, path: &Path) -> Result<bool, String> {
    let original = std::fs::read_to_string(path)
        .map_err(|e| format!("read {} for upgrade: {e}", path.display()))?;
    let Some(facts_start) = original.find("<!-- KRONN:FACTS") else {
        return Ok(false);
    };
    let Some(facts_end_rel) = original[facts_start..].find("<!-- END KRONN:FACTS -->") else {
        return Ok(false);
    };
    let facts_end = facts_start + facts_end_rel + "<!-- END KRONN:FACTS -->".len();
    let Some(critical_rel) =
        original[facts_end..].find("## Critical rules (follow these BEFORE any action)")
    else {
        return Ok(false);
    };
    let critical_start = facts_end + critical_rel;
    let Some(more_rel) = original[critical_start..].find("## More context") else {
        return Ok(false);
    };
    let more_start = critical_start + more_rel;
    if !original[more_start..].contains("docs/AGENTS.md") {
        return Ok(false);
    }
    let header_start = original[..facts_start]
        .rfind("\n# ")
        .map(|index| index + 1)
        .unwrap_or(0);

    let mut updated = String::with_capacity(original.len());
    updated.push_str(&original[..header_start]);
    updated.push_str(&render_agent_template(
        project_root,
        &original[header_start..critical_start],
    )?);
    updated.push_str(&render_agent_template(
        project_root,
        &original[critical_start..more_start],
    )?);
    updated.push_str(&original[more_start..]);
    if updated == original {
        return Ok(false);
    }
    crate::core::fs_guard::assert_contained_no_symlink(project_root, path)?;
    atomic_write(path, updated.as_bytes())
        .map_err(|e| format!("upgrade {}: {e}", path.display()))?;
    Ok(true)
}

/// Install only the shared entry point plus adapters from the caller's
/// pre-write detection snapshot. Existing files are never overwritten and all
/// nested writes use the repository's no-follow guard.
pub fn install_detected_agent_files(
    project_root: &Path,
    template_root: &Path,
    detected: &BTreeSet<AgentAdapter>,
) -> AgentFileInstallReport {
    let mut report = AgentFileInstallReport::default();
    for relative in desired_agent_template_paths(detected) {
        let src = template_root.join(relative);
        let dst = project_root.join(relative);
        let template = match std::fs::read_to_string(&src) {
            Ok(value) => value,
            Err(e) => {
                report.failed.push(format!("{relative}: read failed: {e}"));
                continue;
            }
        };
        let rendered = match render_agent_template(project_root, &template) {
            Ok(value) => value,
            Err(e) => {
                report.failed.push(format!("{relative}: {e}"));
                continue;
            }
        };
        match crate::core::fs_guard::guarded_write_new(project_root, &dst, rendered.as_bytes()) {
            Ok(true) => report.created.push(relative.to_string()),
            Ok(false) => match repair_generated_agent_file(project_root, &dst) {
                Ok(true) => report.updated.push(relative.to_string()),
                Ok(false) => report.already_present.push(relative.to_string()),
                Err(e) => report.failed.push(format!("{relative}: {e}")),
            },
            Err(e) => report.failed.push(format!("{relative}: {e}")),
        }
    }
    report
}

pub fn created_agent_paths(project_root: &Path, report: &AgentFileInstallReport) -> Vec<PathBuf> {
    report
        .created
        .iter()
        .map(|relative| project_root.join(relative))
        .collect()
}

/// Marker that opens the managed block. The exact byte sequence is
/// part of the contract — if you change it, also bump
/// [`KRONN_BLOCK_END`] and add a backwards-compat detection branch
/// in [`inject_or_update`] so existing projects don't end up with
/// double-injected blocks.
pub const KRONN_BLOCK_START: &str = "<!-- KRONN-MANAGED-BLOCK:START -->";

/// Marker that closes the managed block. See [`KRONN_BLOCK_START`].
pub const KRONN_BLOCK_END: &str = "<!-- KRONN-MANAGED-BLOCK:END -->";

/// Body of the Kronn-managed block injected at the top of a user's
/// root agent file. Short on purpose — the goal is to point the
/// agent at `docs/AGENTS.md` without overshadowing the user's own
/// workflow rules below. The block is regenerated on every audit;
/// users who want to customize the message should edit
/// `docs/AGENTS.md` instead (the entry point Kronn manages fully).
const KRONN_BLOCK_BODY: &str = "> **Kronn project context** — Read [docs/AGENTS.md](docs/AGENTS.md) for the tiered context loader (load only what each task needs).\n> \n> Critical rules:\n> - All `docs/` files in English.\n> - Never hallucinate — say `NOT_FOUND` and ask the user when info is missing.\n> - Update `docs/` after learning something new.\n> \n> This block is auto-regenerated by Kronn on each audit. Your content below is preserved.";

/// Files Kronn injects the block into. Order matters: the audit
/// iterates this list once per Phase 1 install. The slice lives
/// here (not in `api/audit/full.rs`) so future helpers reading the
/// canonical set don't have to import the audit module.
pub const KRONN_ROOT_AGENT_FILES: &[&str] =
    &["CLAUDE.md", ".cursorrules", ".windsurfrules", ".clinerules"];

/// Outcome of an [`inject_or_update`] call. Surfaced for tests +
/// future SSE-event reporting if we ever want to tell users which
/// files received the block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectOutcome {
    /// File didn't exist, was created with the block at the top
    /// followed by the template default body.
    Created,
    /// File existed but had no Kronn markers — the block was
    /// **prepended** above the user's existing content, which was
    /// preserved verbatim.
    Prepended,
    /// File existed AND already had markers — the zone between them
    /// was re-rendered. Content outside the markers untouched.
    Updated,
    /// File existed, markers present, and the body inside them was
    /// already the latest. No filesystem write happened.
    Unchanged,
    /// File existed but couldn't be read (permission, IO error).
    /// The audit caller logs + continues — partial install is
    /// better than aborting the whole audit on one stuck file.
    SkippedIoError,
}

/// Build the full managed block as it appears on disk: opening
/// marker + body + closing marker + trailing blank line so the next
/// chunk of content starts cleanly.
fn render_block() -> String {
    format!(
        "{start}\n{body}\n{end}\n\n",
        start = KRONN_BLOCK_START,
        body = KRONN_BLOCK_BODY,
        end = KRONN_BLOCK_END,
    )
}

/// Find the marker zone `[start_idx, end_idx)` covering the full
/// block including its trailing newlines, so a re-render replaces
/// the EXACT range we wrote last time. Returns `None` when at least
/// one marker is missing (we treat that as "no block" rather than
/// guess where the missing marker should have been).
fn find_marker_zone(content: &str) -> Option<(usize, usize)> {
    let start_byte = content.find(KRONN_BLOCK_START)?;
    // Search for END only AFTER the START so a malformed file with
    // markers in the wrong order doesn't false-positive.
    let after_start = start_byte + KRONN_BLOCK_START.len();
    let end_rel = content[after_start..].find(KRONN_BLOCK_END)?;
    let end_byte = after_start + end_rel + KRONN_BLOCK_END.len();
    // Consume one optional trailing newline so re-render keeps the
    // spacing tight (would otherwise pile up blank lines on each run).
    let consume = if content.as_bytes().get(end_byte) == Some(&b'\n') {
        1
    } else {
        0
    };
    let consume2 = if content.as_bytes().get(end_byte + consume) == Some(&b'\n') {
        1
    } else {
        0
    };
    Some((start_byte, end_byte + consume + consume2))
}

/// Inject or update the Kronn block in `target_path`. If
/// `template_body` is `Some(...)`, it's used as the default content
/// when the file is missing entirely (so a fresh project still gets
/// the full template). When `None`, a missing file is created with
/// just the block.
///
/// Atomic writes via tmp-file + rename — a crash mid-write leaves
/// the user's original file intact rather than truncated.
pub fn inject_or_update(
    target_path: &Path,
    template_body: Option<&str>,
) -> std::io::Result<InjectOutcome> {
    let block = render_block();

    // Case 1: file missing → create it. If template provided, the
    // block goes ABOVE the template body (matches the prepend
    // semantics of the existing-file branch).
    if !target_path.exists() {
        let content = match template_body {
            Some(tpl) => format!("{block}{tpl}"),
            None => block,
        };
        atomic_write(target_path, content.as_bytes())?;
        return Ok(InjectOutcome::Created);
    }

    // Case 2: file exists. Read it once. Permission / encoding
    // errors surface as SkippedIoError so the caller can log +
    // continue (the whole audit shouldn't abort on one bad file).
    let existing = match fs::read_to_string(target_path) {
        Ok(s) => s,
        Err(_) => return Ok(InjectOutcome::SkippedIoError),
    };

    if let Some((start, end)) = find_marker_zone(&existing) {
        // Case 2a: markers found → re-render between them.
        let before = &existing[..start];
        let after = &existing[end..];
        let new_content = format!("{before}{block}{after}");
        if new_content == existing {
            return Ok(InjectOutcome::Unchanged);
        }
        atomic_write(target_path, new_content.as_bytes())?;
        Ok(InjectOutcome::Updated)
    } else {
        // Case 2b: no markers → prepend the block at the top.
        // User content stays byte-identical below.
        let new_content = format!("{block}{existing}");
        atomic_write(target_path, new_content.as_bytes())?;
        Ok(InjectOutcome::Prepended)
    }
}

/// Write `data` to `path` atomically via a tmp-file + rename in
/// the same directory. The rename is atomic on POSIX (same
/// filesystem); we leave the temp behind on failure so the
/// original file is never truncated mid-write.
fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("kronn-block");
    let tmp = parent.join(format!(".{file_name}.kronn.tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(p: &Path, content: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    #[test]
    fn creates_file_with_block_when_missing_no_template() {
        // Missing file + no template → file is created with just
        // the block. Common shape for `.cursorrules` / `.windsurfrules`
        // where Kronn doesn't ship a template body.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("CLAUDE.md");
        let outcome = inject_or_update(&target, None).unwrap();
        assert_eq!(outcome, InjectOutcome::Created);
        let body = fs::read_to_string(&target).unwrap();
        assert!(body.contains(KRONN_BLOCK_START));
        assert!(body.contains(KRONN_BLOCK_END));
        assert!(body.contains("docs/AGENTS.md"));
    }

    #[test]
    fn creates_file_with_block_above_template_when_missing_with_template() {
        // Missing file + template provided → block at top, then
        // the template body. The CLAUDE.md template that Kronn
        // ships has `{{PROJECT_NAME}}` etc., so the agent gets the
        // Kronn pointer first, then the canonical Kronn template.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("CLAUDE.md");
        let tpl = "# {{PROJECT_NAME}}\n\nUser rules here.";
        let outcome = inject_or_update(&target, Some(tpl)).unwrap();
        assert_eq!(outcome, InjectOutcome::Created);
        let body = fs::read_to_string(&target).unwrap();
        let block_idx = body.find(KRONN_BLOCK_START).unwrap();
        let tpl_idx = body.find("{{PROJECT_NAME}}").unwrap();
        assert!(
            block_idx < tpl_idx,
            "Kronn block must appear above the template content"
        );
    }

    #[test]
    fn prepends_block_when_file_exists_without_markers() {
        // CRITICAL: this is the killer bug we're fixing. A user
        // with their own CLAUDE.md gets the Kronn block injected
        // at the top, but their entire existing content must be
        // preserved byte-identical below.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("CLAUDE.md");
        let user_content =
            "# My personal rules\n\nDo not edit auto-generated SQL.\nLanguage: TypeScript only.\n";
        write(&target, user_content);
        let outcome = inject_or_update(&target, None).unwrap();
        assert_eq!(outcome, InjectOutcome::Prepended);
        let body = fs::read_to_string(&target).unwrap();
        // Block at top
        assert!(body.starts_with(KRONN_BLOCK_START));
        // User content present in full, byte-identical
        assert!(
            body.contains(user_content),
            "user content must be preserved byte-identical: got {body:?}"
        );
        // Block precedes user content
        let block_end = body.find(KRONN_BLOCK_END).unwrap();
        let user_start = body.find("# My personal rules").unwrap();
        assert!(block_end < user_start);
    }

    #[test]
    fn rerenders_block_in_place_when_markers_already_present() {
        // Re-audit on a file Kronn already touched. The zone
        // between markers gets re-rendered; everything around
        // stays where it was. Idempotent in terms of structure.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("CLAUDE.md");
        let stale_block = format!(
            "{start}\nOLD KRONN MESSAGE THAT MUST BE REPLACED\n{end}\n\n",
            start = KRONN_BLOCK_START,
            end = KRONN_BLOCK_END,
        );
        let user_content = "# My rules\n\nUse pnpm.\n";
        write(&target, &format!("{stale_block}{user_content}"));

        let outcome = inject_or_update(&target, None).unwrap();
        assert_eq!(outcome, InjectOutcome::Updated);
        let body = fs::read_to_string(&target).unwrap();
        // Old block body gone
        assert!(!body.contains("OLD KRONN MESSAGE"));
        // New block body present
        assert!(body.contains("docs/AGENTS.md"));
        // User content preserved
        assert!(body.contains(user_content));
    }

    #[test]
    fn second_run_is_a_no_op_when_block_already_current() {
        // Idempotency check: running inject_or_update twice in a
        // row must NOT emit a second write on the second call.
        // Detected via the `Unchanged` outcome variant.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("CLAUDE.md");
        write(&target, "user content");
        let first = inject_or_update(&target, None).unwrap();
        assert_eq!(first, InjectOutcome::Prepended);
        let mtime1 = fs::metadata(&target).unwrap().modified().unwrap();
        // Sleep a tiny bit so mtime would differ if a write happened
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = inject_or_update(&target, None).unwrap();
        assert_eq!(second, InjectOutcome::Unchanged);
        let mtime2 = fs::metadata(&target).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2, "second call must NOT rewrite the file");
    }

    #[test]
    fn unicode_emoji_and_user_content_below_block_preserved_byte_identical() {
        // Data-safety: a user with French + emoji content (real
        // case for the user reporting the bug) must end up with
        // their original bytes intact below the block.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("CLAUDE.md");
        let user = "# Règles du projet 📝\n\n- Toujours en français.\n- Tests avec émoji ✅ obligatoires.\n";
        write(&target, user);
        inject_or_update(&target, None).unwrap();
        let body = fs::read_to_string(&target).unwrap();
        assert!(
            body.contains(user),
            "unicode / emoji user content must survive verbatim"
        );
    }

    #[test]
    fn markers_at_end_of_file_still_detected() {
        // Edge: a user added their content ABOVE the markers
        // (which Kronn doesn't normally do, but defensive). The
        // block is still in-place re-rendered without duplicating.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("CLAUDE.md");
        let content = format!(
            "# User stuff at the top\n\nMore user stuff.\n\n{start}\nold body\n{end}\n",
            start = KRONN_BLOCK_START,
            end = KRONN_BLOCK_END,
        );
        write(&target, &content);
        let outcome = inject_or_update(&target, None).unwrap();
        assert_eq!(outcome, InjectOutcome::Updated);
        let body = fs::read_to_string(&target).unwrap();
        // User content preserved
        assert!(body.contains("# User stuff at the top"));
        assert!(body.contains("More user stuff."));
        // Exactly ONE START and ONE END marker — no double-render.
        assert_eq!(body.matches(KRONN_BLOCK_START).count(), 1);
        assert_eq!(body.matches(KRONN_BLOCK_END).count(), 1);
    }

    #[test]
    fn malformed_markers_treated_as_no_marker() {
        // Defensive: if a user (or a bad merge) leaves a stray
        // KRONN-MANAGED-BLOCK:START without an END, we treat it
        // as "no block" and prepend a fresh one. The original
        // content (including the malformed marker) stays at the
        // bottom — the user can clean up manually after the audit.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("CLAUDE.md");
        let content = format!(
            "{start}\nincomplete\n\nuser content\n",
            start = KRONN_BLOCK_START
        );
        write(&target, &content);
        let outcome = inject_or_update(&target, None).unwrap();
        assert_eq!(outcome, InjectOutcome::Prepended);
        let body = fs::read_to_string(&target).unwrap();
        // The newly-injected block lives at the top with BOTH markers.
        assert!(body.starts_with(KRONN_BLOCK_START));
        // The original malformed line still exists somewhere below.
        assert!(body.contains("incomplete"));
        assert!(body.contains("user content"));
    }

    #[test]
    fn atomic_write_does_not_leave_partial_file_on_success() {
        // Belt-and-suspenders: after a successful write the
        // temp file is gone (renamed away). Without this guard a
        // crashed test or audit-run would leave `.CLAUDE.md.kronn.tmp`
        // littering the repo.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("CLAUDE.md");
        inject_or_update(&target, None).unwrap();
        let tmp_file = tmp.path().join(".CLAUDE.md.kronn.tmp");
        assert!(
            !tmp_file.exists(),
            "temp file must be removed after successful rename"
        );
    }

    #[test]
    fn handles_empty_existing_file() {
        // `touch CLAUDE.md` before bootstrap → empty file. Must
        // not crash and the block must end up in there.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("CLAUDE.md");
        write(&target, "");
        let outcome = inject_or_update(&target, None).unwrap();
        assert_eq!(outcome, InjectOutcome::Prepended);
        let body = fs::read_to_string(&target).unwrap();
        assert!(body.contains(KRONN_BLOCK_START));
    }

    #[test]
    fn root_agent_files_list_covers_supported_agents() {
        // Lock the slice contents so a future "let's add Aider"
        // PR doesn't silently drop one of the existing names.
        // Mirror of the loop in `api/audit/full.rs::Phase 1`.
        assert!(KRONN_ROOT_AGENT_FILES.contains(&"CLAUDE.md"));
        assert!(KRONN_ROOT_AGENT_FILES.contains(&".cursorrules"));
        assert!(KRONN_ROOT_AGENT_FILES.contains(&".windsurfrules"));
        assert!(KRONN_ROOT_AGENT_FILES.contains(&".clinerules"));
    }

    #[test]
    fn user_content_never_lost_across_three_audit_runs() {
        // Stress: three consecutive injections. User content +
        // marker uniqueness must hold every time.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("CLAUDE.md");
        let user = "# My rules\n\nUse Tailwind v4.\nDo not touch /legacy.\n";
        write(&target, user);

        for _ in 0..3 {
            inject_or_update(&target, None).unwrap();
        }
        let body = fs::read_to_string(&target).unwrap();
        assert_eq!(
            body.matches(KRONN_BLOCK_START).count(),
            1,
            "no marker duplication across re-runs"
        );
        assert_eq!(
            body.matches(KRONN_BLOCK_END).count(),
            1,
            "no marker duplication across re-runs"
        );
        assert!(
            body.contains(user),
            "user content survives 3 audit runs verbatim"
        );
    }

    fn templates_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates")
    }

    #[test]
    fn zero_detected_agents_installs_only_shared_agents_entry() {
        let tmp = TempDir::new().unwrap();
        let detected = detect_agent_adapters(tmp.path());
        assert!(detected.is_empty());

        let report = install_detected_agent_files(tmp.path(), &templates_root(), &detected);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(report.created, vec!["AGENTS.md"]);

        for absent in [
            "CLAUDE.md",
            "GEMINI.md",
            ".cursorrules",
            ".cursor/rules/repo-instructions.mdc",
            ".windsurfrules",
            ".clinerules",
            ".github/copilot-instructions.md",
            ".kiro/steering/instructions.md",
            ".vibe/instructions.md",
        ] {
            assert!(!tmp.path().join(absent).exists(), "created absent {absent}");
        }
    }

    #[test]
    fn subset_detection_installs_only_matching_adapter_files() {
        let tmp = TempDir::new().unwrap();
        write(&tmp.path().join("CLAUDE.md"), "# User Claude rules\n");
        write(
            &tmp.path().join(".cursor/rules/existing.mdc"),
            "existing Cursor rule",
        );

        let detected = detect_agent_adapters(tmp.path());
        assert_eq!(
            detected,
            BTreeSet::from([AgentAdapter::ClaudeCode, AgentAdapter::Cursor])
        );
        let report = install_detected_agent_files(tmp.path(), &templates_root(), &detected);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(
            report.created,
            vec![
                "AGENTS.md",
                ".cursorrules",
                ".cursor/rules/repo-instructions.mdc"
            ]
        );
        assert!(report.already_present.contains(&"CLAUDE.md".to_string()));
        assert!(!tmp.path().join("GEMINI.md").exists());
        assert!(!tmp.path().join(".windsurfrules").exists());
        assert!(!tmp.path().join(".github/copilot-instructions.md").exists());

        for relative in report.created {
            let body = fs::read_to_string(tmp.path().join(relative)).unwrap();
            assert!(!body.contains("{{"), "raw placeholder in {body}");
            assert!(!body.contains("ai/index.md"), "legacy pointer in {body}");
            assert!(!body.contains("[ex:"), "generic command example in {body}");
        }
    }

    #[test]
    fn all_supported_agent_signals_map_to_the_complete_template_set() {
        let tmp = TempDir::new().unwrap();
        for signal in [
            "CLAUDE.md",
            "GEMINI.md",
            ".cursorrules",
            ".windsurfrules",
            ".clinerules",
            ".github/copilot-instructions.md",
            ".kiro/steering/existing.md",
            ".vibe/instructions.md",
        ] {
            write(&tmp.path().join(signal), "present");
        }
        let detected = detect_agent_adapters(tmp.path());
        assert_eq!(detected.len(), 8);
        assert_eq!(
            desired_agent_template_paths(&detected),
            vec![
                "AGENTS.md",
                "CLAUDE.md",
                "GEMINI.md",
                ".cursorrules",
                ".cursor/rules/repo-instructions.mdc",
                ".windsurfrules",
                ".clinerules",
                ".github/copilot-instructions.md",
                ".kiro/steering/instructions.md",
                ".vibe/instructions.md",
            ]
        );
    }

    #[test]
    fn configured_launch_agents_map_only_to_supported_repository_adapters() {
        assert_eq!(
            adapter_for_agent_type(&crate::models::AgentType::ClaudeCode),
            Some(AgentAdapter::ClaudeCode)
        );
        assert_eq!(
            adapter_for_agent_type(&crate::models::AgentType::GeminiCli),
            Some(AgentAdapter::GeminiCli)
        );
        assert_eq!(
            adapter_for_agent_type(&crate::models::AgentType::Kiro),
            Some(AgentAdapter::Kiro)
        );
        assert_eq!(
            adapter_for_agent_type(&crate::models::AgentType::Vibe),
            Some(AgentAdapter::Vibe)
        );
        assert_eq!(
            adapter_for_agent_type(&crate::models::AgentType::CopilotCli),
            Some(AgentAdapter::Copilot)
        );
        for shared_only in [
            crate::models::AgentType::Codex,
            crate::models::AgentType::Ollama,
            crate::models::AgentType::LiteLlm,
            crate::models::AgentType::Nvidia,
            crate::models::AgentType::Custom,
        ] {
            assert_eq!(adapter_for_agent_type(&shared_only), None);
        }
    }

    #[test]
    fn kronn_mcp_settings_alone_do_not_enable_kiro_adapter() {
        let tmp = TempDir::new().unwrap();
        write(
            &tmp.path().join(".kiro/settings/mcp.json"),
            r#"{"mcpServers":{}}"#,
        );
        let detected = detect_agent_adapters(tmp.path());
        assert!(!detected.contains(&AgentAdapter::Kiro));
        assert_eq!(desired_agent_template_paths(&detected), vec!["AGENTS.md"]);
    }

    #[test]
    fn rendered_adapter_uses_honest_fallbacks_without_placeholders() {
        let tmp = TempDir::new().unwrap();
        let template = fs::read_to_string(templates_root().join("CLAUDE.md")).unwrap();
        let rendered = render_agent_template(tmp.path(), &template).unwrap();
        assert!(!rendered.contains("{{"), "{rendered}");
        assert!(!rendered.contains("[ex:"), "{rendered}");
        assert!(rendered.contains("docs/AGENTS.md"));
        assert!(!rendered.contains("ai/index.md"));
    }

    #[test]
    fn complete_fixture_output_has_no_placeholder_legacy_path_or_example_command() {
        let tmp = TempDir::new().unwrap();
        let detected = BTreeSet::from([
            AgentAdapter::ClaudeCode,
            AgentAdapter::GeminiCli,
            AgentAdapter::Cursor,
            AgentAdapter::Windsurf,
            AgentAdapter::Cline,
            AgentAdapter::Copilot,
            AgentAdapter::Kiro,
            AgentAdapter::Vibe,
        ]);
        let report = install_detected_agent_files(tmp.path(), &templates_root(), &detected);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(report.created.len(), 10);
        for relative in report.created {
            let body = fs::read_to_string(tmp.path().join(&relative)).unwrap();
            assert!(!body.contains("{{"), "placeholder in {relative}: {body}");
            assert!(!body.contains("ai/"), "legacy path in {relative}: {body}");
            assert!(!body.contains("[ex:"), "example in {relative}: {body}");
        }
    }

    #[test]
    fn upgrade_repairs_only_recognized_template_ranges_and_preserves_user_bytes() {
        let tmp = TempDir::new().unwrap();
        let old_template = fs::read_to_string(templates_root().join("CLAUDE.md"))
            .unwrap()
            .replace(
                "Test: {{TEST_CMD}}",
                "Test: {{TEST_CMD}} [ex: \"cargo test && npm test\"]",
            )
            .replace(
                "Lint: {{LINT_CMD}}",
                "Lint: {{LINT_CMD}} [ex: \"cargo clippy && npx tsc --noEmit\"]",
            );
        let prefix = "<!-- user prefix stays -->\n";
        let suffix = "\n## User rules\nKeep {{USER_TOKEN}} byte-identical.\n";
        write(
            &tmp.path().join("CLAUDE.md"),
            &format!("{prefix}{old_template}{suffix}"),
        );

        let detected = detect_agent_adapters(tmp.path());
        let report = install_detected_agent_files(tmp.path(), &templates_root(), &detected);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(report.updated.contains(&"CLAUDE.md".to_string()));
        let body = fs::read_to_string(tmp.path().join("CLAUDE.md")).unwrap();
        assert!(body.starts_with(prefix));
        assert!(body.ends_with(suffix));
        assert!(!body.contains("{{DO_NOT_1}}"));
        assert!(!body.contains("{{TEST_CMD}}"));
        assert!(!body.contains("[ex:"));
        assert!(body.contains("{{USER_TOKEN}}"));
    }

    #[test]
    fn upgrade_does_not_rewrite_unmarked_user_file() {
        let tmp = TempDir::new().unwrap();
        let user = "# User template\nTest: {{TEST_CMD}}\n- {{DO_NOT_1}}\n";
        write(&tmp.path().join("CLAUDE.md"), user);
        let detected = detect_agent_adapters(tmp.path());
        let report = install_detected_agent_files(tmp.path(), &templates_root(), &detected);
        assert!(report.updated.is_empty());
        assert_eq!(
            fs::read_to_string(tmp.path().join("CLAUDE.md")).unwrap(),
            user
        );
    }
}
