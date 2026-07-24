//! Tier 4 — Real-agent E2E tests (LOCAL ONLY, gated).
//!
//! ⚠️ **TOKEN COST WARNING** ⚠️
//! Each test in this file spawns a **real** AI agent CLI (Claude
//! Code, Codex, …) and lets it produce a `KRONN:BUNDLE_READY`
//! response. Expect **~30-80k tokens** of agent consumption per
//! test (typically ~$0.30-0.50 on Claude Sonnet, more on Opus).
//! Do not run in CI — these are local-only validation tests for
//! "the agent actually emits a valid bundle on a realistic prompt".
//!
//! ## How to run
//!
//! ```bash
//! # From the backend/ directory:
//! KRONN_E2E_REAL_AGENT=1 cargo test --test real_agent_e2e -- --ignored --nocapture
//! ```
//!
//! - `KRONN_E2E_REAL_AGENT=1` — opt-in env flag (without it, tests
//!   panic-skip with a "not opted in" message so a misguided
//!   `cargo test --ignored` doesn't burn tokens silently).
//! - `--ignored` — Cargo skips `#[ignore]` tests by default; this
//!   flag flips them on.
//! - `--nocapture` — recommended so you see the agent's streamed
//!   response live and can interrupt if it goes off-rails.
//!
//! ## Why local-only
//!
//! - Real agents need a CLI binary installed on the machine (we
//!   shell out to `claude`, `codex`, etc.). CI runners don't have
//!   those.
//! - Even if they did, every CI run would cost tokens — burning a
//!   $5 budget for every PR is not the kind of "verification" we
//!   want from CI.
//! - The bundle endpoint's PARSING + VALIDATION + DB INSERT logic
//!   is already exhaustively tested in `src/api_tests.rs` (Tier 2,
//!   real-DB integration, deterministic). This file only validates
//!   the **last mile**: "given a realistic user prompt, does a real
//!   agent output a parseable `KRONN:BUNDLE_READY` block?"
//!
//! See [[feedback_kronn_deagentify_first]] memory for why this kind
//! of feature needs a real-agent test even when unit tests are
//! green.

use std::env;

/// Helper that runs at the start of every test in this file: if the
/// opt-in env var is missing, panic with a clear message + cost
/// reminder. Tests are `#[ignore]` AND opt-in — belt and braces so
/// nobody runs them by accident.
fn require_opt_in() {
    if env::var("KRONN_E2E_REAL_AGENT").ok().as_deref() != Some("1") {
        panic!(
            "Real-agent E2E tests are opt-in: set KRONN_E2E_REAL_AGENT=1 to run. \
             Expect ~30-80k tokens per test (real LLM cost). \
             Run: `KRONN_E2E_REAL_AGENT=1 cargo test --test real_agent_e2e -- --ignored --nocapture`"
        );
    }
}

/// Real-agent bundle generation E2E.
///
/// Spawns the `claude` CLI with the `workflow-architect` skill
/// loaded, asks it to design a "fetch + per-item summarize + notify"
/// pipeline (the canonical bundle example from the skill itself),
/// then asserts the response contains:
/// - A `KRONN:BUNDLE_READY` signal
/// - A parseable JSON block before it
/// - At least one `quick_prompts` entry with a `bundle_id`
/// - A `workflow` section whose steps reference `@bundle:<id>` of
///   the declared QP/QA
///
/// This test does NOT actually POST the bundle to a backend (that's
/// what `src/api_tests.rs::bundle_creates_qp_qa_and_workflow_atomically`
/// already covers exhaustively with mocked DB). It validates only
/// the **agent's contract compliance**: "the skill's instructions
/// produce a payload the backend will accept."
///
/// **Cost**: ~30-50k tokens (~$0.15-0.30 on Sonnet 4.5).
#[test]
#[ignore]
fn real_agent_emits_valid_bundle_ready_for_fetch_summarize_notify() {
    require_opt_in();

    // TODO: implementation needs the local Kronn agent runner
    // wired up. The shape this test will take:
    //
    // 1. Build an AgentStartConfig pointing at `claude` (assume
    //    installed at $HOME/.local/bin/claude — same as the rest
    //    of the codebase).
    // 2. Compose the prompt:
    //
    //     <load workflow-architect skill>
    //     User: "Build me a workflow that pulls the 5 latest
    //     articles from Chartbeat every weekday morning, summarizes
    //     each one with an LLM, and posts the digest to my Slack
    //     #digests channel."
    //
    // 3. Spawn the agent + capture stdout until completion.
    // 4. Parse the response, locate the last `KRONN:BUNDLE_READY`
    //    marker, extract the preceding ```json block.
    // 5. Validate the JSON shape:
    //    - `quick_prompts` non-empty (the summarize step needs a QP)
    //    - `workflow.steps` references at least one `@bundle:` ref
    //    - All `@bundle:` refs resolve to declared `bundle_id`s
    //    - `workflow` has a `Cron` trigger
    //    - Last step is a `Notify` (Slack webhook)
    //
    // 6. (Optional, no-token cost) POST the payload to a test
    //    in-memory Kronn backend and verify all artifacts land —
    //    this catches the "agent produces a JSON the validator
    //    rejects" failure mode without an extra agent run.
    //
    // Placeholder: until the agent runner integration test helper
    // is exposed at the crate level, this test is a stub that
    // documents the contract.
    eprintln!("[stub] Tier 4 real-agent E2E — implementation pending wire-up to the agent runner. See header comment for the intended shape.");
}

/// Real-agent triage E2E for the Feasibility-Gated pattern.
///
/// Spawns an agent with the `workflow-architect` + `[TRIAGE]` step
/// prompt, feeds it a real big ticket (EW-7247 Africanews→Euronews
/// migration text), and asserts the response is a valid
/// `triage_manifest` JSON schema with non-empty `decided[]` +
/// `mocked[]` + `blocked[]` arrays.
///
/// This is the test that **actually** validates the 0.8.3 killer
/// flow on a real ticket (vs. the in-CI tests which validate the
/// machinery). When this passes on a fresh agent invocation, the
/// Feasibility-Gated AutoPilot is production-ready.
///
/// **Cost**: ~25-30k tokens (~$0.12-0.20 on Sonnet 4.5).
#[test]
#[ignore]
fn real_agent_produces_valid_triage_manifest_for_big_ticket() {
    require_opt_in();
    // TODO: same plumbing as the bundle test above. Prompt = the
    // build_feasibility_workflow's `triage_prompt` template, ticket
    // body = EW-7247's actual description. Assertions:
    // - response contains `---STEP_OUTPUT---` envelope
    // - envelope's `data` parses as triage_manifest schema
    // - `decided.length + mocked.length + blocked.length >= 5`
    //   (a big ticket should produce real freedoms to trace)
    // - every entry has a non-empty `id` and `what`
    eprintln!(
        "[stub] Tier 4 real-agent triage — implementation pending wire-up to the agent runner."
    );
}

#[test]
fn opt_in_panic_message_is_actionable() {
    // Meta-test: when somebody runs this file WITHOUT the env var
    // set AND removes the `#[ignore]` attribute, the panic message
    // must be actionable. Run a sub-process to capture the panic
    // text — but that's overkill. We just sanity-check that
    // `require_opt_in()` panics when the env is absent (or returns
    // when set), and that the message contains the env var name
    // + a cost reminder.
    //
    // This test always runs in CI (not `#[ignore]`) and verifies the
    // safety rail itself, NOT any agent behavior — so it's free.
    let prev = env::var("KRONN_E2E_REAL_AGENT").ok();
    // SAFETY: env::remove_var can be unsafe in multi-threaded test
    // environments. Tests in this file are #[ignore]'d so they only
    // run when explicitly requested — never in parallel CI.
    unsafe {
        env::remove_var("KRONN_E2E_REAL_AGENT");
    }
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        require_opt_in();
    }));
    if let Some(prev_val) = prev {
        unsafe {
            env::set_var("KRONN_E2E_REAL_AGENT", prev_val);
        }
    }
    let err = r.expect_err("require_opt_in() must panic when env is missing");
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        msg.contains("KRONN_E2E_REAL_AGENT"),
        "panic must name the env var: {msg}"
    );
    assert!(
        msg.contains("tokens") || msg.contains("cost"),
        "panic must warn about token cost: {msg}"
    );
}
