//! What every agent run pays before doing any work — KT-192 DoD 3.
//!
//! The MCP catalogue is measured elsewhere (`scripts/ci/mcp_catalogue_census.py`,
//! 275 KB across the fleet). This measures the OTHER half of the fixed cost: the
//! blocks Kronn itself prepends to a prompt — anti-hallucination preamble, skills,
//! directives, profiles, user context, memory prelude.
//!
//! Two things it separates, because conflating them is how a budget stops being
//! usable:
//!
//! THE FLOOR is what a run pays with nothing selected, from CODE alone. Nobody
//! opts into it and nobody can opt out, so it multiplies by every run in the fleet.
//! That is the number worth a ceiling — and it excludes the user's own context,
//! which is read from disk and differs per machine.
//!
//! EVERYTHING ELSE scales with a choice: the skills, directives and profiles a
//! caller selected, and the notes a user wrote. Real costs, but chosen ones —
//! capping them would cap the feature rather than the waste. Reported, not gated.
//!
//! Bytes are exact. Token figures are estimates and gate nothing.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Ceiling on the CODE-FIXED floor — the blocks Kronn injects unconditionally and
/// identically on every machine.
///
/// Pinned to the measurement (1 702 B), tightened on a real reduction, never raised
/// to make a build pass. Same rule as `docs/AGENTS.md` and the MCP catalogue, and
/// that rule is what stopped an 84 KiB instruction file one defensible paragraph at
/// a time.
///
/// It deliberately excludes the user's own context, which is read from disk and
/// therefore differs per machine: gating on it would make the ceiling pass on a
/// bare CI runner and fail on the machine of whoever wrote the most notes. A
/// ceiling that depends on who runs it is not a ceiling.
pub const FLOOR_MAX_BYTES: usize = 1_702;

const _: () = assert!(
    FLOOR_MAX_BYTES <= 8_192,
    "a fixed preamble above 8 KiB is paid by every run in the fleet, forever"
);

/// Coarse average for English prose and code. Printed, never gated on — a real
/// tokenizer is model-specific and a ceiling must not rest on an approximation.
pub const BYTES_PER_TOKEN: f64 = 3.7;

/// One measured block of injected context.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ContextBlock {
    pub name: String,
    pub bytes: usize,
    /// True when this block is injected regardless of what the caller selected.
    /// Those are the ones that multiply by every run.
    pub always: bool,
    /// What makes it worth its bytes, or what to do about them.
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StaticContextInventory {
    pub blocks: Vec<ContextBlock>,
    /// Sum of the `always` blocks that come from CODE — identical on every
    /// machine, and the only figure the ceiling gates on.
    pub floor_bytes: usize,
    /// The user's own context, read from disk. Always injected, but its size is
    /// the user's to choose, so it is reported beside the floor rather than folded
    /// into it.
    pub user_bytes: usize,
    /// Sum of everything measured, selection and user context included.
    pub total_bytes: usize,
}

impl StaticContextInventory {
    pub fn over_floor_budget(&self) -> Option<usize> {
        self.floor_bytes
            .checked_sub(FLOOR_MAX_BYTES)
            .filter(|excess| *excess > 0)
    }

    pub fn estimated_floor_tokens(&self) -> usize {
        (self.floor_bytes as f64 / BYTES_PER_TOKEN) as usize
    }
}

/// Measure what a run would be given.
///
/// Takes the selections rather than reading them from a config, so the floor can
/// be measured by passing nothing — which is exactly the case no caller can see
/// but every caller pays.
pub fn inventory(
    skill_ids: &[String],
    directive_ids: &[String],
    profile_ids: &[String],
) -> StaticContextInventory {
    let mut blocks = vec![ContextBlock {
        name: "anti_halluc::PREAMBLE".to_string(),
        bytes: crate::core::anti_halluc::PREAMBLE.len(),
        always: true,
        note: "the NOT_FOUND contract; removing it is what let agents invent paths".to_string(),
    }];

    // Always injected, but NOT part of the gated floor: it is read from disk, so
    // its size differs per machine. See `FLOOR_MAX_BYTES`.
    let user_bytes = crate::core::user_context::read_user_context().len();
    blocks.push(ContextBlock {
        name: "user_context".to_string(),
        bytes: user_bytes,
        always: false,
        note: "user-authored and machine-specific; injected on every run, and the one \
               fixed block a user can shrink themselves"
            .to_string(),
    });

    let memory = crate::core::user_context::build_memory_prelude_prompt();
    blocks.push(ContextBlock {
        name: "memory_prelude".to_string(),
        bytes: memory.len(),
        always: true,
        note: "how to reach durable memory; a pointer, not the memories".to_string(),
    });

    // With empty selections these builders return the fixed scaffolding — the part
    // paid even by a run that selected nothing.
    let empty: Vec<String> = Vec::new();
    for (name, bytes) in [
        (
            "skills scaffolding",
            crate::core::skills::build_skills_prompt(&empty).len(),
        ),
        (
            "directives scaffolding",
            crate::core::directives::build_directives_prompt(&empty).len(),
        ),
        (
            "profiles scaffolding",
            crate::core::profiles::build_profiles_prompt(&empty).len(),
        ),
    ] {
        blocks.push(ContextBlock {
            name: name.to_string(),
            bytes,
            always: true,
            note: "fixed framing around a selection; paid even when nothing is selected"
                .to_string(),
        });
    }

    // The selected part. Reported so a caller can see what a choice costs, and
    // deliberately NOT gated: capping it would cap the feature, not the waste.
    for (name, bytes) in [
        (
            "skills (selected)",
            selection_cost(
                crate::core::skills::build_skills_prompt(skill_ids).len(),
                crate::core::skills::build_skills_prompt(&empty).len(),
            ),
        ),
        (
            "directives (selected)",
            selection_cost(
                crate::core::directives::build_directives_prompt(directive_ids).len(),
                crate::core::directives::build_directives_prompt(&empty).len(),
            ),
        ),
        (
            "profiles (selected)",
            selection_cost(
                crate::core::profiles::build_profiles_prompt(profile_ids).len(),
                crate::core::profiles::build_profiles_prompt(&empty).len(),
            ),
        ),
    ] {
        blocks.push(ContextBlock {
            name: name.to_string(),
            bytes,
            always: false,
            note: "scales with what the caller asked for".to_string(),
        });
    }

    let floor_bytes = blocks.iter().filter(|b| b.always).map(|b| b.bytes).sum();
    let total_bytes = blocks.iter().map(|b| b.bytes).sum();
    StaticContextInventory {
        blocks,
        floor_bytes,
        user_bytes,
        total_bytes,
    }
}

/// What a selection added on top of its own scaffolding.
///
/// Saturating: a compact builder can return LESS than its empty form, and a
/// negative cost reported as a huge number would be worse than reporting zero.
fn selection_cost(with_selection: usize, scaffolding: usize) -> usize {
    with_selection.saturating_sub(scaffolding)
}

/// Render the inventory, heaviest first, floor separated from selection.
pub fn render(inventory: &StaticContextInventory) -> String {
    let mut out = String::from("STATIC CONTEXT — what a run pays before working\n\n");

    let mut floor: Vec<&ContextBlock> = inventory.blocks.iter().filter(|b| b.always).collect();
    floor.sort_by_key(|b| std::cmp::Reverse(b.bytes));
    out.push_str("FLOOR (every run, no opt-out):\n");
    for block in &floor {
        out.push_str(&format!("  {:>7} B  {}\n", block.bytes, block.name));
    }
    out.push_str(&format!(
        "  {:>7} B  TOTAL — ceiling {} B, ~{} tokens (estimate)\n",
        inventory.floor_bytes,
        FLOOR_MAX_BYTES,
        inventory.estimated_floor_tokens()
    ));

    let selected: Vec<&ContextBlock> = inventory
        .blocks
        .iter()
        .filter(|b| !b.always && b.bytes > 0)
        .collect();
    if selected.is_empty() {
        // Said explicitly: a run with nothing selected still pays the floor above.
        out.push_str("\nSELECTED: nothing — the floor above is the whole cost.\n");
    } else {
        out.push_str("\nSELECTED (scales with the caller's choices):\n");
        for block in selected {
            out.push_str(&format!("  {:>7} B  {}\n", block.bytes, block.name));
        }
    }

    if let Some(excess) = inventory.over_floor_budget() {
        out.push_str(&format!(
            "\nOVER BUDGET by {excess} B. Move reference material behind a tool the \
             caller invokes; do not raise the ceiling.\n"
        ));
    }
    out
}

#[cfg(test)]
#[path = "static_context_test.rs"]
mod static_context_test;
