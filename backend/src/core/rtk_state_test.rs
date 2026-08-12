//! Tests for the RTK adoption state — KT-197.
//!
//! Fixtures are the REAL outputs of rtk 0.42.4 on this machine, including the two
//! failures observed there. That matters more than usual here: the whole value of
//! this module is turning "cc-economics failed" into something someone can act on,
//! and a diagnosis written against an imagined error message is a guess.

use super::*;

// Verbatim from `rtk gain`.
const GAIN: &str = "RTK Token Savings (Global Scope)
════════════════════════════════════════════════════════════

Total commands:    47035
Input tokens:      114.3M
Output tokens:     39.7M
Tokens saved:      74.7M (65.4%)
Total exec time:   5768m43s (avg 7.4s)
";

// Verbatim from `rtk session`.
const SESSION: &str = "RTK Session Overview (last 10)
----------------------------------------------------------------------
Session      Date          Cmds   RTK  Adoption           Output
----------------------------------------------------------------------
9547c757     Today         4929  1803       37% @@...       1.9M
7cb942ba     Today            2     1       50% @@@..        198
----------------------------------------------------------------------
Average adoption: 37%
Tip: Run `rtk discover` to find missed RTK opportunities
";

// Verbatim from `rtk hook-audit` with no log.
const HOOK_AUDIT_MISSING: &str = "No audit log found at /Users/x/.local/share/rtk/hook-audit.log
Enable audit mode: export RTK_HOOK_AUDIT=1 in your shell, then use Claude Code.
";

// Verbatim from `rtk cc-economics` against the current ccusage.
const CC_DRIFT: &str = "rtk: Failed to fetch ccusage monthly data: Failed to parse \
ccusage JSON output: Invalid JSON structure for monthly data: missing field `month` \
at line 85 column 5";

const CC_NPX: &str = "[info] ccusage not installed globally, fetching via npx...";

// ── the sources that answer ─────────────────────────────────────────

#[test]
fn gain_reports_savings_against_a_command_count() {
    let state = classify(RtkSource::Gain, GAIN, "", true);
    let SourceState::Ready { summary, metrics } = state else {
        panic!("gain was not readable: {state:?}");
    };
    assert!(summary.contains("74.7M"), "got: {summary}");
    assert!(summary.contains("47035"));
    assert_eq!(metrics.len(), 2);
}

#[test]
fn session_reports_adoption_and_how_many_sessions_it_covers() {
    // A percentage with no denominator is not usable: 37% of two sessions and 37%
    // of two hundred are different facts.
    let state = classify(RtkSource::Session, SESSION, "", true);
    let SourceState::Ready { summary, .. } = state else {
        panic!("session was not readable: {state:?}");
    };
    assert!(summary.contains("37%"), "got: {summary}");
    assert!(summary.contains("2 session"), "got: {summary}");
}

#[test]
fn discover_finding_nothing_is_a_real_answer() {
    // "Nothing missed" and "could not look" must not collapse into one state.
    let state = classify(
        RtkSource::Discover,
        "No missed opportunities found",
        "",
        true,
    );
    let SourceState::Ready { metrics, .. } = state else {
        panic!("a clean discover was treated as a failure: {state:?}");
    };
    assert_eq!(metrics[0].value, "0");
}

// ── the sources that cannot answer say why AND what to do ────────────

#[test]
fn every_unavailable_state_carries_a_remedy() {
    // A diagnosis with no remedy leaves a reader informed and stuck. This holds
    // for all five sources at once, so a new blocker cannot be added without one.
    let cases = [
        classify(RtkSource::HookAudit, HOOK_AUDIT_MISSING, "", true),
        classify(RtkSource::CcEconomics, "", CC_DRIFT, true),
        classify(RtkSource::CcEconomics, CC_NPX, "", true),
        classify(RtkSource::Gain, "unrecognised output", "", true),
        classify(RtkSource::Session, "no figures here", "", true),
        classify(RtkSource::Discover, "", "", false),
    ];
    for state in cases {
        if let SourceState::Unavailable { diagnosis, remedy } = &state {
            assert!(diagnosis.len() > 20, "thin diagnosis: {diagnosis}");
            assert!(remedy.len() > 20, "thin remedy: {remedy}");
        } else {
            panic!("expected Unavailable, got {state:?}");
        }
    }
}

#[test]
fn the_ccusage_drift_names_the_field_that_moved() {
    // "cc-economics failed" sends someone reading rtk's source. Naming the field
    // sends them to ccusage's release notes, which is where the change is.
    let state = classify(RtkSource::CcEconomics, "", CC_DRIFT, true);
    let SourceState::Unavailable { diagnosis, remedy } = state else {
        panic!("the drift was treated as success");
    };
    assert!(
        diagnosis.contains("month"),
        "the field is not named: {diagnosis}"
    );
    assert!(diagnosis.contains("ccusage"));
    // And it says what still works, so the failure does not read as total.
    assert!(remedy.contains("rtk gain"), "got: {remedy}");
}

#[test]
fn a_drift_with_no_named_field_still_produces_a_usable_diagnosis() {
    // Forward compatibility: a future rtk may word the error differently. The
    // fallback must not be an empty string presented as an explanation.
    let state = classify(
        RtkSource::CcEconomics,
        "",
        "rtk: Failed to parse ccusage JSON output: something else entirely",
        true,
    );
    let SourceState::Unavailable { diagnosis, .. } = state else {
        panic!("expected Unavailable");
    };
    assert!(diagnosis.contains("ccusage"));
    assert!(diagnosis.len() > 20);
}

#[test]
fn the_missing_hook_log_is_reported_as_a_switch_not_a_fault() {
    // Nothing is broken: auditing is off. A remedy phrased as a fix would send
    // someone debugging a working tool.
    let state = classify(RtkSource::HookAudit, HOOK_AUDIT_MISSING, "", true);
    let SourceState::Unavailable { remedy, .. } = state else {
        panic!("expected Unavailable");
    };
    assert!(remedy.contains("RTK_HOOK_AUDIT=1"), "got: {remedy}");
}

#[test]
fn reaching_for_a_package_runner_is_itself_reported() {
    // It cost 17 seconds on this machine, and in a worktree a package runner can
    // rewrite the main checkout's node_modules. Worth naming, not tolerating.
    let state = classify(RtkSource::CcEconomics, CC_NPX, "", true);
    let SourceState::Unavailable { diagnosis, remedy } = state else {
        panic!("expected Unavailable");
    };
    assert!(diagnosis.contains("not installed"));
    assert!(remedy.contains("node_modules"), "got: {remedy}");
}

#[test]
fn a_command_that_never_ran_is_unavailable_not_zero() {
    // THE rule this module shares with the rest of the release: an unmeasured
    // adoption rate is not a good one.
    let state = classify(RtkSource::Gain, "", "", false);
    assert!(matches!(state, SourceState::Unavailable { .. }));
}

#[test]
fn an_output_that_says_nothing_is_not_parsed_as_zero() {
    // `rtk gain` exiting 0 with an unrecognised format would otherwise yield
    // "0 saved over 0 commands", which reads as RTK doing nothing.
    let state = classify(RtkSource::Gain, "RTK Token Savings\n\n", "", true);
    assert!(
        matches!(state, SourceState::Unavailable { .. }),
        "an unparsed output became a figure: {state:?}"
    );
}

// ── the rendered panel ──────────────────────────────────────────────

fn state_of(blocked: usize) -> RtkState {
    let ready = classify(RtkSource::Gain, GAIN, "", true);
    let stuck = classify(RtkSource::HookAudit, HOOK_AUDIT_MISSING, "", true);
    RtkState {
        gain: if blocked > 0 {
            stuck.clone()
        } else {
            ready.clone()
        },
        session: if blocked > 1 {
            stuck.clone()
        } else {
            ready.clone()
        },
        discover: if blocked > 2 {
            stuck.clone()
        } else {
            ready.clone()
        },
        hook_audit: if blocked > 3 {
            stuck.clone()
        } else {
            ready.clone()
        },
        cc_economics: if blocked > 4 { stuck } else { ready },
    }
}

#[test]
fn what_is_not_measured_comes_first() {
    // It is the actionable half. Putting the healthy figures first would let a
    // truncation drop exactly the part someone can act on.
    let text = render(&state_of(2));
    let not_measured = text.find("NOT MEASURED").expect("no blocked section");
    let measured = text.find("MEASURED:").expect("no measured section");
    assert!(not_measured < measured, "the blocked section came second");
}

#[test]
fn a_state_with_nothing_readable_says_adoption_is_unknown() {
    // An empty panel reads as "adoption is fine", which is the failure mode.
    let text = render(&state_of(5));
    assert!(text.contains("UNKNOWN"), "got: {text}");
}

#[test]
fn a_fully_healthy_state_carries_no_blocked_section() {
    // Otherwise the section appears always and stops meaning anything.
    let text = render(&state_of(0));
    assert!(!text.contains("NOT MEASURED"));
    assert!(text.contains("MEASURED:"));
}

#[test]
fn the_panel_stays_within_its_budget() {
    let mut state = state_of(5);
    let long = SourceState::Unavailable {
        diagnosis: "d".repeat(3_000),
        remedy: "r".repeat(3_000),
    };
    state.gain = long.clone();
    state.session = long.clone();
    state.discover = long;
    let text = render(&state);
    assert!(
        text.len() <= RTK_STATE_MAX_BYTES + 32,
        "panel was {} bytes",
        text.len()
    );
    assert!(text.contains("truncated"), "the cap was never reached");
}

#[test]
fn every_source_maps_to_a_template() {
    // The state is collected through Quick Exec, so a source with no template
    // could never be filled.
    for source in [
        RtkSource::Gain,
        RtkSource::Session,
        RtkSource::Discover,
        RtkSource::HookAudit,
        RtkSource::CcEconomics,
    ] {
        let id = source.template_id();
        assert!(
            crate::core::quick_exec_templates::template(id).is_some(),
            "`{id}` is not in the Quick Exec catalogue"
        );
    }
}
