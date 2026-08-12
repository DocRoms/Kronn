//! Tests for the static-context inventory — KT-192 DoD 3.
//!
//! This is a budget, so the failure that matters is the one where it reports a
//! smaller floor than reality: a block forgotten, a selected cost counted as
//! fixed, or a ceiling with slack that lets the floor grow for free.

use super::*;

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

// ── the floor is what nobody can opt out of ─────────────────────────

#[test]
fn the_floor_is_measured_with_nothing_selected() {
    // The case no caller ever sees and every caller pays.
    let inventory = inventory(&[], &[], &[]);
    assert!(inventory.floor_bytes > 0, "the floor came out empty");
    assert_eq!(
        inventory.total_bytes,
        inventory.floor_bytes + inventory.user_bytes,
        "a selection cost leaked into a run that selected nothing"
    );
}

#[test]
fn the_gated_floor_excludes_the_user_context() {
    // A ceiling that moved with whoever wrote the most notes would pass on a bare
    // CI runner and fail on a real machine. The user's context is injected on every
    // run and reported as such — it is simply not what the ceiling gates on.
    let inventory = inventory(&[], &[], &[]);
    let user = inventory
        .blocks
        .iter()
        .find(|b| b.name == "user_context")
        .expect("user context vanished from the inventory");
    assert!(!user.always, "the gated floor became machine-specific");
    assert_eq!(inventory.user_bytes, user.bytes);
}

#[test]
fn the_floor_stays_within_its_ceiling() {
    // The ratchet. If this fails, the fix is to move material behind a tool — not
    // to raise FLOOR_MAX_BYTES.
    let inventory = inventory(&[], &[], &[]);
    assert!(
        inventory.over_floor_budget().is_none(),
        "floor is {} B, ceiling {FLOOR_MAX_BYTES} B — {}",
        inventory.floor_bytes,
        render(&inventory)
    );
}

#[test]
fn the_ceiling_has_no_slack_in_it() {
    // A ceiling with room to spare is not a ratchet: the floor could grow for free
    // until it hit the slack. This asserts the pinned value is close to the real
    // measurement, so growth is caught early rather than eventually.
    let inventory = inventory(&[], &[], &[]);
    assert_eq!(
        inventory.floor_bytes, FLOOR_MAX_BYTES,
        "the ceiling is not pinned to the measurement — {} B measured against a \
         {FLOOR_MAX_BYTES} B ceiling is headroom, not a ratchet",
        inventory.floor_bytes
    );
}

#[test]
fn every_floor_block_is_named_and_justified() {
    // A byte count with no name cannot be argued with, and a block with no reason
    // is the one nobody dares delete.
    let inventory = inventory(&[], &[], &[]);
    let floor: Vec<&ContextBlock> = inventory.blocks.iter().filter(|b| b.always).collect();
    assert!(floor.len() >= 4, "only {} floor blocks found", floor.len());
    for block in floor {
        assert!(!block.name.is_empty());
        assert!(
            block.note.len() > 20,
            "`{}` has no usable justification",
            block.name
        );
    }
}

#[test]
fn the_anti_hallucination_preamble_is_counted() {
    // It is the largest thing Kronn prepends unconditionally. A budget that missed
    // it would understate the floor while looking complete.
    let inventory = inventory(&[], &[], &[]);
    let preamble = inventory
        .blocks
        .iter()
        .find(|b| b.name.contains("PREAMBLE"))
        .expect("the preamble is not in the inventory");
    assert!(preamble.always);
    assert_eq!(preamble.bytes, crate::core::anti_halluc::PREAMBLE.len());
}

// ── selection is reported, not gated ────────────────────────────────

#[test]
fn a_selection_is_reported_as_not_always() {
    // Capping the selected part would cap the feature rather than the waste, so it
    // must be visibly distinct from the floor.
    let inventory = inventory(&ids(&["rust"]), &[], &[]);
    let selected: Vec<&ContextBlock> = inventory.blocks.iter().filter(|b| !b.always).collect();
    assert!(!selected.is_empty());
    assert!(selected.iter().all(|b| !b.always));
}

#[test]
fn selecting_a_skill_does_not_change_the_floor() {
    // THE separation. If a selection moved the floor, the ceiling would fail for
    // callers who asked for more rather than for real growth in the fixed cost.
    let bare = inventory(&[], &[], &[]);
    let loaded = inventory(&ids(&["rust", "typescript"]), &[], &[]);
    assert_eq!(
        bare.floor_bytes, loaded.floor_bytes,
        "a selection inflated the floor"
    );
    assert!(
        loaded.total_bytes >= bare.total_bytes,
        "selecting skills reduced the total"
    );
}

#[test]
fn a_real_skill_selection_costs_something() {
    // Otherwise the report would say a selection is free, and nobody would ever
    // question a long list of them.
    let inventory = inventory(&ids(&["rust"]), &[], &[]);
    let skills = inventory
        .blocks
        .iter()
        .find(|b| b.name == "skills (selected)")
        .expect("no selected-skills block");
    assert!(
        skills.bytes > 0,
        "selecting a builtin skill was reported as free"
    );
}

#[test]
fn an_unknown_selection_is_not_reported_as_a_cost() {
    // A skill id that resolves to nothing adds nothing. Reporting a cost for it
    // would invent bytes.
    let inventory = inventory(&ids(&["definitely-not-a-skill"]), &[], &[]);
    let skills = inventory
        .blocks
        .iter()
        .find(|b| b.name == "skills (selected)")
        .unwrap();
    assert_eq!(skills.bytes, 0);
}

#[test]
fn a_compact_builder_returning_less_than_its_scaffolding_reports_zero_not_a_huge_number() {
    // `saturating_sub` guards this: an underflow would print a cost near
    // `usize::MAX` and make the whole report unreadable.
    assert_eq!(selection_cost(10, 400), 0);
    assert_eq!(selection_cost(400, 10), 390);
}

// ── the rendered report ─────────────────────────────────────────────

#[test]
fn the_report_separates_the_floor_from_the_selection() {
    let text = render(&inventory(&ids(&["rust"]), &[], &[]));
    let floor = text.find("FLOOR").expect("no floor section");
    let selected = text.find("SELECTED").expect("no selected section");
    assert!(floor < selected, "the floor must be read first");

    // Position alone is not the property: a report that listed every block under
    // FLOOR would still pass the ordering check while claiming a chosen cost is
    // unavoidable. Verified by a control that did exactly that.
    let floor_section = &text[floor..selected];
    for chosen in ["skills (selected)", "user_context"] {
        assert!(
            !floor_section.contains(chosen),
            "`{chosen}` is a chosen cost and appeared under FLOOR:\n{floor_section}"
        );
    }
}

#[test]
fn a_run_with_nothing_selected_says_so_rather_than_showing_an_empty_section() {
    // An empty section reads as missing data. Here it is a fact: the floor is the
    // whole cost.
    let text = render(&inventory(&[], &[], &[]));
    assert!(text.contains("nothing"), "got: {text}");
    assert!(text.contains("whole cost"));
}

#[test]
fn the_report_states_the_ceiling_next_to_the_measurement() {
    // A number with no budget beside it invites no decision.
    let text = render(&inventory(&[], &[], &[]));
    assert!(text.contains(&FLOOR_MAX_BYTES.to_string()));
}

#[test]
fn going_over_budget_says_what_to_do_instead_of_raising_the_ceiling() {
    // The instruction matters more than the number: raising a ceiling to go green
    // is how the 84 KiB instruction file happened.
    let mut inventory = inventory(&[], &[], &[]);
    inventory.floor_bytes = FLOOR_MAX_BYTES + 1_000;
    let text = render(&inventory);
    assert!(text.contains("OVER BUDGET"));
    assert!(text.contains("do not raise the ceiling"));
}

#[test]
fn token_figures_are_estimates_and_never_the_gate() {
    // The gate is bytes. A ceiling resting on an approximation would move with the
    // model.
    let inventory = inventory(&[], &[], &[]);
    assert_eq!(
        inventory.estimated_floor_tokens(),
        (inventory.floor_bytes as f64 / BYTES_PER_TOKEN) as usize
    );
    // over_floor_budget compares BYTES, not tokens.
    assert_eq!(inventory.over_floor_budget(), None);
}
