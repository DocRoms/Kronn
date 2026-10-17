//! KT-1211 — a cancelled, killed or failed audit step never leaves an unmarked
//! half-written doc.
//!
//! The agent writes its target in place. When Kronn itself stops the step
//! (cancel, a crash found at the next launch) the write is untrusted and the
//! target is put back as it was before the step ([`restore_target`]). When the
//! step fails after the agent finished or died by itself, what it wrote stays
//! — a human may accept it as it stands — but it carries a visible partial
//! marker ([`mark_partial`]) until a step succeeds on it or a human accepts.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::validation::{target_snapshot, TargetSnapshot};
use crate::core::fs_guard::{assert_contained_no_symlink, guarded_create_dir_all};

pub(crate) const PARTIAL_OPEN: &str = "<!-- kronn:partial-step -->";
pub(crate) const PARTIAL_CLOSE: &str = "<!-- kronn:partial-step:end -->";

/// `target_state` of a step whose target was put back.
pub(crate) const STATE_RESTORED: &str = "restored";
/// `target_state` of a step whose target was kept and marked.
pub(crate) const STATE_PARTIAL: &str = "partial";

/// Longest reason kept inside the marker, in characters.
const MAX_MARKER_REASON_CHARS: usize = 240;

/// The marker block a failed step's kept target starts with. One line of
/// reason, never the agent's text.
pub(crate) fn partial_banner(step: u32, reason: &str) -> String {
    let one_line = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    let bounded: String = one_line.chars().take(MAX_MARKER_REASON_CHARS).collect();
    let bounded = bounded.replace("-->", "—>");
    format!(
        "{PARTIAL_OPEN}\n> ⚠️ **Partial document** — audit step {step} did not complete ({bounded}). \
Kronn kept what the agent wrote: retry the step, or accept it as it stands.\n{PARTIAL_CLOSE}\n\n"
    )
}

/// Whether `content` starts with a partial marker.
#[cfg(test)]
pub(crate) fn has_partial_marker(content: &str) -> bool {
    content.trim_start().starts_with(PARTIAL_OPEN)
}

/// `content` without its partial marker; `None` when it has none.
pub(crate) fn strip_partial_marker(content: &str) -> Option<String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with(PARTIAL_OPEN) {
        return None;
    }
    let after = trimmed
        .find(PARTIAL_CLOSE)
        .map(|at| &trimmed[at + PARTIAL_CLOSE.len()..])?;
    Some(after.trim_start_matches(['\n', '\r']).to_string())
}

/// The path of `target` inside the project, refused when it leaves the
/// project or passes through a symlink (an agent can plant one: a write
/// through it would land outside the repository), or when the project root is
/// gone (an unmounted volume must not be recreated by a restore).
fn contained_path(project_path: &Path, target: &str) -> Result<PathBuf, String> {
    if !project_path.is_dir() {
        return Err(format!(
            "project root {} does not exist: `{target}` left untouched",
            project_path.display()
        ));
    }
    let path = project_path.join(target);
    assert_contained_no_symlink(project_path, &path)
        .map_err(|e| format!("target `{target}` refused: {e}"))?;
    Ok(path)
}

/// The target as it is now: `None` when the file is absent.
pub(crate) fn read_target(project_path: &Path, target: &str) -> Result<Option<String>, String> {
    match std::fs::read_to_string(contained_path(project_path, target)?) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("target `{target}` unreadable: {e}")),
    }
}

/// A sibling of `path` that only this call can have created: never a name an
/// agent could have planted.
fn sibling(path: &Path, tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(
        ".{name}.{tag}-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ))
}

/// Create `path` with `content`, failing when anything is already there and
/// never following a symlink at the final component.
fn create_new_file(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()
}

/// Replace `path` (already checked by [`contained_path`]) with `content`
/// atomically, through a fresh exclusive temp sibling.
fn write_atomically(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = sibling(path, "kronn-restore");
    create_new_file(&tmp, content)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Put `target` back as it was before the step: `pre` is its content then,
/// `None` when it was absent. Returns whether anything changed on disk.
pub(crate) fn restore_target(
    project_path: &Path,
    target: &str,
    pre: Option<&str>,
) -> Result<bool, String> {
    let path = contained_path(project_path, target)?;
    let now = read_target(project_path, target)?;
    if now.as_deref() == pre {
        return Ok(false);
    }
    match pre {
        Some(content) => {
            if let Some(parent) = path.parent() {
                guarded_create_dir_all(project_path, parent)
                    .map_err(|e| format!("could not restore `{target}`: {e}"))?;
            }
            write_atomically(&path, content)
                .map_err(|e| format!("could not restore `{target}`: {e}"))?;
        }
        None => std::fs::remove_file(&path)
            .map_err(|e| format!("could not remove the half-written `{target}`: {e}"))?,
    }
    Ok(true)
}

/// Keep `content` — about to be displaced from `target` — next to it, under a
/// name nothing else can hold. Returns the project-relative path it was kept
/// at.
fn save_aside(project_path: &Path, target: &str, content: &str) -> Result<String, String> {
    let path = contained_path(project_path, target)?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for attempt in 0..100u32 {
        let suffix = if attempt == 0 {
            String::new()
        } else {
            format!("-{attempt}")
        };
        let aside = path.with_file_name(format!("{name}.kronn-displaced-{stamp}{suffix}"));
        match create_new_file(&aside, content) {
            Ok(()) => {
                let shown = aside.strip_prefix(project_path).unwrap_or(&aside);
                return Ok(shown.to_string_lossy().into_owned());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "could not keep the content of `{target}` aside: {e}"
                ))
            }
        }
    }
    Err(format!(
        "could not find a free name to keep the content of `{target}` aside"
    ))
}

/// Mark the kept target of a failed step as partial. Returns whether a
/// marker was written (`false`: nothing to mark — the file is absent or empty).
pub(crate) fn mark_partial(
    project_path: &Path,
    target: &str,
    step: u32,
    reason: &str,
) -> Result<bool, String> {
    let Some(content) = read_target(project_path, target)? else {
        return Ok(false);
    };
    let body = strip_partial_marker(&content).unwrap_or(content);
    if body.trim().is_empty() {
        return Ok(false);
    }
    let marked = format!("{}{}", partial_banner(step, reason), body);
    write_atomically(&contained_path(project_path, target)?, &marked)
        .map_err(|e| format!("could not mark `{target}` as partial: {e}"))?;
    Ok(true)
}

/// Remove the partial marker of `target`, when it has one.
pub(crate) fn clear_partial_marker(project_path: &Path, target: &str) -> Result<bool, String> {
    let Some(content) = read_target(project_path, target)? else {
        return Ok(false);
    };
    let Some(clean) = strip_partial_marker(&content) else {
        return Ok(false);
    };
    write_atomically(&contained_path(project_path, target)?, &clean)
        .map_err(|e| format!("could not clear the partial marker of `{target}`: {e}"))?;
    Ok(true)
}

/// A step ended without success and Kronn did not kill it: when its target
/// differs from what it was before the step, keep it and mark it partial.
/// `pre` is the snapshot taken before the agent ran. Returns the
/// `target_state` to record, `None` when the step left its target alone.
pub(crate) fn settle_failed_target(
    project_path: &Path,
    target: &str,
    pre: &TargetSnapshot,
    step: u32,
    reason: &str,
) -> Result<Option<&'static str>, String> {
    if target == "REVIEW" || target.is_empty() {
        return Ok(None);
    }
    match target_snapshot(project_path, target)? {
        TargetSnapshot::Missing => Ok(None),
        now if &now == pre => Ok(None),
        _ => Ok(mark_partial(project_path, target, step, reason)?.then_some(STATE_PARTIAL)),
    }
}

/// What became of the target of a step Kronn stopped (cancel, or an agent cut
/// off with its backend). The cases are told apart on purpose: "the step wrote
/// nothing" must never absorb "the target could not be settled".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KilledOutcome {
    /// The half-written target was put back as it was before the step.
    Restored,
    /// The step had written nothing: the target is whole as it stood.
    Untouched,
    /// It could not be put back; what the agent wrote is kept under a marker.
    Partial,
    /// What is on disk is not what Kronn last saw the agent write — a hand
    /// edit, or a write after the backend stopped watching. Left in place
    /// under a marker rather than overwritten.
    ChangedSinceSeen,
    /// It could neither be put back nor marked: the doc is in an unknown state.
    Unsettled,
}

/// How the step came to be stopped: it decides whether content nobody vouches
/// for may be replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopKind {
    /// Kronn killed the agent a moment ago (cancel): what is on disk is the
    /// agent's. Unvouched content is still kept aside before it is replaced.
    Cancelled,
    /// The backend died and the agent was found cut off at boot: nothing
    /// watched the file since the last observation, so a hand edit cannot be
    /// told from the agent's own late write. Unvouched content is never
    /// replaced.
    CutOff,
}

/// A stopped step's settlement: what became of the target, and where the
/// content that was displaced was kept, when it was not the agent's known
/// output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KilledSettlement {
    pub outcome: KilledOutcome,
    /// Project-relative path of the copy kept aside.
    pub displaced: Option<String>,
}

impl KilledSettlement {
    fn of(outcome: KilledOutcome) -> Self {
        Self {
            outcome,
            displaced: None,
        }
    }

    /// The `target_state` to record. `None` for [`KilledOutcome::Unsettled`]:
    /// the step is then neither restored nor visibly partial, so acceptance
    /// asks for an override.
    pub(crate) fn target_state(&self) -> Option<&'static str> {
        match self.outcome {
            KilledOutcome::Restored | KilledOutcome::Untouched => Some(STATE_RESTORED),
            KilledOutcome::Partial | KilledOutcome::ChangedSinceSeen => Some(STATE_PARTIAL),
            KilledOutcome::Unsettled => None,
        }
    }

    /// The target is whole: nothing a human has to look at.
    pub(crate) fn is_whole(&self) -> bool {
        matches!(
            self.outcome,
            KilledOutcome::Restored | KilledOutcome::Untouched
        )
    }

    /// A target left unsound keeps its snapshot for the acceptance diff.
    pub(crate) fn keeps_snapshot(&self) -> bool {
        !self.is_whole()
    }
}

/// Fingerprint of a target's content as recorded by [`TargetWatch`].
pub(crate) fn fingerprint(content: Option<&str>) -> String {
    match content {
        None => "absent".to_string(),
        Some(content) => crate::core::docs_merge::sha256_hex(content.as_bytes()),
    }
}

/// The last resort of a target that could not be put back: keep it, marked.
fn mark_instead(
    project_path: &Path,
    target: &str,
    step: u32,
    outcome: KilledOutcome,
    kind: StopKind,
) -> KilledOutcome {
    let settlement = KilledSettlement::of(outcome);
    let reason = match kind {
        StopKind::Cancelled => killed_reason(step, &settlement),
        StopKind::CutOff => interrupted_reason(step, &settlement),
    };
    match mark_partial(project_path, target, step, &reason) {
        Ok(true) => outcome,
        Ok(false) => KilledOutcome::Unsettled,
        Err(e) => {
            tracing::error!("Audit step {step}: `{target}` could not be marked partial: {e}");
            KilledOutcome::Unsettled
        }
    }
}

/// Settle the target of a step Kronn stopped: put it back as it was before the
/// step when what is on disk is the agent's, keep and mark it when it cannot
/// be vouched for. `observed` is the fingerprint of the last content Kronn saw
/// the agent write (see [`TargetWatch`]). Never silent: an error is
/// [`KilledOutcome::Unsettled`], not "wrote nothing"; and content that is
/// displaced and was not the agent's known output is kept aside first.
pub(crate) fn settle_killed(
    project_path: &Path,
    target: &str,
    pre_content: Option<&str>,
    observed: Option<&str>,
    kind: StopKind,
    step: u32,
) -> KilledSettlement {
    if target == "REVIEW" || target.is_empty() {
        return KilledSettlement::of(KilledOutcome::Untouched);
    }
    let now = match read_target(project_path, target) {
        Ok(now) => now,
        Err(e) => {
            tracing::error!("Audit step {step}: `{target}` could not be read: {e}");
            return KilledSettlement::of(mark_instead(
                project_path,
                target,
                step,
                KilledOutcome::Partial,
                kind,
            ));
        }
    };
    if now.as_deref() == pre_content {
        return KilledSettlement::of(KilledOutcome::Untouched);
    }
    // Absent or blank: nothing a human wrote can be lost by putting it back.
    let vouched = match now.as_deref() {
        None => true,
        Some(content) if content.trim().is_empty() => true,
        Some(content) => observed == Some(fingerprint(Some(content)).as_str()),
    };
    let mut displaced = None;
    if !vouched {
        let content = now.as_deref().unwrap_or_default();
        match kind {
            StopKind::CutOff => {
                return KilledSettlement::of(mark_instead(
                    project_path,
                    target,
                    step,
                    KilledOutcome::ChangedSinceSeen,
                    kind,
                ));
            }
            StopKind::Cancelled => match save_aside(project_path, target, content) {
                Ok(kept) => displaced = Some(kept),
                Err(e) => {
                    // Never replace what could not be kept.
                    tracing::error!("Audit step {step}: {e}");
                    return KilledSettlement::of(mark_instead(
                        project_path,
                        target,
                        step,
                        KilledOutcome::Partial,
                        kind,
                    ));
                }
            },
        }
    }
    match restore_target(project_path, target, pre_content) {
        Ok(_) => KilledSettlement {
            outcome: KilledOutcome::Restored,
            displaced,
        },
        Err(e) => {
            tracing::error!("Audit step {step}: `{target}` could not be put back: {e}");
            KilledSettlement {
                outcome: mark_instead(project_path, target, step, KilledOutcome::Partial, kind),
                displaced,
            }
        }
    }
}

/// Watches a step's target while its agent runs, so a later crash knows what
/// the agent itself had written. [`TargetWatch::moved`] is cheap enough for the
/// activity tick: one `lstat`, and a read only when the file changed.
#[derive(Debug)]
pub(crate) struct TargetWatch {
    project_path: PathBuf,
    target: String,
    stat: Option<(Option<std::time::SystemTime>, u64)>,
    first: bool,
    reported: String,
}

impl TargetWatch {
    /// `None` for a step with no file target.
    pub(crate) fn new(
        project_path: &Path,
        target: &str,
        pre_content: Option<&str>,
    ) -> Option<Self> {
        if target == "REVIEW" || target.is_empty() {
            return None;
        }
        Some(Self {
            project_path: project_path.to_path_buf(),
            target: target.to_string(),
            stat: None,
            first: true,
            reported: fingerprint(pre_content),
        })
    }

    /// The fingerprint of the target's new content, when it changed since the
    /// last call (or since the step started, the first time).
    pub(crate) fn moved(&mut self) -> Option<String> {
        let stat = std::fs::symlink_metadata(self.project_path.join(&self.target))
            .ok()
            .map(|meta| (meta.modified().ok(), meta.len()));
        if !self.first && stat == self.stat {
            return None;
        }
        self.first = false;
        self.stat = stat;
        let content = read_target(&self.project_path, &self.target).ok()?;
        let seen = fingerprint(content.as_deref());
        if seen == self.reported {
            return None;
        }
        self.reported = seen.clone();
        Some(seen)
    }

    /// The last fingerprint seen (the pre-step content's until it moves).
    pub(crate) fn last_seen(&self) -> &str {
        &self.reported
    }
}

/// A step that never ended and has its pre-step snapshot, settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettledStep {
    pub step_index: u32,
    pub target_file: String,
    pub outcome: KilledOutcome,
    pub displaced: Option<String>,
    settlement: KilledSettlement,
}

/// The steps of `run_id` that never ended and have a snapshot. Database read
/// only: the file work of [`settle_pending`] must not hold the writer
/// connection.
pub(crate) fn pending_steps(
    conn: &rusqlite::Connection,
    run_id: &str,
) -> anyhow::Result<Vec<crate::db::audit_runs::StepSnapshot>> {
    crate::db::audit_runs::unfinished_step_snapshots(conn, run_id)
}

/// Settle the targets of `pending`: their agent died with the backend, so each
/// target is put back from its snapshot, or marked. File work only. Nothing is
/// touched, and nothing returned, when the project root is not there — a
/// restore must not recreate an unmounted project; the steps stay open and are
/// settled at the next boot or launch that finds the root.
pub(crate) fn settle_pending(
    project_root: &Path,
    pending: &[crate::db::audit_runs::StepSnapshot],
) -> Vec<SettledStep> {
    if !project_root.is_dir() {
        if !pending.is_empty() {
            tracing::warn!(
                "Audit steps cut off with the backend are left unsettled: project root {} does not exist",
                project_root.display()
            );
        }
        return Vec::new();
    }
    pending
        .iter()
        .map(|snapshot| {
            let settlement = settle_killed(
                project_root,
                &snapshot.target_file,
                snapshot.content.as_deref(),
                snapshot.observed_hash.as_deref(),
                StopKind::CutOff,
                snapshot.step_index,
            );
            SettledStep {
                step_index: snapshot.step_index,
                target_file: snapshot.target_file.clone(),
                outcome: settlement.outcome,
                displaced: settlement.displaced.clone(),
                settlement,
            }
        })
        .collect()
}

/// Record the verdicts of [`settle_pending`]: each step failed, with what
/// became of its target.
pub(crate) fn record_settled(
    conn: &rusqlite::Connection,
    run_id: &str,
    settled: &[SettledStep],
) -> anyhow::Result<()> {
    for step in settled {
        crate::db::audit_runs::settle_killed_step(
            conn,
            run_id,
            step.step_index,
            &interrupted_reason(step.step_index, &step.settlement),
            step.settlement.target_state(),
            step.settlement.keeps_snapshot(),
            chrono::Utc::now(),
        )?;
    }
    Ok(())
}

/// [`pending_steps`], [`settle_pending`] and [`record_settled`] in one go, for
/// callers that own their connection (boot).
pub(crate) fn settle_unfinished_steps(
    conn: &rusqlite::Connection,
    run_id: &str,
    project_root: &Path,
) -> anyhow::Result<Vec<SettledStep>> {
    let pending = pending_steps(conn, run_id)?;
    let settled = settle_pending(project_root, &pending);
    record_settled(conn, run_id, &settled)?;
    Ok(settled)
}

/// Boot, after the agents of the previous process were stopped: no step of a
/// previous process is still running, so every step that never ended is
/// settled now — not at the next launch, which may be days away with the doc
/// half-written and nothing marking it. Returns how many steps were settled.
pub(crate) fn settle_unfinished_at_boot(conn: &rusqlite::Connection) -> anyhow::Result<usize> {
    settle_unfinished_except(conn, &HashSet::new())
}

/// [`settle_unfinished_at_boot`] leaving the runs of `skip` alone: an agent of
/// theirs may still be writing, so what is on disk is not settled yet.
fn settle_unfinished_except(
    conn: &rusqlite::Connection,
    skip: &HashSet<String>,
) -> anyhow::Result<usize> {
    let mut count = 0;
    for (run_id, project_id) in crate::db::audit_runs::runs_with_unfinished_steps(conn)? {
        if skip.contains(&run_id) {
            tracing::error!(
                "Boot: the unfinished steps of audit run {run_id} are left unsettled — an agent of the run may still be writing"
            );
            continue;
        }
        let Some(project) = crate::db::projects::get_project(conn, &project_id)? else {
            continue;
        };
        let root = crate::core::scanner::resolve_host_path(&project.path);
        match settle_unfinished_steps(conn, &run_id, &root) {
            Ok(settled) => {
                for step in &settled {
                    tracing::warn!(
                        "Boot: step {} of audit run {run_id} was cut off with the backend — `{}`: {:?}",
                        step.step_index, step.target_file, step.outcome
                    );
                }
                count += settled.len();
            }
            Err(e) => {
                tracing::warn!("Boot: could not settle the unfinished steps of run {run_id}: {e}")
            }
        }
    }
    Ok(count)
}

/// KT-1324 — stop every process still carrying the tag of a run this database
/// knows (see `audit_run_tag`), wherever it moved in the process tree. The
/// agent of an ACP adapter has no pid recorded on its run, and a recorded group
/// misses what left it: only the tag, which everything an agent starts
/// inherits, finds them all. Returns the runs something of which may still be
/// alive (or could not be looked for): their steps must not be settled.
pub(crate) fn reap_tagged_agents_at_boot(
    conn: &rusqlite::Connection,
) -> anyhow::Result<HashSet<String>> {
    let mut unsafe_to_settle = HashSet::new();
    for run_id in crate::db::audit_runs::runs_that_may_have_agents(conn)? {
        let tag = crate::core::dispatch_reaper::audit_run_tag(&run_id);
        let report = crate::core::dispatch_reaper::reap_dispatch(&tag);
        if report.signalled > 0 {
            tracing::warn!(
                "Boot: stopped {} process(es) the previous backend left running for audit run {run_id}",
                report.signalled
            );
        }
        if report.may_still_run() {
            tracing::error!(
                "Boot: processes of audit run {run_id} survived the reap or could not be looked for: {report:?}"
            );
            unsafe_to_settle.insert(run_id);
        }
    }
    Ok(unsafe_to_settle)
}

/// Boot (KT-1211, KT-1324): the previous backend's agents are stopped first —
/// the one recorded on each run, then everything carrying a run's tag — and only
/// then are the steps they never ended settled. Settling first lets an agent
/// that outlived the backend write after the verdict ("it had written nothing")
/// was recorded. A run something of which may still be alive is not settled.
/// Returns how many steps were settled.
pub(crate) fn stop_agents_and_settle_at_boot(conn: &rusqlite::Connection) -> anyhow::Result<usize> {
    match crate::core::agent_orphans::reap_recorded_agents(conn) {
        Ok(0) => {}
        Ok(n) => tracing::warn!("Killed {n} audit agent(s) left running by the previous backend"),
        Err(e) => tracing::warn!("Failed to reap orphaned audit agents: {e}"),
    }
    let skip = match reap_tagged_agents_at_boot(conn) {
        Ok(skip) => skip,
        Err(e) => {
            // Unknown what still runs: no run is settled on a guess.
            tracing::error!("Failed to reap the tagged audit agents: {e}");
            return Ok(0);
        }
    };
    settle_unfinished_except(conn, &skip)
}

/// Why accepting a failed step as it stands is refused unless a human
/// overrides it: the step's document is known to be unsound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AcceptBlocker {
    /// The agent's output is cut short.
    Truncated,
    /// Citations that do not resolve against the tree (or the documentary
    /// gate's blockers).
    FabricatedCitations,
    /// A deterministic detector signal nothing in the output disposes of.
    UndisposedSignal,
    /// The dimension coverage table is incomplete.
    IncompleteCoverage,
    /// The step was stopped halfway: what is on disk is not its output.
    Killed,
    /// The step never ended (no verdict was ever recorded on it): whatever its
    /// agent left on disk is unvetted, and no failure reason says why.
    NeverEnded,
}

/// Fragments of the reasons the audit gates persist on a failed step. Each is
/// produced by exactly one gate; `blocker_of_reason_matches_the_real_gates`
/// builds the real reasons and checks they are all recognised.
pub(crate) const TRUNCATED_FRAGMENT: &str = "looks truncated";
pub(crate) const FABRICATED_FRAGMENT: &str = "fabricated `[src:]` citation";
pub(crate) const DOCUMENTARY_FRAGMENT: &str = "Documentary gate";
pub(crate) const DETECTOR_FRAGMENT: &str = "detector signal(s) injected but never addressed";
pub(crate) const COVERAGE_FRAGMENT: &str = "dimension coverage incomplete";
pub(crate) const KILLED_FRAGMENT: &str = "stopped during step";

impl AcceptBlocker {
    pub(crate) fn of_reason(reason: &str) -> Option<Self> {
        if reason.contains(KILLED_FRAGMENT) {
            Some(Self::Killed)
        } else if reason.contains(TRUNCATED_FRAGMENT) {
            Some(Self::Truncated)
        } else if reason.contains(FABRICATED_FRAGMENT) || reason.contains(DOCUMENTARY_FRAGMENT) {
            Some(Self::FabricatedCitations)
        } else if reason.contains(DETECTOR_FRAGMENT) {
            Some(Self::UndisposedSignal)
        } else if reason.contains(COVERAGE_FRAGMENT) {
            Some(Self::IncompleteCoverage)
        } else {
            None
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Truncated => "its output is truncated",
            Self::FabricatedCitations => "it holds citations that do not resolve",
            Self::UndisposedSignal => "a detector signal is not disposed of",
            Self::IncompleteCoverage => "its dimension coverage is incomplete",
            Self::Killed => "the step was stopped before it finished",
            Self::NeverEnded => "the step never ended, so nothing vouches for what it wrote",
        }
    }
}

fn outcome_text(settlement: &KilledSettlement) -> String {
    let base = match settlement.outcome {
        KilledOutcome::Restored => "its document was put back as it was before the step",
        KilledOutcome::Untouched => "it had written nothing",
        KilledOutcome::Partial => {
            "its document could not be put back: what the agent wrote is kept under a partial marker"
        }
        KilledOutcome::ChangedSinceSeen => {
            "its document changed after Kronn last saw the agent write it (a hand edit, or a late \
write): it was left in place under a partial marker, not overwritten"
        }
        KilledOutcome::Unsettled => {
            "its document could neither be put back nor marked: check it by hand"
        }
    };
    match &settlement.displaced {
        Some(kept) => format!(
            "{base}. The content it displaced was not the agent's known output and is kept at `{kept}`"
        ),
        None => base.to_string(),
    }
}

/// The text recorded on a step the user cancelled.
pub(crate) fn killed_reason(step: u32, settlement: &KilledSettlement) -> String {
    format!(
        "Audit {KILLED_FRAGMENT} {step} by the user: {}.",
        outcome_text(settlement)
    )
}

/// The text recorded on a step whose agent died with the backend.
pub(crate) fn interrupted_reason(step: u32, settlement: &KilledSettlement) -> String {
    format!(
        "Audit {KILLED_FRAGMENT} {step} (the backend stopped): {}.",
        outcome_text(settlement)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        root
    }

    #[test]
    fn a_marked_target_is_marked_once_and_clears_back_to_the_agents_text() {
        let root = project();
        let file = root.path().join("docs/architecture.md");
        std::fs::write(&file, "# Architecture\n\nhalf a sentence").unwrap();
        assert!(mark_partial(
            root.path(),
            "docs/architecture.md",
            3,
            "output looks truncated"
        )
        .unwrap());
        let marked = std::fs::read_to_string(&file).unwrap();
        assert!(has_partial_marker(&marked));
        assert!(marked.contains("step 3"), "{marked}");
        // Marking again replaces the marker, never stacks one.
        assert!(mark_partial(root.path(), "docs/architecture.md", 3, "again").unwrap());
        let again = std::fs::read_to_string(&file).unwrap();
        assert_eq!(again.matches(PARTIAL_OPEN).count(), 1);
        assert!(clear_partial_marker(root.path(), "docs/architecture.md").unwrap());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "# Architecture\n\nhalf a sentence"
        );
        assert!(!clear_partial_marker(root.path(), "docs/architecture.md").unwrap());
    }

    #[test]
    fn an_absent_or_empty_target_gets_no_marker() {
        let root = project();
        assert!(!mark_partial(root.path(), "docs/none.md", 1, "x").unwrap());
        std::fs::write(root.path().join("docs/empty.md"), " \n").unwrap();
        assert!(!mark_partial(root.path(), "docs/empty.md", 1, "x").unwrap());
        assert_eq!(
            std::fs::read_to_string(root.path().join("docs/empty.md")).unwrap(),
            " \n"
        );
    }

    #[test]
    fn a_reason_cannot_close_the_marker_early() {
        let banner = partial_banner(2, "oops --> <script>");
        assert_eq!(
            banner.matches("-->").count(),
            2,
            "only the open and close markers: {banner}"
        );
    }

    #[test]
    fn restore_puts_back_the_prior_content_or_removes_a_new_file() {
        let root = project();
        let file = root.path().join("docs/glossary.md");
        std::fs::write(&file, "half-writ").unwrap();
        assert!(
            restore_target(root.path(), "docs/glossary.md", Some("# Glossary\nwhole\n")).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "# Glossary\nwhole\n"
        );
        assert!(
            !restore_target(root.path(), "docs/glossary.md", Some("# Glossary\nwhole\n")).unwrap()
        );
        assert!(restore_target(root.path(), "docs/glossary.md", None).unwrap());
        assert!(!file.exists());
        assert!(!restore_target(root.path(), "docs/glossary.md", None).unwrap());
    }

    #[test]
    fn a_failed_step_marks_only_a_target_it_changed() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        std::fs::write(&file, "# Stack\nrust\n").unwrap();
        let pre = target_snapshot(root.path(), "docs/stack.md").unwrap();
        // Untouched: nothing to mark.
        assert_eq!(
            settle_failed_target(root.path(), "docs/stack.md", &pre, 4, "agent exited 1").unwrap(),
            None
        );
        assert!(!has_partial_marker(
            &std::fs::read_to_string(&file).unwrap()
        ));
        // Changed: kept and marked.
        std::fs::write(&file, "# Stack\nrust, and then the agent died mid-sen").unwrap();
        assert_eq!(
            settle_failed_target(root.path(), "docs/stack.md", &pre, 4, "agent exited 1").unwrap(),
            Some(STATE_PARTIAL)
        );
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(has_partial_marker(&content));
        assert!(
            content.contains("mid-sen"),
            "the agent's text is kept: {content}"
        );
    }

    #[test]
    fn a_killed_step_gets_its_prior_document_back() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        std::fs::write(&file, "half").unwrap();
        let seen = fingerprint(Some("half"));
        let settled = settle_killed(
            root.path(),
            "docs/stack.md",
            Some("# Stack\nwhole\n"),
            Some(&seen),
            StopKind::Cancelled,
            2,
        );
        assert_eq!(settled.outcome, KilledOutcome::Restored);
        assert_eq!(
            settled.displaced, None,
            "the agent's own known output is not kept aside"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Stack\nwhole\n");
        // Nothing written: nothing restored.
        let again = settle_killed(
            root.path(),
            "docs/stack.md",
            Some("# Stack\nwhole\n"),
            None,
            StopKind::Cancelled,
            2,
        );
        assert_eq!(again.outcome, KilledOutcome::Untouched);
    }

    // One test per gate, each on the reason the gate's own code builds, so a
    // gate that stops being recognised fails exactly its own test.
    fn recognised(reason: String, expected: Option<AcceptBlocker>) {
        assert_eq!(AcceptBlocker::of_reason(&reason), expected, "{reason}");
    }

    #[test]
    fn the_size_floor_reason_blocks_acceptance() {
        recognised(
            super::super::validation::truncated_reason("docs/a.md", 40, 4000),
            Some(AcceptBlocker::Truncated),
        );
    }

    #[test]
    fn the_enforce_gate_reason_blocks_acceptance() {
        recognised(
            super::super::anti_hallu_enforce::enforce_failure_reason(3, 3),
            Some(AcceptBlocker::FabricatedCitations),
        );
    }

    #[test]
    fn the_documentary_gate_reason_blocks_acceptance() {
        let diagnostic = crate::core::document_optimization::Diagnostic {
            code: "stale_citation".into(),
            path: "docs/a.md".into(),
            message: "x".into(),
            blocking: true,
        };
        recognised(
            super::super::document_repair::feedback(&[diagnostic]),
            Some(AcceptBlocker::FabricatedCitations),
        );
    }

    #[test]
    fn the_detector_disposition_reason_blocks_acceptance() {
        recognised(
            super::super::validation::undisposed_reason(2, "a (`x`)"),
            Some(AcceptBlocker::UndisposedSignal),
        );
    }

    #[test]
    fn the_coverage_reason_blocks_acceptance() {
        recognised(
            super::super::validation::coverage_reason("docs/x.md", "3 missing"),
            Some(AcceptBlocker::IncompleteCoverage),
        );
    }

    #[test]
    fn the_stopped_step_reasons_block_acceptance() {
        let of = |outcome| KilledSettlement {
            outcome,
            displaced: None,
        };
        recognised(
            killed_reason(5, &of(KilledOutcome::Restored)),
            Some(AcceptBlocker::Killed),
        );
        recognised(
            killed_reason(5, &of(KilledOutcome::Unsettled)),
            Some(AcceptBlocker::Killed),
        );
        recognised(
            interrupted_reason(5, &of(KilledOutcome::Partial)),
            Some(AcceptBlocker::Killed),
        );
        recognised(
            interrupted_reason(5, &of(KilledOutcome::ChangedSinceSeen)),
            Some(AcceptBlocker::Killed),
        );
    }

    #[test]
    fn an_ordinary_failure_is_not_a_blocker() {
        // Not an unsound document: an agent that exited, an empty output.
        recognised("agent exited with code 1".to_string(), None);
        recognised(
            "step produced no output: `docs/a.md` is empty (0 B) after CLI completed".to_string(),
            None,
        );
    }

    #[test]
    fn a_target_that_cannot_be_settled_is_never_reported_as_wrote_nothing() {
        let root = project();
        // Nothing written: whole.
        std::fs::write(root.path().join("docs/a.md"), "whole").unwrap();
        let untouched = settle_killed(
            root.path(),
            "docs/a.md",
            Some("whole"),
            None,
            StopKind::Cancelled,
            2,
        );
        assert_eq!(untouched.outcome, KilledOutcome::Untouched);
        // Written: put back.
        std::fs::write(root.path().join("docs/a.md"), "half").unwrap();
        let seen = fingerprint(Some("half"));
        let restored = settle_killed(
            root.path(),
            "docs/a.md",
            Some("whole"),
            Some(&seen),
            StopKind::Cancelled,
            2,
        );
        assert_eq!(restored.outcome, KilledOutcome::Restored);
        // The target is a directory: it can be neither read, restored nor marked.
        std::fs::create_dir_all(root.path().join("docs/dir.md")).unwrap();
        let stuck = settle_killed(
            root.path(),
            "docs/dir.md",
            Some("whole"),
            None,
            StopKind::Cancelled,
            2,
        );
        assert_eq!(stuck.outcome, KilledOutcome::Unsettled);
        assert_eq!(stuck.target_state(), None, "never recorded as restored");
        assert!(stuck.keeps_snapshot());
        assert_eq!(untouched.target_state(), Some(STATE_RESTORED));
    }

    #[test]
    fn a_hand_edit_after_a_crash_survives_boot() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        let conn = boot_conn(root.path());
        crate::db::audit_runs::insert_audit_step_start(
            &conn,
            "r1",
            2,
            "docs/stack.md",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::save_step_snapshot(
            &conn,
            "r1",
            2,
            "docs/stack.md",
            Some("# Stack\nwhole\n"),
        )
        .unwrap();
        // The agent was seen writing its half; the backend died; the user then
        // edited the file by hand before the next boot.
        crate::db::audit_runs::record_observed_output(
            &conn,
            "r1",
            2,
            &fingerprint(Some("# Stack\nhalf-writ")),
        )
        .unwrap();
        std::fs::write(&file, "# Stack\nmy own careful edit\n").unwrap();

        assert_eq!(settle_unfinished_at_boot(&conn).unwrap(), 1);
        let after = std::fs::read_to_string(&file).unwrap();
        assert!(
            after.contains("my own careful edit"),
            "the hand edit survives: {after}"
        );
        assert!(
            has_partial_marker(&after),
            "and the file is flagged: {after}"
        );
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        let step = steps.iter().find(|s| s.step_index == 2).unwrap();
        assert_eq!(step.target_state.as_deref(), Some(STATE_PARTIAL));
        assert!(step.ended_at.is_some());
        assert_eq!(
            AcceptBlocker::of_reason(step.step_warning.as_deref().unwrap()),
            Some(AcceptBlocker::Killed)
        );
        assert!(
            crate::db::audit_runs::step_snapshot(&conn, "r1", 2)
                .unwrap()
                .is_some(),
            "the snapshot is kept for the acceptance diff"
        );
        // The step created the file: a hand edit of it is just as safe.
        crate::db::audit_runs::insert_audit_step_start(
            &conn,
            "r1",
            3,
            "docs/new.md",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::save_step_snapshot(&conn, "r1", 3, "docs/new.md", None).unwrap();
        std::fs::write(root.path().join("docs/new.md"), "written by hand").unwrap();
        assert_eq!(settle_unfinished_at_boot(&conn).unwrap(), 1);
        assert!(std::fs::read_to_string(root.path().join("docs/new.md"))
            .unwrap()
            .contains("written by hand"));
    }

    #[test]
    fn what_the_agent_was_seen_writing_is_put_back_at_boot() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        let conn = boot_conn(root.path());
        crate::db::audit_runs::insert_audit_step_start(
            &conn,
            "r1",
            2,
            "docs/stack.md",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::save_step_snapshot(
            &conn,
            "r1",
            2,
            "docs/stack.md",
            Some("# Stack\nwhole\n"),
        )
        .unwrap();
        crate::db::audit_runs::record_observed_output(
            &conn,
            "r1",
            2,
            &fingerprint(Some("# Stack\nhalf-writ")),
        )
        .unwrap();
        std::fs::write(&file, "# Stack\nhalf-writ").unwrap();
        assert_eq!(settle_unfinished_at_boot(&conn).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Stack\nwhole\n");
        // And no copy is left aside: that was the agent's own half document.
        let strays: Vec<_> = std::fs::read_dir(root.path().join("docs"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("kronn-displaced"))
            .collect();
        assert!(strays.is_empty(), "{strays:?}");
    }

    #[test]
    fn a_cancel_keeps_aside_what_it_cannot_vouch_for_before_putting_the_doc_back() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        std::fs::write(&file, "# Stack\nwritten after the last observation\n").unwrap();
        let settled = settle_killed(
            root.path(),
            "docs/stack.md",
            Some("# Stack\nwhole\n"),
            None,
            StopKind::Cancelled,
            4,
        );
        assert_eq!(settled.outcome, KilledOutcome::Restored);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Stack\nwhole\n");
        let kept = settled
            .displaced
            .clone()
            .expect("the displaced content is kept");
        assert!(kept.starts_with("docs/stack.md.kronn-displaced-"), "{kept}");
        assert_eq!(
            std::fs::read_to_string(root.path().join(&kept)).unwrap(),
            "# Stack\nwritten after the last observation\n"
        );
        // And the step says where.
        assert!(killed_reason(4, &settled).contains(&kept));
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_symlink_is_never_written_through() {
        use std::os::unix::fs::symlink;
        let root = project();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.md");
        std::fs::write(&victim, "outside the repository").unwrap();

        // (a) The target itself became a symlink to a file outside.
        symlink(&victim, root.path().join("docs/stack.md")).unwrap();
        let a = settle_killed(
            root.path(),
            "docs/stack.md",
            Some("# Stack\nwhole\n"),
            None,
            StopKind::Cancelled,
            2,
        );
        assert_eq!(
            a.outcome,
            KilledOutcome::Unsettled,
            "a symlinked target is never written through"
        );
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "outside the repository"
        );
        assert!(restore_target(root.path(), "docs/stack.md", Some("x")).is_err());
        assert!(mark_partial(root.path(), "docs/stack.md", 2, "r").is_err());
        assert!(
            read_target(root.path(), "docs/stack.md").is_err(),
            "nor read through"
        );

        // (b) A parent directory became a symlink to a directory outside.
        let elsewhere = outside.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("glossary.md"), "outside").unwrap();
        symlink(&elsewhere, root.path().join("linked")).unwrap();
        assert!(restore_target(root.path(), "linked/glossary.md", Some("planted")).is_err());
        assert!(restore_target(root.path(), "linked/new/deep.md", Some("planted")).is_err());
        assert_eq!(
            std::fs::read_to_string(elsewhere.join("glossary.md")).unwrap(),
            "outside"
        );
        assert!(
            !elsewhere.join("new").exists(),
            "no directory was created outside"
        );

        // (c) A symlink at the old fixed temp name does not redirect a restore.
        std::fs::remove_file(root.path().join("docs/stack.md")).unwrap();
        std::fs::write(root.path().join("docs/stack.md"), "half").unwrap();
        symlink(&victim, root.path().join("docs/stack.kronn-restore.tmp")).unwrap();
        assert!(restore_target(root.path(), "docs/stack.md", Some("# Stack\nwhole\n")).unwrap());
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "outside the repository"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("docs/stack.md")).unwrap(),
            "# Stack\nwhole\n"
        );

        // (d) A target that climbs out of the project is refused.
        assert!(restore_target(root.path(), "../escape.md", Some("x")).is_err());
        assert!(!root.path().parent().unwrap().join("escape.md").exists());
    }

    #[test]
    fn the_exclusive_create_never_replaces_what_is_already_there() {
        let root = project();
        let file = root.path().join("docs/taken.md");
        std::fs::write(&file, "mine").unwrap();
        assert!(create_new_file(&file, "theirs").is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "mine");
    }

    #[test]
    fn a_missing_project_root_is_not_recreated_and_leaves_the_step_open() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("mounted-volume").join("repo");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        let conn = boot_conn(&root);
        crate::db::audit_runs::insert_audit_step_start(
            &conn,
            "r1",
            2,
            "docs/stack.md",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::save_step_snapshot(
            &conn,
            "r1",
            2,
            "docs/stack.md",
            Some("# Stack\nwhole\n"),
        )
        .unwrap();
        // The volume is gone at boot.
        std::fs::remove_dir_all(parent.path().join("mounted-volume")).unwrap();

        assert_eq!(settle_unfinished_at_boot(&conn).unwrap(), 0);
        assert!(!root.exists(), "the old location is not recreated");
        assert!(!parent.path().join("mounted-volume").exists());
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        let step = steps.iter().find(|s| s.step_index == 2).unwrap();
        assert!(
            step.ended_at.is_none(),
            "no verdict is invented: the step stays open"
        );
        assert!(step.target_state.is_none());
        assert!(
            crate::db::audit_runs::step_snapshot(&conn, "r1", 2)
                .unwrap()
                .is_some(),
            "the snapshot waits for the volume"
        );
        // Accepting it still needs the override.
        assert_eq!(
            super::super::accept::accept_blocker_of(step),
            Some(AcceptBlocker::NeverEnded)
        );
        // The volume comes back: the next boot settles it.
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/stack.md"), "# Stack\nhalf").unwrap();
        // (unobserved content of a cut-off agent is left in place, marked)
        assert_eq!(settle_unfinished_at_boot(&conn).unwrap(), 1);
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        assert!(steps
            .iter()
            .find(|s| s.step_index == 2)
            .unwrap()
            .ended_at
            .is_some());
    }

    #[test]
    fn the_watch_reports_only_what_changed_since_the_step_started() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        std::fs::write(&file, "before").unwrap();
        let mut watch = TargetWatch::new(root.path(), "docs/stack.md", Some("before")).unwrap();
        assert_eq!(watch.moved(), None, "unchanged since the step started");
        assert_eq!(watch.moved(), None);
        std::fs::write(&file, "the agent wrote this").unwrap();
        assert_eq!(
            watch.moved(),
            Some(fingerprint(Some("the agent wrote this")))
        );
        assert_eq!(watch.moved(), None, "reported once");
        assert_eq!(watch.last_seen(), fingerprint(Some("the agent wrote this")));
        assert!(TargetWatch::new(root.path(), "REVIEW", None).is_none());
    }

    fn boot_conn(root: &Path) -> rusqlite::Connection {
        boot_conn_of_run(root, "r1")
    }

    /// [`boot_conn`] with its run named: a tag is machine-wide, so a test that
    /// starts a real agent must not share its run id with the boots of the
    /// tests running beside it.
    fn boot_conn_of_run(root: &Path, run_id: &str) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES ('p1', 'P', ?1, ?2, ?2)",
            rusqlite::params![root.to_str().unwrap(), now],
        )
        .unwrap();
        crate::db::audit_runs::insert_running(
            &conn,
            run_id,
            "p1",
            "Full",
            "ClaudeCode",
            chrono::Utc::now(),
        )
        .unwrap();
        conn
    }

    /// The interpreter the scripted agents run under: macOS shows no
    /// environment for a system binary such as `sh`, and the tag is read there.
    /// Its absolute path: the agent's own environment has no `PATH` to find a
    /// `python3` shim by.
    #[cfg(unix)]
    fn python() -> Option<String> {
        ["python3", "python"].into_iter().find_map(|name| {
            let output = std::process::Command::new(name)
                .args(["-c", "import sys; print(sys.executable)"])
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|output| output.status.success())?;
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!path.is_empty()).then_some(path)
        })
    }

    /// An agent as the ACP adapter starts it — through `try_spawn`, so it
    /// carries its run's tag — that keeps rewriting `docs/stack.md` until it
    /// is stopped. No pid is recorded on the run, as for an adapter agent.
    #[cfg(unix)]
    fn spawn_writing_agent(python: &str, root: &Path, run_id: &str) -> tokio::process::Child {
        const WRITER: &str = "import time\n\
            while True:\n\
            \x20   open('docs/stack.md', 'w').write('half\\n')\n\
            \x20   time.sleep(0.02)\n";
        crate::agents::runner::try_spawn(
            python,
            None,
            &["-c".to_string(), WRITER.to_string()],
            root,
            "",
            None,
            crate::agents::runner::SpawnIo::Adapter(&[]),
            None,
            None,
            None,
            None,
            Some(run_id),
            &[],
            None,
        )
        .expect("the scripted agent starts")
    }

    #[cfg(unix)]
    async fn until_written(file: &Path, content: &str) {
        // An interpreter's start-up is slow on a loaded machine.
        for _ in 0..1500 {
            if std::fs::read_to_string(file).is_ok_and(|now| now == content) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("the agent never wrote {content:?}");
    }

    /// KT-1324 — the founding bug: the agent outlived the backend, boot settled
    /// the step it was writing, and the agent then wrote on. Boot now stops it
    /// first (nothing is recorded on the run to find it by), so what boot puts
    /// back is the last word.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_agent_that_outlived_the_backend_is_stopped_before_its_step_is_settled() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        let run = format!("r-{}", uuid::Uuid::new_v4());
        let conn = boot_conn_of_run(root.path(), &run);
        crate::db::audit_runs::insert_audit_step_start(
            &conn,
            &run,
            2,
            "docs/stack.md",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::save_step_snapshot(
            &conn,
            &run,
            2,
            "docs/stack.md",
            Some("# Stack\nwhole\n"),
        )
        .unwrap();
        let Some(python) = python() else {
            eprintln!("skipped: no python to script an agent with");
            return;
        };
        let mut agent = spawn_writing_agent(&python, root.path(), &run);
        until_written(&file, "half\n").await;
        crate::db::audit_runs::record_observed_output(&conn, &run, 2, &fingerprint(Some("half\n")))
            .unwrap();
        assert!(
            crate::db::audit_runs::recorded_agent_processes(&conn)
                .unwrap()
                .is_empty(),
            "nothing but the tag can find this agent"
        );

        assert_eq!(stop_agents_and_settle_at_boot(&conn).unwrap(), 1);

        let status = agent.try_wait().unwrap();
        assert!(
            status.is_some_and(|status| !status.success()),
            "the agent is stopped by the time boot returns: {status:?}"
        );
        // Whatever it would write after the verdict has nowhere to go.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "# Stack\nwhole\n",
            "no write lands after the step was settled"
        );
        let steps = crate::db::audit_runs::list_audit_steps(&conn, &run).unwrap();
        let step = steps.iter().find(|s| s.step_index == 2).unwrap();
        assert_eq!(step.target_state.as_deref(), Some(STATE_RESTORED));
    }

    /// A run whose agent may still be alive is not settled: the verdict would
    /// be on a file somebody is still writing.
    #[test]
    fn a_run_that_may_still_have_a_writer_is_left_unsettled() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        std::fs::write(&file, "half\n").unwrap();
        let conn = boot_conn(root.path());
        crate::db::audit_runs::insert_audit_step_start(
            &conn,
            "r1",
            2,
            "docs/stack.md",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::save_step_snapshot(
            &conn,
            "r1",
            2,
            "docs/stack.md",
            Some("# Stack\nwhole\n"),
        )
        .unwrap();
        crate::db::audit_runs::record_observed_output(&conn, "r1", 2, &fingerprint(Some("half\n")))
            .unwrap();

        let skip = HashSet::from(["r1".to_string()]);
        assert_eq!(settle_unfinished_except(&conn, &skip).unwrap(), 0);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "half\n");
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        assert!(
            steps
                .iter()
                .find(|s| s.step_index == 2)
                .unwrap()
                .ended_at
                .is_none(),
            "the step stays open for the next boot or launch"
        );
        assert!(crate::core::dispatch_reaper::ReapReport {
            unverified: true,
            ..Default::default()
        }
        .may_still_run());
        assert!(crate::core::dispatch_reaper::ReapReport {
            survivors: vec![42],
            ..Default::default()
        }
        .may_still_run());
        assert!(!crate::core::dispatch_reaper::ReapReport::default().may_still_run());
        // Nothing to look for, nothing in the way.
        assert_eq!(settle_unfinished_except(&conn, &HashSet::new()).unwrap(), 1);
    }

    /// Boot names only the runs of its own database: the agent of another
    /// backend on the same machine (a lane, a second install) is none of its
    /// business.
    #[cfg(unix)]
    #[tokio::test]
    async fn boot_leaves_the_agents_of_other_runs_alone() {
        let root = project();
        let conn = boot_conn_of_run(root.path(), &format!("r-{}", uuid::Uuid::new_v4()));
        let Some(python) = python() else {
            eprintln!("skipped: no python to script an agent with");
            return;
        };
        let mut stranger = spawn_writing_agent(&python, root.path(), "a-run-of-another-backend");

        assert_eq!(stop_agents_and_settle_at_boot(&conn).unwrap(), 0);

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            stranger.try_wait().unwrap().is_none(),
            "an agent that is not of this database's runs keeps running"
        );
        stranger.kill().await.unwrap();
    }

    #[test]
    fn boot_names_the_running_runs_and_those_with_an_unfinished_step() {
        let root = project();
        let conn = boot_conn(root.path());
        // Not running any more, nothing open: no agent to look for.
        crate::db::audit_runs::insert_running(
            &conn,
            "r-done",
            "p1",
            "Full",
            "ClaudeCode",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::mark_interrupted(&conn, "r-done", "done").unwrap();
        // Interrupted, but a step of it never ended.
        crate::db::audit_runs::insert_running(
            &conn,
            "r-open",
            "p1",
            "Full",
            "ClaudeCode",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::insert_audit_step_start(
            &conn,
            "r-open",
            1,
            "docs/x.md",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::save_step_snapshot(&conn, "r-open", 1, "docs/x.md", None).unwrap();
        crate::db::audit_runs::mark_interrupted(&conn, "r-open", "open").unwrap();

        let mut runs = crate::db::audit_runs::runs_that_may_have_agents(&conn).unwrap();
        runs.sort();
        assert_eq!(runs, vec!["r-open".to_string(), "r1".to_string()]);
    }

    #[test]
    fn boot_puts_back_the_doc_of_a_step_that_never_ended() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        let conn = boot_conn(root.path());
        // Step 1 done; step 2 was rewriting stack.md when the backend died.
        crate::db::audit_runs::insert_audit_step_start(
            &conn,
            "r1",
            2,
            "docs/stack.md",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::save_step_snapshot(
            &conn,
            "r1",
            2,
            "docs/stack.md",
            Some("# Stack\nwhole\n"),
        )
        .unwrap();
        crate::db::audit_runs::record_observed_output(
            &conn,
            "r1",
            2,
            &fingerprint(Some("# Stack\nhalf-writ")),
        )
        .unwrap();
        std::fs::write(&file, "# Stack\nhalf-writ").unwrap();

        assert_eq!(settle_unfinished_at_boot(&conn).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Stack\nwhole\n");
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        let step = steps.iter().find(|s| s.step_index == 2).unwrap();
        assert!(step.ended_at.is_some(), "the step now has a verdict");
        assert!(!step.cli_success);
        assert_eq!(step.target_state.as_deref(), Some(STATE_RESTORED));
        assert_eq!(
            AcceptBlocker::of_reason(step.step_warning.as_deref().unwrap()),
            Some(AcceptBlocker::Killed)
        );
        assert!(crate::db::audit_runs::step_snapshot(&conn, "r1", 2)
            .unwrap()
            .is_none());
        // Idempotent: nothing left to settle.
        assert_eq!(settle_unfinished_at_boot(&conn).unwrap(), 0);
    }

    #[test]
    fn boot_removes_a_doc_the_step_created_and_keeps_the_snapshot_of_an_unsettled_one() {
        let root = project();
        let conn = boot_conn(root.path());
        // Created by the step (absent before): removed.
        crate::db::audit_runs::insert_audit_step_start(
            &conn,
            "r1",
            1,
            "docs/new.md",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::save_step_snapshot(&conn, "r1", 1, "docs/new.md", None).unwrap();
        crate::db::audit_runs::record_observed_output(&conn, "r1", 1, &fingerprint(Some("half")))
            .unwrap();
        std::fs::write(root.path().join("docs/new.md"), "half").unwrap();
        // Unsettleable (a directory): marked neither restored nor partial.
        crate::db::audit_runs::insert_audit_step_start(
            &conn,
            "r1",
            2,
            "docs/dir.md",
            chrono::Utc::now(),
        )
        .unwrap();
        crate::db::audit_runs::save_step_snapshot(&conn, "r1", 2, "docs/dir.md", Some("whole"))
            .unwrap();
        std::fs::create_dir_all(root.path().join("docs/dir.md")).unwrap();

        assert_eq!(settle_unfinished_at_boot(&conn).unwrap(), 2);
        assert!(!root.path().join("docs/new.md").exists());
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        let stuck = steps.iter().find(|s| s.step_index == 2).unwrap();
        assert_eq!(stuck.target_state, None);
        assert!(stuck.ended_at.is_some());
        assert!(
            crate::db::audit_runs::step_snapshot(&conn, "r1", 2)
                .unwrap()
                .is_some(),
            "kept for the acceptance diff"
        );
        // Accepting it needs an override: it is not a restored step.
        assert_eq!(
            super::super::accept::accept_blocker_of(stuck),
            Some(AcceptBlocker::Killed)
        );
    }

    #[test]
    fn accept_asks_for_an_override_on_a_step_that_never_ended() {
        let conn = boot_conn(Path::new("/tmp"));
        crate::db::audit_runs::insert_audit_step_start(
            &conn,
            "r1",
            3,
            "docs/x.md",
            chrono::Utc::now(),
        )
        .unwrap();
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        let open = &steps[0];
        assert!(open.ended_at.is_none() && open.step_warning.is_none());
        assert_eq!(
            super::super::accept::accept_blocker_of(open),
            Some(AcceptBlocker::NeverEnded)
        );
        // A step that ended failed for an ordinary reason is not blocked.
        crate::db::audit_runs::finalize_audit_step(
            &conn,
            "r1",
            3,
            chrono::Utc::now(),
            5,
            &crate::db::audit_runs::StepTokens::default(),
            None,
            false,
            Some("agent exited with code 1"),
            false,
        )
        .unwrap();
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        assert_eq!(super::super::accept::accept_blocker_of(&steps[0]), None);
    }
}
