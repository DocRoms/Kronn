//! A bounded handover for a fresh CLI session — KT-193 DoD 2.
//!
//! The premise of KT-193 is that a CLI session is a disposable compute unit and
//! the PLAN is the durable memory. Rotating a session only works if what matters
//! survives without the transcript: this assembles that, from the plan, and
//! bounds it in bytes.
//!
//! Why bounded, and not "as much as fits": an unbounded bundle recreates the
//! problem it exists to solve. This very session reached 4 143 787 451 tokens
//! because nothing ever said "enough". A bundle that grew with the project would
//! simply move the cost from the transcript to the handover.
//!
//! Why the PLAN and not a summary of the conversation: a summary is a lossy
//! retelling nobody verified. Objective, DoD, blockers and evidence were written
//! deliberately and are already the record — and if something important lives
//! only in the chat, that is a signal to write it down, not to ship more chat.
//!
//! What is deliberately NOT in here: the message history. A fresh session that
//! needs an old message asks for that one message (DoD 4), which costs a
//! retrieval instead of a re-read of everything.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Byte ceiling for one bundle. Roughly 4 000 tokens at 3.7 B/token — small
/// enough that a rotation is cheap, large enough for an objective, its checklist
/// and its blockers.
pub const BUNDLE_MAX_BYTES: usize = 16_384;

// Raising the ceiling must be a deliberate, visible act — so it breaks the
// BUILD rather than a test. A handover that grew with the project would move the
// cost from the transcript to the bundle and change nothing, which is the exact
// failure KT-193 exists to prevent.
const _: () = assert!(
    BUNDLE_MAX_BYTES <= 20_480,
    "resume bundle ceiling raised: a bigger handover defeats its own purpose"
);

/// Per-section ceilings, so one long description cannot crowd out the blockers.
/// A single budget for the whole bundle would let whatever is assembled first
/// eat it all — and the sections are not equally replaceable.
const OBJECTIVE_MAX_BYTES: usize = 4_096;
const TASK_LINE_MAX_BYTES: usize = 240;
const SECTION_MAX_ITEMS: usize = 20;

/// One line about a task: enough to recognise it, not enough to re-read it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BundleTask {
    pub reference: String,
    pub title: String,
    pub status: String,
    /// `n/m` — an agent needs the shape of the remaining work, not every
    /// sentence of it.
    pub dod_progress: Option<String>,
}

/// What a fresh session needs to continue, and nothing else.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResumeBundle {
    /// The current objective, truncated if enormous.
    pub objective: Option<String>,
    /// Unticked DoD sentences of the objective: what "done" still means.
    pub open_dod: Vec<String>,
    /// Tasks in flight — the shape of the work, one line each.
    pub active_tasks: Vec<BundleTask>,
    /// Blocked tasks with their stated reason. Kept even when the budget is
    /// tight: a fresh session that re-attempts a blocked task wastes far more
    /// than the bytes this costs.
    pub blockers: Vec<String>,
    /// Sections that were cut for size, named. A bundle that silently dropped
    /// blockers would be worse than one that admits it.
    pub omitted: Vec<String>,
    pub bytes: usize,
}

/// Truncate on a char boundary, marking the cut so nobody reads a fragment as
/// the whole thing.
fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max.saturating_sub(3);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Assemble a bundle from a plan snapshot.
///
/// Sections are added in order of how badly a fresh session needs them:
/// objective, then blockers, then open DoD, then active tasks. Whatever no
/// longer fits is named in `omitted` rather than dropped in silence.
pub fn build(
    objective_title: Option<&str>,
    objective_description: Option<&str>,
    open_dod: &[String],
    active: &[BundleTask],
    blockers: &[String],
) -> ResumeBundle {
    let mut omitted: Vec<String> = Vec::new();
    let mut used = 0_usize;

    let objective = match (objective_title, objective_description) {
        (None, _) => None,
        (Some(title), description) => {
            let joined = match description {
                Some(body) if !body.trim().is_empty() => format!("{title}\n\n{body}"),
                _ => title.to_string(),
            };
            let clipped = clip(&joined, OBJECTIVE_MAX_BYTES.min(BUNDLE_MAX_BYTES));
            if clipped.len() < joined.len() {
                omitted.push("objective (truncated)".to_string());
            }
            used += clipped.len();
            Some(clipped)
        }
    };

    // Blockers before the checklist: re-attempting a blocked task costs far more
    // than the bytes needed to say it is blocked.
    let mut kept_blockers = Vec::new();
    for blocker in blockers.iter().take(SECTION_MAX_ITEMS) {
        let line = clip(blocker, TASK_LINE_MAX_BYTES);
        if used + line.len() > BUNDLE_MAX_BYTES {
            omitted.push(format!(
                "{} blocker(s)",
                blockers.len() - kept_blockers.len()
            ));
            break;
        }
        used += line.len();
        kept_blockers.push(line);
    }
    if blockers.len() > SECTION_MAX_ITEMS && !omitted.iter().any(|o| o.contains("blocker")) {
        omitted.push(format!("{} blocker(s)", blockers.len() - SECTION_MAX_ITEMS));
    }

    let mut kept_dod = Vec::new();
    for sentence in open_dod.iter().take(SECTION_MAX_ITEMS) {
        let line = clip(sentence, TASK_LINE_MAX_BYTES);
        if used + line.len() > BUNDLE_MAX_BYTES {
            omitted.push(format!(
                "{} open DoD item(s)",
                open_dod.len() - kept_dod.len()
            ));
            break;
        }
        used += line.len();
        kept_dod.push(line);
    }
    if open_dod.len() > SECTION_MAX_ITEMS && !omitted.iter().any(|o| o.contains("DoD")) {
        omitted.push(format!(
            "{} open DoD item(s)",
            open_dod.len() - SECTION_MAX_ITEMS
        ));
    }

    let mut kept_tasks = Vec::new();
    for task in active.iter().take(SECTION_MAX_ITEMS) {
        let line = BundleTask {
            reference: task.reference.clone(),
            title: clip(&task.title, TASK_LINE_MAX_BYTES),
            status: task.status.clone(),
            dod_progress: task.dod_progress.clone(),
        };
        let cost = line.reference.len() + line.title.len() + line.status.len() + 8;
        if used + cost > BUNDLE_MAX_BYTES {
            omitted.push(format!(
                "{} active task(s)",
                active.len() - kept_tasks.len()
            ));
            break;
        }
        used += cost;
        kept_tasks.push(line);
    }
    if active.len() > SECTION_MAX_ITEMS && !omitted.iter().any(|o| o.contains("active task")) {
        omitted.push(format!(
            "{} active task(s)",
            active.len() - SECTION_MAX_ITEMS
        ));
    }

    ResumeBundle {
        objective,
        open_dod: kept_dod,
        active_tasks: kept_tasks,
        blockers: kept_blockers,
        omitted,
        bytes: used,
    }
}

#[cfg(test)]
#[path = "resume_bundle_test.rs"]
mod resume_bundle_test;
