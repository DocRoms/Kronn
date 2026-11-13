//! Human tech-debt decisions survive a re-audit. The prompt only asks the
//! agents to respect them, so Kronn snapshots the decided sheets before a run,
//! puts back any decision the run removed or changed, and flags new sheets
//! that repeat a finding a human already rejected or accepted.

use std::collections::BTreeSet;
use std::path::Path;

use crate::core::audit_validation::{
    append_history_entry, replace_current_status, td_decision, TdDecision,
};
use crate::core::td_index::{is_td_id, parse_sheet, TdSheet};

const GUARD_REVIEWER: &str = "Kronn decision guard";
const GUARD_NOTE: &str = "An audit changed this human decision; Kronn restored it.";
/// Body line Kronn adds to a new sheet that repeats a settled one.
pub const DUPLICATE_FIELD: &str = "- **Matches a settled sheet**:";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Decided {
    id: String,
    decision: TdDecision,
    content: String,
}

/// The sheets as they were before a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecisionSnapshot {
    decided: Vec<Decided>,
    known_ids: BTreeSet<String>,
}

impl DecisionSnapshot {
    pub fn is_empty(&self) -> bool {
        self.known_ids.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreOutcome {
    /// Settled sheets the run deleted, written back.
    pub restored: Vec<String>,
    /// Sheets whose decision the run changed, set back.
    pub reverted: Vec<String>,
    /// `(new sheet, settled sheet it repeats)`.
    pub settled_duplicates: Vec<(String, String)>,
    /// Decided sheets left alone because the name holds a link (or anything
    /// but a plain file), or something appeared at a missing one meanwhile:
    /// nothing was read, written nor deleted through it, and the other sheets
    /// were still put back. `(sheet id, why)`.
    pub refused: Vec<(String, String)>,
}

impl RestoreOutcome {
    pub fn is_empty(&self) -> bool {
        self.restored.is_empty()
            && self.reverted.is_empty()
            && self.settled_duplicates.is_empty()
            && self.refused.is_empty()
    }
}

/// The names of the `TD-*.md` sheets among `names`, with their ids.
fn sheet_ids(names: impl IntoIterator<Item = std::ffi::OsString>) -> Vec<String> {
    let mut ids: Vec<String> = names
        .into_iter()
        .filter_map(|name| {
            let name = name.to_str()?;
            let id = name.strip_suffix(".md").filter(|id| is_td_id(id))?;
            Some(id.to_string())
        })
        .collect();
    ids.sort();
    ids
}

/// The `TD-*.md` sheets directly under `td_dir`, read with `read`; none when
/// `td_dir` is a link (a sheet must live in the repository). `list` gives the
/// ids found in the folder.
fn sheet_files(
    td_dir: &Path,
    list: impl Fn(&Path) -> Vec<String>,
    read: impl Fn(&Path) -> Option<String>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = list(td_dir)
        .into_iter()
        .filter_map(|id| {
            let content = read(&td_dir.join(format!("{id}.md")))?;
            Some((id, content))
        })
        .collect();
    out.sort();
    out
}

/// [`sheet_files`] listing by path, for the snapshot taken before the agent
/// runs (a read-only view: nothing is written from it).
fn list_by_path(td_dir: &Path) -> Vec<String> {
    if !crate::core::td_index::is_real_dir(td_dir) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(td_dir) else {
        return Vec::new();
    };
    sheet_ids(
        entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.file_name()),
    )
}

/// Every sheet id, and the content of each decided one (a deferral is no
/// decision: the human postponed it).
pub fn snapshot(docs_dir: &Path) -> DecisionSnapshot {
    let mut snap = DecisionSnapshot::default();
    let read = |path: &Path| std::fs::read_to_string(path).ok();
    for (id, content) in sheet_files(&docs_dir.join("tech-debt"), list_by_path, read) {
        snap.known_ids.insert(id.clone());
        if let Some(decision) = td_decision(&content).filter(|d| d.is_settled()) {
            snap.decided.push(Decided {
                id,
                decision,
                content,
            });
        }
    }
    snap
}

fn normalized_area(area: &str) -> String {
    let area = area.trim().trim_matches('`').trim().to_lowercase();
    let area = area.trim_start_matches("./");
    match area.rsplit_once(':') {
        Some((path, line)) if line.chars().all(|c| c.is_ascii_digit() || c == '-') => {
            path.to_string()
        }
        _ => area.to_string(),
    }
}

fn normalized_title(title: &str) -> String {
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The rejected or accepted sheet a finding repeats: same file, same problem.
/// Deliberately strict, so a flag is never a guess.
pub fn settled_match<'a>(area: &str, title: &str, sheets: &'a [TdSheet]) -> Option<&'a TdSheet> {
    let (area, title) = (normalized_area(area), normalized_title(title));
    if area.is_empty() || area == "unknown" || title.is_empty() {
        return None;
    }
    sheets.iter().find(|s| {
        s.decision.is_some_and(TdDecision::leaves_index)
            && normalized_area(&s.area) == area
            && normalized_title(&s.title) == title
    })
}

fn put_back_decision(content: &str, decision: TdDecision) -> Option<String> {
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    replace_current_status(&mut lines, decision.status_label()).ok()?;
    // A sheet without front matter cannot carry the history entry; the status is what counts.
    let _ = append_history_entry(
        &mut lines,
        decision.status_label(),
        GUARD_REVIEWER,
        GUARD_NOTE,
    );
    let mut out = lines.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    (td_decision(&out) == Some(decision)).then_some(out)
}

fn flag_duplicate(content: &str, settled: &TdSheet) -> Option<String> {
    if content.contains(DUPLICATE_FIELD) {
        return None;
    }
    let why = if settled.decision == Some(TdDecision::Rejected) {
        "rejected"
    } else {
        "accepted as a decision"
    };
    let line = format!(
        "{DUPLICATE_FIELD} `{}` ({why} by a human). Settle this sheet the same way unless the code changed.",
        settled.id
    );
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let at = lines
        .iter()
        .position(|l| l.trim_start().to_lowercase().starts_with("- **status**:"))
        .map_or(lines.len(), |i| i + 1);
    lines.insert(at, line);
    let mut out = lines.join("\n");
    if content.ends_with('\n') || content.is_empty() {
        out.push('\n');
    }
    Some(out)
}

/// After a run: a rejected or accepted sheet the run deleted is written back;
/// a decided sheet whose decision changed gets it back (a confirmed sheet the
/// run deleted stays deleted: reconciliation reports it as possibly fixed);
/// a new active sheet repeating a settled one is flagged.
///
/// This runs seconds after the agent's last write, with a process it left
/// behind possibly still alive: `root` is the project, and every read and
/// write of a sheet goes through the rooted primitive under it, so a
/// `tech-debt/` folder (or a parent) swapped for a link never takes a sheet
/// outside the project (KT-1595). A `docs_dir` outside `root` is refused.
pub fn restore(
    root: &Path,
    docs_dir: &Path,
    snap: &DecisionSnapshot,
) -> Result<RestoreOutcome, String> {
    crate::core::fs_guard::assert_contained_no_symlink(root, docs_dir)?;
    let td_dir = docs_dir.join("tech-debt");
    let mut outcome = RestoreOutcome::default();
    for entry in &snap.decided {
        let path = td_dir.join(format!("{}.md", entry.id));
        match crate::core::fs_guard::guarded_read(root, &path) {
            Ok(Some(bytes)) => {
                let current = String::from_utf8(bytes)
                    .map_err(|e| format!("read {}: {e}", path.display()))?;
                if td_decision(&current) == Some(entry.decision) {
                    continue;
                }
                let fixed = put_back_decision(&current, entry.decision)
                    .unwrap_or_else(|| entry.content.clone());
                crate::core::fs_guard::guarded_replace(root, &path, fixed.as_bytes())?;
                outcome.reverted.push(entry.id.clone());
            }
            Ok(None) => {
                if !entry.decision.leaves_index() {
                    continue;
                }
                // Create-only: nothing is overwritten, and an entry that
                // appeared since the read (a link included) is left alone.
                if crate::core::fs_guard::guarded_write_new(root, &path, entry.content.as_bytes())?
                {
                    outcome.restored.push(entry.id.clone());
                } else {
                    outcome.refused.push((
                        entry.id.clone(),
                        "an entry appeared at the deleted sheet's name; left as it is".into(),
                    ));
                }
            }
            // A link at the sheet's own name (or anything but a plain file)
            // is left alone, never followed, and reported: the other decided
            // sheets are still put back. A `tech-debt/` folder (or a parent)
            // that is itself a link is not one sheet's problem: it aborts.
            Err(e)
                if crate::core::fs_guard::is_link_refusal(&e)
                    || e.kind() == std::io::ErrorKind::InvalidInput =>
            {
                crate::core::fs_guard::assert_contained_no_symlink(root, &td_dir)?;
                outcome.refused.push((entry.id.clone(), e.to_string()));
            }
            Err(e) => return Err(format!("read {}: {e}", path.display())),
        }
    }

    let read = |path: &Path| {
        crate::core::fs_guard::guarded_read(root, path)
            .ok()
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    };
    // Rooted listing: a refused folder lists nothing (no duplicate is flagged),
    // it never lists what a link points at.
    let list = |dir: &Path| {
        crate::core::fs_guard::guarded_list(root, dir)
            .ok()
            .flatten()
            .map(sheet_ids)
            .unwrap_or_default()
    };
    let files = sheet_files(&td_dir, list, read);
    let sheets: Vec<TdSheet> = files.iter().map(|(id, c)| parse_sheet(id, c)).collect();
    for ((id, content), sheet) in files.iter().zip(&sheets) {
        if snap.known_ids.contains(id) || !sheet.is_active() {
            continue;
        }
        let Some(settled) = settled_match(&sheet.area, &sheet.title, &sheets) else {
            continue;
        };
        if let Some(flagged) = flag_duplicate(content, settled) {
            let path = td_dir.join(format!("{id}.md"));
            crate::core::fs_guard::guarded_replace(root, &path, flagged.as_bytes())?;
        }
        outcome
            .settled_duplicates
            .push((id.clone(), settled.id.clone()));
    }
    Ok(outcome)
}

#[cfg(test)]
#[path = "td_decisions_test.rs"]
mod tests;
