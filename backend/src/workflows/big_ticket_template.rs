//! Feasibility-Gated big-ticket AutoPilot template — 0.8.3.
//!
//! Returns a `CreateWorkflowRequest` ready to POST to `/api/workflows`.
//! **7 steps with mixed primitives** so token cost is paid only where
//! genuine reasoning is needed (triage, implement, pr_draft) — the rest
//! is deterministic code (ApiCall/JsonData, Exec, Gate). See
//! [[feedback_kronn_deagentify_first]] memory for the rule.
//!
//! 1. `fetch_issue` — **JsonData** seed with the ticket body. The
//!    frontend wizard upgrades this to **ApiCall** at create time when
//!    a tracker plugin (Jira/GitHub/GitLab) is wired for the project —
//!    same pattern as `ticket-to-pr` preset. 0 tokens either way.
//! 2. `triage` — **Agent** + TypedSchema(triage_manifest,
//!    on_invalid=Fail). `[TRIAGE]` description marker → runner injects
//!    the "audit, don't code" addendum. Reads
//!    `{{steps.fetch_issue.data}}`.
//! 3. `review_triage` — **Gate**. Renders the manifest for human
//!    review. `gate_request_changes_target: "triage"` lets the
//!    operator re-loop.
//! 4. `implement` — **Agent**. Prompt references
//!    `{{steps.triage.data.decided}}` / `.mocked` / `.blocked` so the
//!    agent is constrained by the validated manifest. Inserts
//!    `KRONN-*` markers per entry. `[SIGNAL: BLOCKED <id>]` →
//!    `Goto(triage)` if reality diverges.
//! 5. `run_tests` — **Exec**. Generic auto-detect (Make / Cargo /
//!    pnpm-yarn-npm / composer / pytest). 0 tokens, real verdict.
//!    `ERROR` → `Goto(implement)`, max 2 iterations.
//! 6. `drift_check` — **Exec**. `grep KRONN-(ASSUMED|MOCKED|TODO)` on
//!    the worktree. 0 tokens, surfaces every freedom the agent took
//!    in the code. The pr_draft step embeds this output verbatim.
//! 7. `pr_draft` — **Agent**. PR body = manifest checklist + open
//!    items + drift_check output.
//!
//! Loop budget: `loop_detection_max_revisits: 5` so an operator can
//! RequestChanges up to 5 times. Implement's `BLOCKED → Goto(triage)`
//! is itself capped at 3 iterations.

use crate::models::{
    AgentSettings, AgentType, ConditionAction, CreateWorkflowRequest, ModelTier, StepConditionRule,
    StepMode, StepOutputFormat, StepType, WorkflowGuards, WorkflowStep, WorkflowTrigger,
};
use std::collections::HashMap;

/// Parameters the user supplies when instantiating the template.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct FeasibilityWorkflowParams {
    pub project_id: Option<String>,
    /// Ticket key (Jira) or number (GitHub) to mention in prompts.
    pub ticket_ref: Option<String>,
    /// Ticket body / description. Seeded into `fetch_issue` JsonData
    /// step. When None, the fixture stays empty (`{{ticket_body}}` as
    /// a launch-time variable can be wired by the wizard).
    pub ticket_body: Option<String>,
    /// Agent to run the three Agent steps with. Defaults to ClaudeCode.
    #[serde(default)]
    pub agent: Option<AgentType>,
    /// Workflow name override. Defaults to
    /// `Big-ticket AutoPilot (<ticket_ref or 'manual'>)`.
    #[serde(default)]
    pub name: Option<String>,
    /// 2026-06-12 Phase 3b — when true, the child sub-workflow runs ONCE PER
    /// SUB-TASK of the triage manifest (`sub_workflow_foreach_file` over
    /// `.kronn/tasks.json`), each implement seeing ONLY its task slice via
    /// `.kronn/current_task.json` (scoped context: fewer tokens, more
    /// deterministic, one commit per task). false = monolithic implement
    /// (Phase 1/2 behaviour) — kept as the A/B baseline.
    #[serde(default)]
    pub decomposed: bool,
    /// 2026-06-12 — agent for the `plan_review` step. Defaults to Codex:
    /// a DIFFERENT model family reviewing the plan avoids same-model blind
    /// spots (user decision: triage = Claude Reasoning, review = Codex,
    /// execution = cheap tiers).
    #[serde(default)]
    pub reviewer_agent: Option<AgentType>,
}

/// Common shell allowlist for the Exec steps (`run_tests` +
/// `drift_check`). Mirror of the `ticket-to-pr` preset list, plus the
/// `grep` we need for `drift_check`. `bash` is the entry point for the
/// auto-detect script.
const EXEC_ALLOWLIST: &[&str] = &[
    "bash", "cargo", "pnpm", "npm", "yarn", "pytest", "make", "composer", "grep", "git",
];

/// Build a `CreateWorkflowRequest` for the Feasibility-Gated
/// Implementation pattern. Frontend CTA path:
/// `onLaunchWorkflowFromPreset('feasibility-autopilot', projectId)`
/// → wizard → POST `/api/workflows`.
pub fn build_feasibility_workflow(params: FeasibilityWorkflowParams) -> CreateWorkflowRequest {
    let agent = params.agent.unwrap_or(AgentType::ClaudeCode);
    let ticket_ref = params
        .ticket_ref
        .clone()
        .unwrap_or_else(|| "{{ticket_ref}}".into());
    let ticket_body = params
        .ticket_body
        .clone()
        .unwrap_or_else(|| "{{ticket_body}}".into());

    // 2026-06-11 (PR-C) — DECOMPOSED. The parent keeps the human-gated
    // triage; the implement→run_tests→drift_check loop is a CHILD workflow
    // (`build_feasibility_child`) that shares the parent's worktree (Phase 2
    // handoff). `sub_workflow_id` is `None` here — the endpoint creates the
    // child first and patches it before creating the parent.
    // 2026-06-12 (#2) — a parent-level drift_check AFTER the sub-workflow:
    // in foreach mode the envelope's last_output is the last child's commit
    // noise, so the markers table for the PR must be regenerated over the
    // FINAL worktree (all tasks merged) — deterministic, 0 token.
    let steps = vec![
        build_fetch_issue_step(&ticket_ref, &ticket_body),
        // « Deux cerveaux » (2026-06-13) — the plan is now DEBATED inside the
        // triage step: after triage emits the manifest, a second agent (the
        // reviewer) challenges it in a shared transcript until consensus. This
        // REPLACES the old `plan_review → Goto(triage)` file-relay loop (which
        // re-read everything from scratch each round). Cheaper + a real
        // back-and-forth. The deterministic plan_lint stays as a 0-token guard.
        build_triage_step(agent.clone(), &ticket_ref, params.reviewer_agent.clone().unwrap_or(AgentType::Codex)),
        build_plan_lint_step(),
        build_gate_step(),
        // run-14 finding: capture the tests ALREADY red on the approved base
        // (pre-existing repo debt) ONCE, so per-task item_tests only loops to
        // implement on NET-NEW failures — not on the repo's own broken tests
        // (which wasted ~104k tokens making agents chase a timezone bug).
        build_test_baseline_step(),
        build_feasibility_impl_step(params.decomposed),
        // Read-only FULL-suite integration verdict over the merged worktree
        // (the per-task test→fix loop lives in the CHILD now, 2026-06-13).
        // Catches cross-item regressions the per-item scoped tests miss; never
        // loops/fixes here — just documents the verdict for the PR.
        build_run_tests_step(),
        build_drift_check_step(),
        build_pr_draft_step(agent, &ticket_ref),
    ];

    let workflow_name = params.name.unwrap_or_else(|| {
        let key = params.ticket_ref.unwrap_or_else(|| "manual".into());
        format!("Big-ticket AutoPilot ({key})")
    });

    CreateWorkflowRequest {
        name: workflow_name,
        project_id: params.project_id,
        trigger: WorkflowTrigger::Manual,
        steps,
        actions: vec![],
        safety: None,
        workspace_config: None,
        concurrency_limit: None,
        guards: Some(WorkflowGuards {
            // run-14 finding (2026-06-13): a big-ticket fan-out over 26 items
            // (+ debate + human gate wait) ran ~3h and the DEFAULT 2h guard
            // killed it AFTER the fan-out but BEFORE run_tests/drift/pr_draft —
            // losing the test verdict + PR. This workflow is inherently long;
            // give it an explicit 8h wall-clock budget. (Child runs never
            // approach it — they're bounded by per-step stall timeouts.)
            timeout_seconds: Some(28800),
            max_llm_calls: None,
            loop_detection_max_revisits: Some(5),
        }),
        artifacts: HashMap::new(),
        on_failure: vec![],
        // Parent runs ONE Exec (the final drift_check over the merged
        // worktree) — bash + grep only. The child carries the full list.
        exec_allowlist: vec!["bash".into(), "grep".into()],
        variables: vec![],
        enabled: None,
    }
}

/// 2026-06-11 (PR-C) — the CHILD workflow for the decomposed feasibility
/// AutoPilot: `implement → run_tests → drift_check`, run as a sub-workflow
/// sharing the parent's worktree. No Gate (forbidden in a child); the human
/// gate stays in the parent. The endpoint creates this FIRST, then references
/// its id from the parent's `feasibility_impl` SubWorkflow step.
pub fn build_feasibility_child(
    agent: AgentType,
    ticket_ref: &str,
    project_id: Option<String>,
    parent_name: &str,
    decomposed: bool,
) -> CreateWorkflowRequest {
    let implement = if decomposed {
        build_implement_task_step(agent, ticket_ref)
    } else {
        build_implement_step(agent, ticket_ref)
    };
    CreateWorkflowRequest {
        name: if decomposed {
            format!("{parent_name} — implement & verify (per-task)")
        } else {
            format!("{parent_name} — implement & verify")
        },
        project_id,
        trigger: WorkflowTrigger::Manual,
        steps: vec![
            implement,
            build_item_tests_step(),
            build_scope_check_step(),
            build_completeness_check_step(),
            build_commit_step(),
        ],
        actions: vec![],
        safety: None,
        workspace_config: None,
        concurrency_limit: None,
        guards: Some(WorkflowGuards {
            // run-14 finding (2026-06-13): a big-ticket fan-out over 26 items
            // (+ debate + human gate wait) ran ~3h and the DEFAULT 2h guard
            // killed it AFTER the fan-out but BEFORE run_tests/drift/pr_draft —
            // losing the test verdict + PR. This workflow is inherently long;
            // give it an explicit 8h wall-clock budget. (Child runs never
            // approach it — they're bounded by per-step stall timeouts.)
            timeout_seconds: Some(28800),
            max_llm_calls: None,
            loop_detection_max_revisits: Some(5),
        }),
        artifacts: HashMap::new(),
        on_failure: vec![],
        exec_allowlist: EXEC_ALLOWLIST.iter().map(|s| s.to_string()).collect(),
        variables: vec![],
        enabled: None,
    }
}

/// 4. `feasibility_impl` — **SubWorkflow**. Runs the implement/test/drift
///    child. On child failure (tests red after retries, or a hard block that
///    fails the run) → `SUBWF_FAILED` → re-triage at the parent (cap 3),
///    reconstructing the old `BLOCKED → Goto(triage)` across the boundary.
///    `sub_workflow_id` is filled by the endpoint after the child is created.
fn build_feasibility_impl_step(decomposed: bool) -> WorkflowStep {
    let mut s = blank_step("feasibility_impl", StepType::SubWorkflow, AgentType::ClaudeCode);
    if decomposed {
        // Phase 3b — fan-out: one child run per implementable sub-task of the
        // manifest. Triage writes `.kronn/tasks.json`; each iteration gets its
        // slice via `.kronn/current_task.json`. One commit per task.
        s.sub_workflow_foreach_file = Some(".kronn/tasks.json".into());
    }
    s.description = Some(
        "Implement → run_tests → drift_check loop (child sub-workflow sharing the parent worktree). Reads the approved manifest from .kronn/triage-manifest.md, logs deviations to .kronn/decisions.md.".into(),
    );
    s.on_result = vec![StepConditionRule {
        contains: "SUBWF_FAILED".into(),
        action: ConditionAction::Goto {
            step_name: "triage".into(),
            max_iterations: Some(3),
        },
    }];
    s
}

/// Standard retry for the AutoPilot's Agent steps. run-10 (2026-06-13)
/// proved the failure mode is the provider rate-limit / transient blip:
/// `implement` (4 fan-out items) AND the final `pr_draft` all exited 1 with
/// no output and — with NO retry — died permanently, losing the whole run at
/// the last step. 2 retries × the rate-limit-aware backoff (15s/30s) lets a
/// per-minute limit clear. A sustained account-wide exhaustion still fails
/// (correctly), but transient blips no longer throw away an 800k-token run.
fn agent_retry() -> Option<crate::models::workflows::RetryConfig> {
    Some(crate::models::workflows::RetryConfig { max_retries: 2, backoff: "exponential".into() })
}

fn blank_agent_settings() -> AgentSettings {
    AgentSettings {
        model: None,
        tier: None,
        reasoning_effort: None,
        max_tokens: None,
    }
}

/// Minimal `WorkflowStep` builder filling all the `None`/`vec![]`
/// fields so the call sites stay focused on the few that matter.
/// Keeps the template's intent legible without 50+ lines per step.
fn blank_step(name: &str, kind: StepType, agent: AgentType) -> WorkflowStep {
    WorkflowStep {
        name: name.into(),
        step_type: kind,
        description: None,
        agent,
        prompt_template: String::new(),
        mode: StepMode::Normal,
        output_format: StepOutputFormat::FreeText,
        mcp_config_ids: vec![],
        agent_settings: None,
        on_result: vec![],
        on_timeout: None,
        stall_timeout_secs: None,
        retry: None,
        delay_after_secs: None,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        batch_quick_prompt_id: None,
        batch_items_from: None,
        batch_wait_for_completion: None,
        batch_max_items: None,
        batch_workspace_mode: None,
        batch_chain_prompt_ids: vec![],
        batch_concurrent_limit: None,
        quick_api_id: None,
        notify_config: None,
        api_plugin_slug: None,
        api_config_id: None,
        api_endpoint_path: None,
        api_method: None,
        api_path_params: None,
        api_query: None,
        api_headers: None,
        api_body: None,
        api_extract: None,
        api_pagination: None,
        api_timeout_ms: None,
        api_max_retries: None,
        api_output_var: None,
        gate_message: None,
        gate_request_changes_target: None,
        gate_notify_url: None,
        gate_checkpoint_before: None,
        gate_auto_approve_after_secs: None,
        exec_command: None,
        exec_args: vec![],
        exec_timeout_secs: None,
        exec_setup_command: None,
        exec_setup_args: vec![],
        exec_stdin: None,
        quick_prompt_id: None,
        json_data_payload: None,
        sub_workflow_id: None,
        sub_workflow_foreach_file: None,
        multi_agent_review: None,
    }
}

/// 1. `fetch_issue` — **JsonData** with the ticket as fixture.
///    The frontend wizard transforms this to ApiCall when a tracker plugin
///    is wired (mirror of `ticket-to-pr`'s `fetch_issue` upgrade path,
///    see `WorkflowWizard.tsx:290-317`).
fn build_fetch_issue_step(ticket_ref: &str, ticket_body: &str) -> WorkflowStep {
    let mut s = blank_step(
        "fetch_issue",
        StepType::JsonData,
        AgentType::ClaudeCode, // unused for JsonData; field is required
    );
    s.description = Some(
        "Source du ticket. JsonData fixture par défaut, swap en ApiCall (+ quick_api_id) quand un plugin tracker est actif."
            .into(),
    );
    s.output_format = StepOutputFormat::Structured;
    // The shape mirrors what the ApiCall extract would emit so the
    // triage prompt can read `{{steps.fetch_issue.data.key}}` /
    // `.body` uniformly whether the step is JsonData or ApiCall.
    s.json_data_payload = Some(serde_json::json!({
        "key": ticket_ref,
        "body": ticket_body,
    }));
    s
}

/// 2. `triage` — **Agent** + TypedSchema.
///    The `[TRIAGE]` description marker triggers `triage::is_triage_step()`
///    in the runner, which appends the "audit, don't code" addendum.
fn build_triage_step(agent: AgentType, ticket_ref: &str, reviewer: AgentType) -> WorkflowStep {
    let prompt = format!(
        "You are the TRIAGE step of a Feasibility-Gated AutoPilot run.\n\
         Ticket: {ticket_ref}\n\
         \n\
         Ticket body (from `fetch_issue`):\n\
         ---\n\
         {{{{steps.fetch_issue.data.body}}}}\n\
         ---\n\
         \n\
         Read the project (current cwd is its worktree). Classify every sub-task into clear / decided / mocked / blocked per the schema below. Emit the JSON manifest only.\n\
         \n\
         For EACH item, in addition to its category fields, include:\n\
         - `scope`: array of repo-relative file paths / globs this sub-task touches (drives context + conflict isolation downstream),\n\
         - `complexity`: \"low\" | \"med\" | \"high\" (low = mechanical/boilerplate, high = real logic/architecture),\n\
         - `mechanical`: true if the change is FULLY determined by values you lifted (config/enum/constant/known value) — i.e. could be generated from this manifest without further reasoning — else false,\n\
         - `acceptance`: a deterministic done-check (e.g. \"file X exists AND contains marker KRONN-…(<id>)\"),\n\
         - `depends_on`: array of item ids this sub-task requires done first (empty when independent),\n\
         - `files` (ONLY when `mechanical` is true AND the change CREATES or fully rewrites small files — config/enum/tokens, ≤ ~150 lines): array of {{path, content}} with the COMPLETE final file content, values lifted from evidence. Include the `KRONN-…(<id>)` marker INSIDE the content for decided/mocked items. The engine applies these files directly — ZERO agent run. If the change is an edit to an existing complex file, set `mechanical: false` instead.\n\
         \n\
         After you emit this manifest, a SECOND agent (the reviewer) will challenge it in a debate; address their critique and re-emit the COMPLETE updated manifest each round until you both agree.\n\
         \n\
         GRANULARITY: aim for one item = ONE cohesive concern (the files in its scope belong together). Typically 8-20 items for a big ticket; do not split a single file's edit across items, and do not bundle unrelated files into one item.\n\
         \n\
         Besides the JSON envelope, write ONE file: `.kronn/triage-manifest.md` — the full manifest, human-readable (four categories). The machine-readable work files (tasks.json, decision_ids.txt, files_touched.txt) are DERIVED automatically by the engine from your validated envelope — do NOT write them yourself.",
    );
    let mut s = blank_step("triage", StepType::Agent, agent);
    s.description = Some(
        "[TRIAGE] Feasibility audit — classify every sub-task before any code is written.".into(),
    );
    s.prompt_template = prompt;
    s.output_format = crate::workflows::triage::triage_output_format();
    // « Deux cerveaux » (2026-06-12) — the PLAN must come from the strongest
    // reasoning tier; execution is then routed to cheap tiers per item.
    s.agent_settings = Some(AgentSettings {
        model: None, tier: Some(ModelTier::Reasoning), reasoning_effort: None, max_tokens: None,
    });
    s.stall_timeout_secs = Some(900);
    s.retry = agent_retry();
    // « Deux cerveaux » via DEBATE (2026-06-13) — replaces the old
    // `plan_review → Goto(triage)` file-relay loop. After triage emits the
    // manifest, the reviewer (different model family, reasoning tier) debates
    // it in a shared transcript until `[CONSENSUS: APPROVED]`. The converged
    // manifest is what the human gate sees.
    s.multi_agent_review = Some(crate::models::MultiAgentReviewConfig {
        reviewer_agent: reviewer,
        reviewer_tier: Some(ModelTier::Reasoning),
        debate_prompt: format!(
            "You are reviewing a Feasibility-Gated AutoPilot TRIAGE manifest for ticket {ticket_ref} \
             (the plan, BEFORE any code is written). Read the project (cwd is its worktree) and challenge \
             the manifest adversarially: items whose `scope` misses files they'll need or grabs unrelated \
             ones; granularity (bundled concerns to split, trivial items to merge); mis-classification \
             (clear/decided/mocked/blocked); wrong/missing `depends_on`; and ANY ticket requirement that no \
             item covers. Be concrete and reference real file:line evidence. Do NOT rewrite the manifest \
             yourself — the author revises it. Approve only when it is genuinely sound."
        ),
        max_rounds: Some(2),
    });
    s
}

/// « Deux cerveaux » — `plan_lint` (**Exec, 0 token**). Surfaces the engine-
/// computed lint report (`.kronn/plan_lint.txt`, written alongside the derived
/// machine files) into the step-output system so the plan reviewer and the
/// human gate can reference it via `{{steps.plan_lint.output}}`.
fn build_plan_lint_step() -> WorkflowStep {
    let mut s = blank_step("plan_lint", StepType::Exec, AgentType::ClaudeCode);
    s.description = Some("Deterministic plan-shape report (stats + outlier/overlap/deps warnings). 0 tokens.".into());
    s.exec_command = Some("bash".into());
    s.exec_args = vec!["-c".into(), "cat .kronn/plan_lint.txt 2>/dev/null || echo 'no lint report (manifest derive missing?)'".into()];
    s.exec_timeout_secs = Some(30);
    s
}

/// 3. `review_triage` — **Gate**. Renders the manifest for human review.
fn build_gate_step() -> WorkflowStep {
    let mut s = blank_step("review_triage", StepType::Gate, AgentType::ClaudeCode);
    s.description = Some("Human review of the triage manifest before code is written.".into());
    s.gate_message = Some(
        "## Manifest (debated & converged by the author + reviewer)\n\
         The triage agent and the reviewer have already debated this plan to agreement (see the triage step's transcript). This is the converged result.\n\
         \n\
         ## Plan shape (deterministic lint)\n\
         {{steps.plan_lint.output}}\n\
         \n\
         ## Triage manifest\n\
         {{steps.triage.data}}\n\
         \n\
         - Approve  : continue to implementation as-is.\n\
         - Request changes : send back to triage with your notes (re-debates, loops up to 5 times).\n\
         - Reject   : abandon the run."
            .into(),
    );
    s.gate_request_changes_target = Some("triage".into());
    s
}

/// 4. `implement` — **Agent**.
///    Constrained by the validated manifest; inserts `KRONN-*` markers
///    per entry. Can `[SIGNAL: BLOCKED]` → `Goto(triage)`.
fn build_implement_step(agent: AgentType, ticket_ref: &str) -> WorkflowStep {
    let prompt = format!(
        "You are the IMPLEMENT step of a Feasibility-Gated AutoPilot run.\n\
         Ticket: {ticket_ref}\n\
         \n\
         The validated, human-approved triage manifest is in `.kronn/triage-manifest.md` at the repo root (the triage step wrote it; you share its worktree). READ it first. It classifies every sub-task into:\n\
         - CLEAR — implement directly, no marker needed.\n\
         - DECIDED — implement the `chosen` option, insert `// KRONN-ASSUMED(<id>): <chosen> — <why>` at the primary touch point.\n\
         - MOCKED — implement the `placeholder` strategy, insert `// KRONN-MOCKED(<id>): <strategy>`.\n\
         - BLOCKED — do NOT implement; insert `// KRONN-TODO(<id>): waiting on <needed_from> — <why>` where the feature would have gone.\n\
         \n\
         RULES:\n\
         1. Stay STRICTLY within the files the manifest lists. Do NOT modify anything outside that list — in particular do NOT edit `docs/AGENTS.md`, README, or other docs. If touching an out-of-list file is truly unavoidable, append a line to `.kronn/decisions.md` explaining why BEFORE editing it.\n\
         2. Every entry in decided/mocked/blocked MUST have its corresponding KRONN-* marker in the code, referencing the `id` from the manifest. The next step (`drift_check`) greps for these — anything missing is visible in the PR.\n\
         3. If you discover an entry classified as `clear` or `decided` is actually impossible, do NOT silently mock or fake it: insert a `// KRONN-TODO(<id>): blocked — <why>` marker where it would have gone, append a line to `.kronn/decisions.md` (create it if missing) explaining the blocker, and emit `[SIGNAL: BLOCKED <id>]`. The blocker is then traced in the PR; if it makes tests fail, the run re-triages automatically.\n\
         4. Do not invent values that aren't in the manifest (URLs, secrets, IDs). If the manifest says it's mocked/blocked, respect that.\n\
         5. Do NOT write tests in this step. The next step (`run_tests`) runs the project's existing test suite — keep your scope to implementation.\n\
         6. If a manifest entry's `why` or `strategy` cites `evidence: <linked_repo>/<path>:<line>`, READ that file in the linked repo and lift the concrete value (color hex, URL, constant) — do NOT invent or paraphrase. The `## Linked repositories` block below this prompt lists each linked repo's location. Linked repos are READ-ONLY references — NEVER modify them.\n\
         7. Log any deviation from the manifest (mock you had to add, sub-task deferred, choice not foreseen) to `.kronn/decisions.md` with the reason — the PR-draft step reads it.\n\
         \n\
         Use your tools to edit files. End your response with a brief summary of what you implemented + the [SIGNAL: ...] line.",
    );
    let mut s = blank_step("implement", StepType::Agent, agent);
    s.description = Some("Implementation constrained by the validated triage manifest (read from .kronn/triage-manifest.md).".into());
    s.prompt_template = prompt;
    s.agent_settings = Some(blank_agent_settings());
    s.stall_timeout_secs = Some(1800);
    s.retry = agent_retry();
    s
}

/// Phase 3b — per-TASK implement (decomposed fan-out variant). The child runs
/// once per manifest item; this prompt reads ONLY `.kronn/current_task.json`
/// (its slice) instead of the whole manifest — scoped context means fewer
/// tokens and more determinism. The shared worktree already contains earlier
/// tasks' committed work, so later tasks build on it naturally.
fn build_implement_task_step(agent: AgentType, ticket_ref: &str) -> WorkflowStep {
    let prompt = format!(
        "You are implementing ONE sub-task of a Feasibility-Gated AutoPilot run (ticket {ticket_ref}).\n\
         \n\
         Your task is described in `.kronn/current_task.json` at the repo root — READ IT FIRST. It is ONE item of a human-approved triage manifest, with fields: `id`, `what`, `where`, `scope` (the ONLY files you may touch), `complexity`, `mechanical`, `acceptance` (your done-check), plus `chosen`/`why` (decided) or `placeholder`/`strategy` (mocked) when applicable.\n\
         \n\
         SHARED STATE (read before implementing):\n\
         - `.kronn/decisions.md` — short append-only log of what PREVIOUS tasks deviated on / decided; your task may be affected by it. Always read it (cheap, high-signal). When YOU write to it, keep each entry to 1-3 lines, prefixed by `[<your task id>]` so later tasks can attribute it.\n\
         - `.kronn/triage-manifest.md` — the FULL approved plan (initial intent). Do NOT load it by default (your slice is current_task.json); read it ONLY if your task references something outside its own fields.\n\
         \n\
         RULES:\n\
         1. Implement ONLY this task. Do NOT touch anything outside its `scope` (no docs, no README, no AGENTS.md). If an out-of-scope edit is truly unavoidable, append the reason to `.kronn/decisions.md` BEFORE editing.\n\
         2. If the item is `decided`, implement the `chosen` option and insert `// KRONN-ASSUMED(<id>): <chosen> — <why>` at the primary touch point. If `mocked`, implement the `placeholder` and insert `// KRONN-MOCKED(<id>): <strategy>`. The `<id>` inside the marker MUST be EXACTLY the `id` value of current_task.json — never a variant or a related id (a deterministic check greps for that exact string). Clear items need no marker.\n\
         2bis. If a previous completeness check reported a missing marker (output below — empty on the first pass), your ONLY job is to add exactly that marker:\n\
         {{{{steps.completeness_check.output}}}}\n\
         2ter. RE-RUN AFTER RED TESTS: if `.kronn/item-test-failures.txt` exists, your LAST attempt broke tests. It has TWO sections: fix ONLY the failures under \"YOUR change broke these tests\" (edit the test files too when the behaviour legitimately changed; fix a real code bug otherwise; never weaken a test to force green). The failures under \"pre-existing failures (NOT yours)\" are the repo's own debt — do NOT touch them, do NOT try to fix them (that burns tokens for nothing); ignore them entirely.\n\
         3. If the `why`/`strategy` cites `evidence: <linked_repo>/<path>:<line>`, READ that file and lift the concrete value — do NOT invent. Linked repos are READ-ONLY.\n\
         4. If the task turns out impossible, insert `// KRONN-TODO(<id>): blocked — <why>`, log it in `.kronn/decisions.md`, and emit `[SIGNAL: BLOCKED <id>]`.\n\
         5. Don't author NEW tests for unrelated code, and do NOT commit (a later step commits) — but fixing/updating the tests your change touched is expected (see 2ter). The scoped suite (`item_tests`) runs next and loops back here until green.\n\
         6. Check your `acceptance` criterion before finishing; log any deviation to `.kronn/decisions.md`.\n\
         \n\
         The worktree already contains the previously completed tasks' code — build on it, don't redo it. End with a 2-3 line summary + `[SIGNAL: CONTINUE]`.",
    );
    let mut s = blank_step("implement", StepType::Agent, agent);
    s.description = Some("Implement ONE manifest sub-task (read from .kronn/current_task.json — scoped context).".into());
    s.prompt_template = prompt;
    s.agent_settings = Some(blank_agent_settings());
    // run-14 finding: a hung re-implement sat ~20min before the stall fired.
    // 10min is plenty for one scoped sub-task; recover faster on a hang.
    s.stall_timeout_secs = Some(600);
    s.retry = agent_retry();
    s
}

/// Parent — `test_baseline` (**Exec, 0 token**). Runs the FULL JS + PHP suites
/// ONCE on the human-approved base (after the gate, before any item) and records
/// the set of ALREADY-FAILING test signatures into `.kronn/known-failing.txt`.
/// Per-task `item_tests` subtracts this so it only loops back to implement on
/// NET-NEW failures — never on the repo's pre-existing broken tests (run-14:
/// a timezone bug in `Utils/Time.test.ts`, pulled in transitively, made agents
/// burn ~104k tokens chasing failures they didn't cause). Best-effort: if a
/// suite can't run, its baseline is empty (item_tests then just sees all as new
/// — same as before the fix, never worse).
fn build_test_baseline_step() -> WorkflowStep {
    let script = [
        "set +e",
        "main=\"$(dirname \"$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)\")\"",
        "wt=\"$(git rev-parse --show-toplevel 2>/dev/null)\"",
        "hosttr() { printf '%s' \"${1/#\\/host-home/${KRONN_HOST_HOME:-/host-home}}\"; }",
        "mkdir -p .kronn; : > .kronn/known-failing.txt",
        "[ ! -e node_modules ] && [ -d \"$main/node_modules\" ] && ln -s \"$main/node_modules\" node_modules 2>/dev/null",
        "# JS baseline — full jest; failing tests print as '● Suite › test' lines",
        "if [ -f package.json ] && grep -q '\"test\"' package.json && [ -e node_modules ]; then",
        "  npx --no-install jest --coverage=false >/tmp/base_js.out 2>&1",
        "  grep -oE '●[^\\n]+' /tmp/base_js.out | sed 's/[[:space:]]*$//' | sort -u >> .kronn/known-failing.txt",
        "fi",
        "# PHP baseline — full phpunit via the project docker stack; failing as 'Class::method'",
        "phpdir=''; for d in . application app; do [ -f \"$d/phpunit.xml.dist\" ] && { phpdir=\"$d\"; break; }; done",
        "compose=''; for c in \"$main/docker-compose.yml\" \"$main/docker-compose.yaml\" \"$main/compose.yml\"; do [ -f \"$c\" ] && { compose=\"$c\"; break; }; done",
        "if [ -n \"$phpdir\" ] && [ -n \"$compose\" ] && command -v docker >/dev/null 2>&1; then",
        "  svc=\"$(grep -oE '^  [a-zA-Z0-9_-]+:' \"$compose\" | tr -d ' :' | grep -iE '^php' | head -1)\"; [ -z \"$svc\" ] && svc='php'",
        "  sub=\"${phpdir#./}\"; if [ \"$sub\" = '.' ] || [ -z \"$sub\" ]; then mnt=\"$(hosttr \"$wt\")\"; vend=\"$(hosttr \"$main\")/vendor\"; else mnt=\"$(hosttr \"$wt\")/$sub\"; vend=\"$(hosttr \"$main\")/$sub/vendor\"; fi",
        "  docker compose -f \"$compose\" run --rm --no-deps -T -v \"$mnt:/app\" -v \"$vend:/app/vendor\" -w /app \"$svc\" vendor/bin/phpunit -c phpunit.xml.dist >/tmp/base_php.out 2>&1",
        "  grep -oE '[A-Za-z\\\\]+Test::[a-zA-Z0-9_]+' /tmp/base_php.out | sort -u >> .kronn/known-failing.txt",
        "fi",
        "n=$(wc -l < .kronn/known-failing.txt 2>/dev/null || echo 0)",
        "echo \"baseline: $n pre-existing failing test(s) recorded → item_tests will ignore these\"",
        "echo '[SIGNAL: OK]'; exit 0",
    ]
    .join("\n");
    let mut s = blank_step("test_baseline", StepType::Exec, AgentType::ClaudeCode);
    s.description = Some("Records tests already red on the approved base (.kronn/known-failing.txt) so item_tests only loops on NET-NEW failures. 0 tokens.".into());
    s.exec_command = Some("bash".into());
    s.exec_args = vec!["-c".into(), script];
    s.exec_timeout_secs = Some(900);
    s
}

/// Child — `item_tests` (**Exec, 0 token**). The per-task test→fix loop the
/// user asked for (2026-06-13): the sub-WF re-runs its OWN tests and, if the
/// item broke some, loops back to `implement` until green (bounded). Scoped to
/// what THIS item changed (`git diff`), so it's fast and the failure is the
/// item's own — not an aggregate the parent would have to re-derive:
///   1. `php -l` syntax on changed PHP (instant).
///   2. JS: `jest --findRelatedTests` on changed .ts/.tsx (jest finds exactly
///      the tests covering those files — fast + precise).
///   3. PHP: scoped phpunit (`--filter` on changed class basenames) in the
///      project's dockerized php service (worktree mounted), when available.
///
/// Any failure → writes `.kronn/item-test-failures.txt` + exit 2 →
/// `[SIGNAL: exit_2]` → Goto(implement) (cap 3 = "until green"). After the cap
/// it falls through (committed + documented — a migration item can legitimately
/// need a blocked dep). The FULL cross-item suite is the parent's read-only
/// verdict.
fn build_item_tests_step() -> WorkflowStep {
    let script = [
        "set +e",
        "main=\"$(dirname \"$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)\")\"",
        "wt=\"$(git rev-parse --show-toplevel 2>/dev/null)\"",
        "hosttr() { printf '%s' \"${1/#\\/host-home/${KRONN_HOST_HOME:-/host-home}}\"; }",
        "base=\"$(git merge-base HEAD origin/main 2>/dev/null || git rev-parse HEAD~1 2>/dev/null || git rev-parse HEAD)\"",
        "changed=\"$( { git diff --name-only \"$base\" 2>/dev/null; git diff --name-only 2>/dev/null; git ls-files --others --exclude-standard 2>/dev/null; } | sort -u )\"",
        "[ ! -e node_modules ] && [ -d \"$main/node_modules\" ] && ln -s \"$main/node_modules\" node_modules 2>/dev/null",
        "fail=0; ts_files=''; php_filter=''",
        "while IFS= read -r f; do",
        "  [ -z \"$f\" ] && continue",
        "  case \"$f\" in",
        "    *.php) [ -f \"$f\" ] && command -v php >/dev/null 2>&1 && { php -l \"$f\" >/dev/null 2>/tmp/phpl.err || { echo \"✗ PHP syntax: $f\"; cat /tmp/phpl.err; fail=1; }; }; b=$(basename \"$f\" .php); case \"$f\" in *test*|*Test*) php_filter=\"${php_filter:+$php_filter|}$b\";; esac ;;",
        "    *.ts|*.tsx) [ -f \"$f\" ] && ts_files=\"$ts_files $f\" ;;",
        "    *.json) [ -f \"$f\" ] && { python3 -m json.tool \"$f\" >/dev/null 2>&1 || { echo \"✗ invalid JSON: $f\"; fail=1; }; } ;;",
        "  esac",
        "done <<EOF\n$changed\nEOF",
        "# JS — only the tests covering the changed source files (fast, precise)",
        "if [ -n \"$ts_files\" ] && [ -f package.json ] && [ -e node_modules ]; then",
        "  echo \"→ jest --findRelatedTests$ts_files\"",
        "  npx --no-install jest --findRelatedTests $ts_files --coverage=false --passWithNoTests >/tmp/js.out 2>&1",
        "  [ $? -ne 0 ] && { echo '✗ JS tests red'; tail -25 /tmp/js.out; fail=1; }",
        "fi",
        "# PHP — scoped phpunit (filter on changed test classes) in the project's docker php service",
        "phpdir=''; for d in . application app; do [ -f \"$d/phpunit.xml.dist\" ] && { phpdir=\"$d\"; break; }; done",
        "compose=''; for c in \"$main/docker-compose.yml\" \"$main/docker-compose.yaml\" \"$main/compose.yml\"; do [ -f \"$c\" ] && { compose=\"$c\"; break; }; done",
        "if [ -n \"$php_filter\" ] && [ -n \"$phpdir\" ] && [ -n \"$compose\" ] && command -v docker >/dev/null 2>&1; then",
        "  svc=\"$(grep -oE '^  [a-zA-Z0-9_-]+:' \"$compose\" | tr -d ' :' | grep -iE '^php' | head -1)\"; [ -z \"$svc\" ] && svc='php'",
        "  sub=\"${phpdir#./}\"; if [ \"$sub\" = '.' ] || [ -z \"$sub\" ]; then mnt=\"$(hosttr \"$wt\")\"; vend=\"$(hosttr \"$main\")/vendor\"; else mnt=\"$(hosttr \"$wt\")/$sub\"; vend=\"$(hosttr \"$main\")/$sub/vendor\"; fi",
        "  echo \"→ scoped phpunit --filter '($php_filter)' via docker ($svc)\"",
        "  docker compose -f \"$compose\" run --rm --no-deps -T -v \"$mnt:/app\" -v \"$vend:/app/vendor\" -w /app \"$svc\" vendor/bin/phpunit -c phpunit.xml.dist --filter \"($php_filter)\" >/tmp/php.out 2>&1",
        "  rc=$?; if [ $rc -ne 0 ] && grep -qE 'Tests: [0-9]+' /tmp/php.out; then echo '✗ PHP tests red'; tail -30 /tmp/php.out; fail=1; fi",
        "fi",
        "# Baseline-aware (run-14 fix): the tests ALWAYS run + their full result",
        "# is shown, but we loop back to implement ONLY on NET-NEW failures —",
        "# never on the repo's pre-existing broken tests (.kronn/known-failing.txt",
        "# captured at fan-out start) which made agents burn ~104k chasing a",
        "# timezone bug they didn't cause.",
        "if [ \"$fail\" = 1 ]; then",
        "  { grep -oE '●[^\\n]+' /tmp/js.out 2>/dev/null | sed 's/[[:space:]]*$//'; grep -oE '[A-Za-z\\\\]+Test::[a-zA-Z0-9_]+' /tmp/php.out 2>/dev/null; } | sort -u > /tmp/cur_fail.txt",
        "  if [ -s .kronn/known-failing.txt ]; then netnew=\"$(grep -vxF -f .kronn/known-failing.txt /tmp/cur_fail.txt)\"; else netnew=\"$(cat /tmp/cur_fail.txt)\"; fi",
        "  if [ -n \"$netnew\" ]; then",
        "    { echo '=== YOUR change broke these tests — FIX them (you MAY edit the tests if behaviour legitimately changed) ==='; echo \"$netnew\"; echo; echo '=== pre-existing failures (NOT yours — do NOT chase, the repo was already red here) ==='; grep -xF -f .kronn/known-failing.txt /tmp/cur_fail.txt 2>/dev/null; echo; echo '--- JS tail ---'; tail -25 /tmp/js.out 2>/dev/null; echo '--- PHP tail ---'; tail -30 /tmp/php.out 2>/dev/null; } > .kronn/item-test-failures.txt",
        "    echo \"item_tests FAILED — $(printf '%s\\n' \"$netnew\" | grep -c .) NET-NEW failure(s) caused by this item — looping back to implement\"; exit 2",
        "  fi",
        "  echo 'item_tests: all failures are PRE-EXISTING repo debt (none caused by this item) — not looping'",
        "fi",
        "rm -f .kronn/item-test-failures.txt 2>/dev/null",
        "echo '[SIGNAL: OK]'; exit 0",
    ]
    .join("\n");
    let mut s = blank_step("item_tests", StepType::Exec, AgentType::ClaudeCode);
    s.description = Some("Per-task tests scoped to what THIS item changed (php -l + jest --findRelatedTests + scoped phpunit). Loops to implement ONLY on NET-NEW failures (ignores pre-existing repo debt via .kronn/known-failing.txt) until green (cap 3). 0 tokens.".into());
    s.exec_command = Some("bash".into());
    s.exec_args = vec!["-c".into(), script];
    s.exec_timeout_secs = Some(600);
    s.on_result = vec![StepConditionRule {
        contains: "exit_2".into(),
        action: ConditionAction::Goto { step_name: "implement".into(), max_iterations: Some(3) },
    }];
    s
}

/// 5. `run_tests` — **Exec**, 0 tokens, honest per-suite verdict.
fn build_run_tests_step() -> WorkflowStep {
    // v3 (2026-06-13, run-10 findings) — two corrections over v2:
    //
    // 1. JS VERDICT SEMANTICS. run-10's JS suite was 3396/3396 PASS yet the
    //    step said FAIL — because jest also enforces a COVERAGE threshold and
    //    the new files `implement` created (which, by design, ship without
    //    tests) dipped global coverage 88.17%→88.05%, exiting jest non-zero.
    //    A coverage dip is NOT a test failure. We run `--coverage=false` for
    //    the pass/fail signal and classify a non-zero exit with zero failed
    //    tests as PASS (lint/coverage gate — a CI concern, not the AutoPilot's).
    //
    // 2. PHP RUNS IN THE PROJECT'S DOCKER STACK (user: "tout est dockerisé —
    //    n'installe rien en local, utilise Docker correctement"). The Kronn
    //    container has no php, but it HAS the docker CLI + socket. So we spin
    //    an EPHEMERAL container from the project's own php service image, with
    //    the WORKTREE's app dir bind-mounted over the service's app mount (and
    //    main's vendor borrowed) — testing the AGENT'S branch, not main. Bind
    //    mounts resolve on the docker host, so container paths are translated
    //    back to host paths (KRONN_HOST_HOME). Validated live 2026-06-13.
    //    Falls back to an honest SKIP when no dockerized php stack is found —
    //    never a false FAIL.
    //
    // Exit 0 always — the verdict lives in the output so pr_draft documents it.
    let script = [
        "set +e",
        "main=\"$(dirname \"$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)\")\"",
        "wt=\"$(git rev-parse --show-toplevel 2>/dev/null)\"",
        "# bind mounts resolve on the docker HOST — map container path → host path",
        "hosttr() { printf '%s' \"${1/#\\/host-home/${KRONN_HOST_HOME:-/host-home}}\"; }",
        "js='NOT-RUN'; php_v='NOT-RUN'",
        "",
        "# ---- JS: node is in the Kronn container; run jest against the worktree ----",
        "if [ -f package.json ] && grep -q '\"test\"' package.json; then",
        "  [ ! -e node_modules ] && [ -d \"$main/node_modules\" ] && ln -s \"$main/node_modules\" node_modules && echo '→ node_modules symlinked from main checkout'",
        "  if [ -e node_modules ]; then",
        "    if [ -f yarn.lock ] && command -v yarn >/dev/null 2>&1; then yarn test --coverage=false --silent >/tmp/js.out 2>&1; else npm test -- --coverage=false >/tmp/js.out 2>&1; fi",
        "    rc=$?; tail -15 /tmp/js.out",
        "    if grep -qE 'Tests:[^,]*[1-9][0-9]* failed' /tmp/js.out; then js='FAIL'",
        "    elif [ $rc -eq 0 ]; then js='PASS'",
        "    elif grep -qE 'Tests:[^,]*[0-9]+ passed' /tmp/js.out; then js='PASS(non-test exit — lint/coverage gate, CI-enforced)'",
        "    else js='FAIL'; fi",
        "  else js='SKIP(no node_modules — run yarn install in the main checkout)'; fi",
        "fi",
        "",
        "# ---- PHP: run in the project's dockerized php service, on the worktree ----",
        "phpdir=''; for d in . application app; do [ -f \"$d/phpunit.xml.dist\" ] && { phpdir=\"$d\"; break; }; done",
        "compose=''; for c in \"$main/docker-compose.yml\" \"$main/docker-compose.yaml\" \"$main/compose.yml\"; do [ -f \"$c\" ] && { compose=\"$c\"; break; }; done",
        "if [ -n \"$phpdir\" ] && [ -n \"$compose\" ] && command -v docker >/dev/null 2>&1; then",
        "  svc=\"$(grep -oE '^  [a-zA-Z0-9_-]+:' \"$compose\" | tr -d ' :' | grep -iE '^php' | head -1)\"; [ -z \"$svc\" ] && svc='php'",
        "  sub=\"${phpdir#./}\"; [ \"$sub\" = '.' ] && sub=''",
        "  base=''; [ -n \"$sub\" ] && base=\"/$sub\"",
        "  mnt=\"$(hosttr \"$wt\")$base\"",
        "  # vendor: prefer the worktree's own, else borrow main's. Check the",
        "  # CONTAINER paths directly ($wt/$main come from git inside the Kronn",
        "  # container) — the previous host→container back-substitution was",
        "  # fragile and, when it left vendor unmounted, phpunit failed to boot",
        "  # and got mis-tagged ERROR(harness). A truly absent vendor is now an",
        "  # honest SKIP, not a scary ERROR.",
        "  vend=''",
        "  if [ -d \"$wt$base/vendor\" ]; then vend=\"$(hosttr \"$wt\")$base/vendor\"",
        "  elif [ -d \"$main$base/vendor\" ]; then vend=\"$(hosttr \"$main\")$base/vendor\"; fi",
        "  if [ -z \"$vend\" ]; then php_v='SKIP(no vendor/ — run composer install in the project)'",
        "  else",
        "    echo \"→ PHP via docker compose service '$svc' (worktree mounted, vendor: $vend)\"",
        "    docker compose -f \"$compose\" run --rm --no-deps -T -v \"$mnt:/app\" -v \"$vend:/app/vendor\" -w /app \"$svc\" vendor/bin/phpunit -c phpunit.xml.dist --colors=never >/tmp/php.out 2>&1",
        "    rc=$?; tail -20 /tmp/php.out",
        "    # classify: a phpunit summary line ('Tests: N') means the suite RAN —",
        "    # rc!=0 with a summary = real failures (FAIL); rc!=0 WITHOUT a summary",
        "    # = phpunit couldn't boot (harness/env error). --colors=never keeps",
        "    # the summary parse free of ANSI codes.",
        "    if [ $rc -eq 0 ]; then php_v='PASS'",
        "    elif grep -qE 'Tests: [0-9]+' /tmp/php.out; then fails=\"$(grep -oE '(Failures|Errors): [0-9]+' /tmp/php.out | paste -sd, -)\"; php_v=\"FAIL($fails)\"",
        "    elif grep -qE '(No tests executed|Cannot open|could not open|Fatal error|Class .* not found|bootstrap)' /tmp/php.out; then php_v='ERROR(php harness — not a code failure)'",
        "    else php_v='FAIL'; fi",
        "  fi",
        "else",
        "  php_v='SKIP(no dockerized php stack at repo root — run `make test` in the project)'",
        "fi",
        "",
        "echo \"TEST VERDICT — JS: $js | PHP: $php_v\"",
        "case \"$js$php_v\" in *FAIL*) echo '[SIGNAL: TESTS_FAILED]';; *PASS*) echo '[SIGNAL: OK]';; *) echo '[SIGNAL: SKIPPED]';; esac",
        // Read-only verdict: exit 0 always so the run reaches pr_draft, which
        // documents the result. The test→fix LOOP lives in the CHILD now
        // (per task, where implement has the context) — not here at the parent.
        "exit 0",
    ]
    .join("\n");
    let mut s = blank_step("run_tests", StepType::Exec, AgentType::ClaudeCode);
    s.description = Some(
        "Read-only FULL-suite integration verdict over the merged worktree: JS (jest in-container, coverage≠test-failure) + PHP (project's dockerized php service). Documents the verdict for the PR — the per-task test→fix loop is in the child. 0 tokens.".into(),
    );
    s.exec_command = Some("bash".into());
    s.exec_args = vec!["-c".into(), script];
    s.exec_timeout_secs = Some(1500);
    s
}

fn build_drift_check_step() -> WorkflowStep {
    // Why bash instead of `grep` direct? We need to:
    //   1. Find markers in source code (any extension)
    //   2. Pretty-print as a table the pr_draft can quote
    //   3. Always exit 0 even if no markers (clean implementation
    //      = no markers, not an error)
    // `grep -E ... || true` is the canonical idiom for "ok if empty".
    let script = [
        "set -e",
        "echo '=== KRONN markers in worktree ==='",
        "echo",
        "if grep -rEn 'KRONN-(ASSUMED|MOCKED|TODO)\\([^)]+\\):' \\",
        "  --include='*.php' --include='*.ts' --include='*.tsx' \\",
        "  --include='*.js' --include='*.jsx' --include='*.rs' \\",
        "  --include='*.py' --include='*.go' --include='*.rb' \\",
        "  --include='*.scss' --include='*.css' --include='*.twig' \\",
        "  --include='*.yaml' --include='*.yml' --include='*.json' \\",
        "  --exclude-dir=node_modules --exclude-dir=vendor \\",
        "  --exclude-dir=target --exclude-dir=.git --exclude-dir=dist \\",
        "  . 2>/dev/null; then",
        "  echo",
        "  echo '(markers above — each one should match a decision_id in the triage manifest)'",
        "else",
        "  echo '(no KRONN-* markers found — implementation is fully clear-category)'",
        "fi",
        "echo",
        "exit 0",
    ]
    .join("\n");
    let mut s = blank_step("drift_check", StepType::Exec, AgentType::ClaudeCode);
    s.description = Some(
        "List every KRONN-(ASSUMED|MOCKED|TODO) marker in the worktree so pr_draft can include the audit trail.".into(),
    );
    s.exec_command = Some("bash".into());
    s.exec_args = vec!["-c".into(), script];
    s.exec_timeout_secs = Some(60);
    s
}

/// `scope_check` (child) — **Exec, 0 token, ADVISORY**. Compares the files the
/// agent actually changed against `.kronn/files_touched.txt` (the manifest's
/// declared scope) and logs any out-of-scope file to `.kronn/decisions.md`.
/// The run-2 live audit proved a prompt alone does NOT constrain scope (the
/// agent edited docs/AGENTS.md anyway) — this makes the deviation VISIBLE
/// deterministically. Advisory (exit 0): scope drift isn't always wrong (a
/// referenced architecture doc may be legit), the human reviews the log.
fn build_scope_check_step() -> WorkflowStep {
    let script = [
        "set -e",
        "allow='.kronn/files_touched.txt'",
        "if [ ! -f \"$allow\" ]; then echo 'no files_touched.txt — skip scope check'; echo '[SIGNAL: SKIPPED]'; exit 0; fi",
        "base=\"$(git merge-base HEAD origin/main 2>/dev/null || git rev-parse HEAD 2>/dev/null)\"",
        "changed=\"$( { git diff --name-only \"$base\" 2>/dev/null; git ls-files --others --exclude-standard 2>/dev/null; } | sort -u )\"",
        "extra=''",
        "while IFS= read -r f; do",
        "  [ -z \"$f\" ] && continue",
        // keep f if NO allowlist entry is a prefix of it (handles dir/glob roots)
        "  if ! grep -qF -- \"$f\" \"$allow\" 2>/dev/null && ! awk -v p=\"$f\" 'index(p,$0)==1{found=1} END{exit !found}' \"$allow\" 2>/dev/null; then extra=\"$extra- $f\\n\"; fi",
        "done <<EOF\n$changed\nEOF",
        "if [ -n \"$extra\" ]; then",
        "  { echo ''; echo '## Out-of-scope files (changed but NOT in manifest files_touched — review)'; printf '%b' \"$extra\"; } >> .kronn/decisions.md",
        "  echo 'scope: out-of-scope files flagged in .kronn/decisions.md'",
        "else echo 'scope: all changes within declared files_touched'; fi",
        "echo '[SIGNAL: OK]'",
        "exit 0",
    ]
    .join("\n");
    let mut s = blank_step("scope_check", StepType::Exec, AgentType::ClaudeCode);
    s.description = Some(
        "Flag files changed outside the manifest's declared scope into decisions.md (deterministic, advisory). 0 tokens.".into(),
    );
    s.exec_command = Some("bash".into());
    s.exec_args = vec!["-c".into(), script];
    s.exec_timeout_secs = Some(60);
    s
}

/// `completeness_check` (child) — **Exec, 0 token, ENFORCING**. For every id in
/// `.kronn/decision_ids.txt`, greps the worktree for its `KRONN-*(<id>)` marker.
/// A missing marker = the agent silently skipped that sub-task → emit
/// `[SIGNAL: MISSING]` → `on_result` loops back to `implement` (capped). This
/// replaces an expensive agent "review" with a 0-token deterministic anti-skip.
fn build_completeness_check_step() -> WorkflowStep {
    let script = [
        "set -e",
        "ids='.kronn/decision_ids.txt'",
        "if [ ! -f \"$ids\" ]; then echo 'no decision_ids.txt — skip completeness check'; echo '[SIGNAL: OK]'; exit 0; fi",
        // Phase 3b — per-task mode: when the fan-out exposes the current item
        // via current_task.json, check ONLY that item's id (and only if it's a
        // decided/mocked one — clear items carry no marker by design).
        "if [ -f .kronn/current_task.json ]; then",
        "  tid=\"$(grep -o '\"id\"[[:space:]]*:[[:space:]]*\"[^\"]*\"' .kronn/current_task.json | head -1 | sed 's/.*: *\"//; s/\"$//')\"",
        "  if [ -n \"$tid\" ] && grep -qxF \"$tid\" \"$ids\" 2>/dev/null; then printf '%s\\n' \"$tid\" > /tmp/kronn_cc_ids.txt; ids=/tmp/kronn_cc_ids.txt;",
        "  else echo \"per-task: '$tid' is clear/unlisted — no marker required\"; echo '[SIGNAL: OK]'; exit 0; fi",
        "fi",
        "missing=''",
        "while IFS= read -r id; do",
        "  [ -z \"$id\" ] && continue",
        "  if ! grep -rqE \"KRONN-(ASSUMED|MOCKED|TODO)\\\\($id\\\\)\" --include='*.php' --include='*.ts' --include='*.tsx' --include='*.js' --include='*.scss' --include='*.css' --include='*.twig' --include='*.yaml' --include='*.yml' --exclude-dir=node_modules --exclude-dir=vendor --exclude-dir=.git . 2>/dev/null; then missing=\"$missing $id\"; fi",
        "done < \"$ids\"",
        "if [ -n \"$missing\" ]; then",
        "  { echo ''; echo \"## Missing markers — sub-tasks possibly skipped:$missing\"; } >> .kronn/decisions.md",
        // exit 3 — the Exec executor then emits `[SIGNAL: exit_3]` in the
        // step's LAST lines (where the on_result matcher actually looks);
        // an inline `[SIGNAL: …]` in stdout is wrapped inside the JSON
        // envelope and never matches (run-3 live finding, 2026-06-12).
        "  echo \"completeness: MISSING markers for:$missing\"; exit 3",
        "fi",
        "echo 'completeness: every decision id has its KRONN-* marker'",
        "echo '[SIGNAL: OK]'",
        "exit 0",
    ]
    .join("\n");
    let mut s = blank_step("completeness_check", StepType::Exec, AgentType::ClaudeCode);
    s.description = Some(
        "Verify every decided/mocked/blocked id has its KRONN-* marker; MISSING → loop back to implement. Deterministic anti-skip, 0 tokens.".into(),
    );
    s.exec_command = Some("bash".into());
    s.exec_args = vec!["-c".into(), script];
    s.exec_timeout_secs = Some(120);
    s.on_result = vec![StepConditionRule {
        contains: "exit_3".into(),
        action: ConditionAction::Goto { step_name: "implement".into(), max_iterations: Some(2) },
    }];
    s
}

/// `commit` (child, last step) — **Exec**. Commits the validated Phase-0
/// implementation onto the parent's branch (shared worktree, Phase 2) so the
/// code SURVIVES worktree cleanup. Without it the agent's files stay
/// uncommitted and are deleted when the run ends. Idempotent: nothing staged
/// → soft skip. `git` runs as a bash subprocess (bash allowed) + git is in
/// EXEC_ALLOWLIST.
fn build_commit_step() -> WorkflowStep {
    let script = [
        "set -e",
        "git add -A",
        "if git diff --cached --quiet; then echo 'nothing to commit'; echo '[SIGNAL: SKIPPED]'; exit 0; fi",
        // Per-task subject (`[<id>] …`) + body listing the staged files:
        // the branch history then reads as one reviewable line per sub-task
        // (and the [id] is what the foreach resume-skip greps for).
        "tid=''",
        "if [ -f .kronn/current_task.json ]; then tid=\"$(grep -o '\"id\"[[:space:]]*:[[:space:]]*\"[^\"]*\"' .kronn/current_task.json | head -1 | sed 's/.*: *\"//; s/\"$//')\"; fi",
        "subject=\"Kronn AutoPilot${tid:+ [$tid]} — implementation (KRONN-traced)\"",
        "body=\"$(git diff --cached --name-only)\"",
        "git -c user.email='autopilot@kronn.local' -c user.name='Kronn AutoPilot' commit --no-verify -m \"$subject\" -m \"$body\"",
        "echo \"→ committed $(git rev-parse --short HEAD) ${tid:+[$tid]}\"",
        "echo '[SIGNAL: OK]'",
    ]
    .join("\n");
    let mut s = blank_step("commit", StepType::Exec, AgentType::ClaudeCode);
    s.description = Some(
        "Commit the validated implementation onto the parent branch so it survives worktree cleanup. 0 tokens.".into(),
    );
    s.exec_command = Some("bash".into());
    s.exec_args = vec!["-c".into(), script];
    s.exec_timeout_secs = Some(120);
    s
}

/// 7. `pr_draft` — **Agent**.
///    PR body = manifest checklist + open items + drift_check output verbatim.
fn build_pr_draft_step(agent: AgentType, ticket_ref: &str) -> WorkflowStep {
    let prompt = format!(
        "You are the PR-DRAFT step of a Feasibility-Gated AutoPilot run.\n\
         Ticket: {ticket_ref}\n\
         \n\
         Output ONLY the Markdown PR body below — no preamble, no \"Reading files…\" / \"Let me check…\" commentary, no closing remarks. Your FIRST character must be the `## Ticket` heading.\n\
         \n\
         Produce a PR description in Markdown with this structure:\n\
         \n\
         ## Ticket\n\
         {ticket_ref}\n\
         \n\
         ## Summary\n\
         <1-3 sentences>\n\
         \n\
         ## Implemented (from triage)\n\
         <list of CLEAR + DECIDED + MOCKED entries with their KRONN-* markers>\n\
         \n\
         ## Still open (blocked)\n\
         <list of BLOCKED entries with `needed_from` — these are explicit asks to other teams>\n\
         \n\
         ## Decisions taken (review carefully)\n\
         <bullets of DECIDED entries: what was chosen vs alternatives, why>\n\
         \n\
         ## Mocks to replace later\n\
         <bullets of MOCKED entries with `revisit_when`>\n\
         \n\
         ## Test verdict\n\
         <quote the `TEST VERDICT — JS: … | PHP: …` line from {{{{steps.run_tests.output}}}} verbatim; if a suite FAILED or was SKIPPED, say it plainly — do NOT soften it. Also give the sub-workflow outcome: {{{{steps.feasibility_impl.summary}}}}>\n\
         \n\
         ## Failed sub-tasks (ONLY when some failed)\n\
         <if `{{{{steps.feasibility_impl.data.failed}}}}` is greater than 0, list the failed entries from `{{{{steps.feasibility_impl.data.items}}}}` with their ids — these need a human follow-up. Omit the whole section when zero.>\n\
         \n\
         ## Markers audit (drift_check output)\n\
         <embed `{{{{steps.drift_check.output}}}}` verbatim (regenerated over the FINAL worktree) inside a ``` code fence so reviewers see every traced freedom>\n\
         \n\
         ## Test plan\n\
         <bullet list of how to verify each implemented entry manually, beyond what the test suite covers>\n\
         \n\
         Pull values from `{{{{steps.triage.data}}}}` and read the deviations log `.kronn/decisions.md`. End with `[SIGNAL: CONTINUE]`.",
    );
    let mut s = blank_step("pr_draft", StepType::Agent, agent);
    s.description = Some(
        "PR description listing implemented + open + decided + mocked + test verdict + drift audit."
            .into(),
    );
    s.prompt_template = prompt;
    s.agent_settings = Some(blank_agent_settings());
    s.stall_timeout_secs = Some(600);
    s.retry = agent_retry();
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::OnInvalid;

    fn default_params() -> FeasibilityWorkflowParams {
        FeasibilityWorkflowParams {
            project_id: Some("proj".into()),
            ticket_ref: Some("TEST-1".into()),
            ticket_body: Some("Body of the ticket".into()),
            agent: None,
            name: None,
            decomposed: false,
            reviewer_agent: None,
        }
    }

    fn default_child() -> CreateWorkflowRequest {
        build_feasibility_child(
            AgentType::ClaudeCode,
            "TEST-1",
            Some("proj".into()),
            "Big-ticket AutoPilot (TEST-1)",
            false,
        )
    }

    fn decomposed_params() -> FeasibilityWorkflowParams {
        FeasibilityWorkflowParams { decomposed: true, ..default_params() }
    }

    #[test]
    fn decomposed_variant_fans_out_per_task() {
        // Phase 3b — parent's SubWorkflow step iterates .kronn/tasks.json;
        // the child's implement reads ONLY its current_task slice.
        let parent = build_feasibility_workflow(decomposed_params());
        let sw = parent.steps.iter().find(|s| s.name == "feasibility_impl").unwrap();
        assert_eq!(sw.sub_workflow_foreach_file.as_deref(), Some(".kronn/tasks.json"));
        // Baseline (monolith) has NO foreach — the A/B comparison hinge.
        let mono = build_feasibility_workflow(default_params());
        let sw2 = mono.steps.iter().find(|s| s.name == "feasibility_impl").unwrap();
        assert!(sw2.sub_workflow_foreach_file.is_none());

        let child = build_feasibility_child(
            AgentType::ClaudeCode, "TEST-1", Some("proj".into()), "X", true,
        );
        let imp = &child.steps[0];
        assert!(imp.prompt_template.contains(".kronn/current_task.json"),
            "per-task implement must read its slice");
        // Shared-state contract (user decision 2026-06-12): always READ the
        // running deviations log; the full plan stays opt-in (scoped context).
        assert!(imp.prompt_template.contains("decisions.md"),
            "per-task implement must read the shared decisions log (current state)");
        assert!(imp.prompt_template.contains("ONLY if your task references"),
            "the full manifest must be referenced as OPT-IN, not loaded by default");
        // Triage (both variants) writes ONLY the human manifest; the machine
        // files (tasks.json…) are engine-DERIVED from the validated envelope
        // (critical fix #1 — trust boundary).
        assert!(parent.steps[1].prompt_template.contains("DERIVED automatically"));
        assert!(parent.steps[1].prompt_template.contains("do NOT write them yourself"));
        assert!(parent.steps[1].prompt_template.contains("depends_on"));
        // completeness_check is per-task aware (current_task.json branch).
        let cc = child.steps.iter().find(|s| s.name == "completeness_check").unwrap();
        assert!(cc.exec_args.last().unwrap().contains("current_task.json"));
    }

    #[test]
    fn parent_has_decomposed_five_steps_in_order() {
        // 2026-06-11 (PR-C) — the implement/test/drift loop moved to a child.
        let wf = build_feasibility_workflow(default_params());
        let names: Vec<&str> = wf.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["fetch_issue", "triage", "plan_lint", "review_triage", "test_baseline", "feasibility_impl", "run_tests", "drift_check", "pr_draft"]
        );
        // run_tests is a READ-ONLY integration verdict (no fix loop at the
        // parent — the per-task test→fix loop belongs in the child). It exits
        // 0 always and flows linearly to drift_check.
        let rt = wf.steps.iter().find(|s| s.name == "run_tests").unwrap();
        assert!(rt.on_result.is_empty(), "run_tests must not branch — read-only verdict");
        assert!(rt.exec_args.last().unwrap().trim_end().ends_with("exit 0"));
    }

    #[test]
    fn child_has_implement_test_drift_loop_no_gate() {
        let child = default_child();
        let names: Vec<&str> = child.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["implement", "item_tests", "scope_check", "completeness_check", "commit"]);
        // `commit` is a deterministic Exec (git via bash subprocess) — last step.
        assert_eq!(child.steps.last().unwrap().step_type, StepType::Exec);
        // completeness_check loops back to implement on a missing marker (anti-skip).
        let cc = child.steps.iter().find(|s| s.name == "completeness_check").unwrap();
        let rule = cc.on_result.first().expect("completeness_check needs an exit_3 rule");
        assert_eq!(rule.contains, "exit_3");
        match &rule.action {
            ConditionAction::Goto { step_name, max_iterations } => {
                assert_eq!(step_name, "implement");
                assert_eq!(*max_iterations, Some(2));
            }
            other => panic!("expected Goto(implement), got {other:?}"),
        }
        // No Gate inside a child (forbidden in a sub-workflow, validated server-side).
        assert!(!child.steps.iter().any(|s| s.step_type == StepType::Gate));
        assert!(child.project_id.is_some(), "child inherits the parent project_id");
    }

    #[test]
    fn fetch_issue_is_json_data_with_ticket_body() {
        // The wizard upgrades JsonData → ApiCall on save when a
        // tracker plugin is available. The Rust template stays
        // JsonData so it works regardless.
        let wf = build_feasibility_workflow(default_params());
        let s = &wf.steps[0];
        assert_eq!(s.step_type, StepType::JsonData);
        let payload = s.json_data_payload.as_ref().expect("JsonData step needs payload");
        assert_eq!(payload["key"].as_str(), Some("TEST-1"));
        assert_eq!(payload["body"].as_str(), Some("Body of the ticket"));
    }

    #[test]
    fn triage_step_uses_typed_schema_fail_and_has_marker() {
        let wf = build_feasibility_workflow(default_params());
        let s = &wf.steps[1];
        assert_eq!(s.step_type, StepType::Agent);
        let desc = s.description.as_deref().unwrap();
        assert!(desc.starts_with("[TRIAGE]"), "got: {desc}");
        match &s.output_format {
            StepOutputFormat::TypedSchema { on_invalid, .. } => {
                assert_eq!(*on_invalid, OnInvalid::Fail);
            }
            other => panic!("expected TypedSchema, got {other:?}"),
        }
    }

    #[test]
    fn triage_reads_from_fetch_issue_data() {
        // The prompt template must reference fetch_issue's output so
        // the wizard's JsonData → ApiCall transform stays transparent
        // to the agent.
        let wf = build_feasibility_workflow(default_params());
        assert!(
            wf.steps[1].prompt_template.contains("{{steps.fetch_issue.data"),
            "triage prompt must reference fetch_issue data"
        );
    }

    #[test]
    fn gate_step_targets_triage_for_request_changes() {
        let wf = build_feasibility_workflow(default_params());
        let s = wf.steps.iter().find(|s| s.name == "review_triage").unwrap();
        assert_eq!(s.step_type, StepType::Gate);
        assert_eq!(s.gate_request_changes_target.as_deref(), Some("triage"));
    }

    #[test]
    fn triage_writes_manifest_file_for_the_child() {
        // The child reads the manifest from .kronn/triage-manifest.md — it
        // can't see the parent's {{steps.triage.data}} across the boundary.
        let wf = build_feasibility_workflow(default_params());
        assert!(
            wf.steps[1].prompt_template.contains(".kronn/triage-manifest.md"),
            "triage must write the manifest to the shared file the child reads"
        );
    }

    #[test]
    fn feasibility_impl_is_subworkflow_retriaging_on_failure() {
        // PR-C — the old `implement BLOCKED → Goto(triage)` is reconstructed
        // at the parent: a failed child run (SUBWF_FAILED) re-triages.
        let wf = build_feasibility_workflow(default_params());
        let s = wf.steps.iter().find(|s| s.name == "feasibility_impl").unwrap();
        assert_eq!(s.step_type, StepType::SubWorkflow);
        let rule = s.on_result.first().expect("feasibility_impl needs a SUBWF_FAILED rule");
        assert_eq!(rule.contains, "SUBWF_FAILED");
        match &rule.action {
            ConditionAction::Goto { step_name, max_iterations } => {
                assert_eq!(step_name, "triage");
                assert_eq!(*max_iterations, Some(3));
            }
            other => panic!("expected Goto(triage), got {other:?}"),
        }
    }

    #[test]
    fn child_implement_reads_manifest_file_and_keeps_evidence_lift() {
        let child = default_child();
        let prompt = &child.steps[0].prompt_template;
        assert_eq!(child.steps[0].step_type, StepType::Agent);
        assert!(prompt.contains(".kronn/triage-manifest.md"), "implement must read the manifest file");
        // Must NOT reach across the parent boundary (unresolvable in the child).
        assert!(!prompt.contains("{{steps.triage.data"), "implement must not reference parent step data");
        // The linked_repos evidence-lift rule is preserved (anti-MOCKED regression).
        assert!(prompt.contains("evidence:"));
        assert!(prompt.contains("Linked repositories"));
        assert!(prompt.contains("READ-ONLY"));
        assert!(prompt.contains("lift"));
    }

    #[test]
    fn child_item_tests_loops_to_implement_until_green() {
        // 2026-06-13 (user): the sub-WF re-runs its OWN tests scoped to what
        // the item changed, looping back to implement until green (bounded).
        let child = default_child();
        let s = &child.steps[1];
        assert_eq!(s.name, "item_tests");
        assert_eq!(s.step_type, StepType::Exec);
        let script = s.exec_args.last().expect("bash needs a -c script");
        assert!(script.contains("command -v php"), "php -l guarded by runtime presence");
        assert!(script.contains("jest --findRelatedTests"), "JS tests scoped to the item's changed files");
        assert!(script.contains("--filter"), "PHP scoped to the item's changed test classes");
        assert!(script.contains("item-test-failures.txt"), "failures fed back to implement");
        let rule = s.on_result.first().expect("item_tests needs exit_2 rule");
        assert_eq!(rule.contains, "exit_2");
        match &rule.action {
            ConditionAction::Goto { step_name, max_iterations } => {
                assert_eq!(step_name, "implement");
                assert_eq!(*max_iterations, Some(3)); // "until green", bounded
            }
            other => panic!("expected Goto(implement), got {other:?}"),
        }
    }

    #[test]
    fn test_baseline_and_item_tests_are_baseline_aware() {
        // run-14 fix: a parent test_baseline step records pre-existing failures;
        // item_tests only loops on NET-NEW ones (ignores the repo's own debt).
        let wf = build_feasibility_workflow(default_params());
        let base = wf.steps.iter().find(|s| s.name == "test_baseline").expect("parent has a test_baseline step");
        assert_eq!(base.step_type, StepType::Exec);
        assert!(base.exec_args.last().unwrap().contains("known-failing.txt"));
        // test_baseline runs BEFORE the fan-out.
        let pos = |n: &str| wf.steps.iter().position(|s| s.name == n).unwrap();
        assert!(pos("test_baseline") < pos("feasibility_impl"));
        // item_tests gates the loop on net-new failures vs the baseline.
        let child = default_child();
        let it = child.steps.iter().find(|s| s.name == "item_tests").unwrap();
        let script = it.exec_args.last().unwrap();
        assert!(script.contains("known-failing.txt"), "subtracts the pre-existing baseline");
        assert!(script.contains("NET-NEW"), "loops only on net-new failures");
    }

    #[test]
    fn parent_run_tests_v3_js_in_container_php_via_docker() {
        // run-8: exec'd first match (JS shadowed PHP). run-10: coverage dip
        // read as a false FAIL; php skipped for lack of a local runtime.
        let wf = build_feasibility_workflow(default_params());
        let s = wf.steps.iter().find(|s| s.name == "run_tests").unwrap();
        let script = s.exec_args.last().unwrap();
        // JS: jest in-container, coverage gate ≠ test failure (run-10 fix)
        assert!(script.contains("--coverage=false"), "coverage gate must not mask a real test pass/fail");
        assert!(script.contains("lint/coverage gate"), "non-test exit with 0 failures classified as PASS");
        assert!(script.contains("command -v yarn"), "yarn falls back to npm when absent");
        // PHP: project's dockerized php service, worktree-mounted (no local install)
        assert!(script.contains("docker compose -f"), "PHP runs in the project's docker stack");
        assert!(script.contains("vendor/bin/phpunit -c phpunit.xml.dist"), "PHP suite actually runs");
        assert!(script.contains("hosttr"), "container→host path translation for bind mounts");
        assert!(script.contains("no dockerized php stack"), "honest SKIP when no stack — never a false FAIL");
        // 0.8.8 fignolage — robust PHP verdict:
        assert!(script.contains("--colors=never"), "ANSI-free phpunit output → reliable summary parse");
        assert!(script.contains("composer install"), "absent vendor → honest SKIP, not a scary ERROR(harness)");
        assert!(script.contains("$wt$base/vendor"), "vendor resolved via CONTAINER path (worktree first) — no fragile host→container back-substitution");
        assert!(!script.contains("${vend/#"), "the fragile parameter back-substitution is gone");
        assert!(!script.contains("no php runtime in the Kronn container"), "no longer installs/needs local php");
        assert!(script.contains("TEST VERDICT"), "per-suite verdict for the PR");
    }

    #[test]
    fn triage_debate_reviewer_defaults_to_codex_cross_model() {
        // User decision: plan author = Claude Reasoning, debate reviewer =
        // Codex (different model family → no same-model blind spots).
        let wf = build_feasibility_workflow(default_params());
        let triage = wf.steps.iter().find(|s| s.name == "triage").unwrap();
        assert_eq!(triage.multi_agent_review.as_ref().unwrap().reviewer_agent, AgentType::Codex);
    }

    #[test]
    fn agent_steps_carry_retry_for_rate_limit_resilience() {
        // run-10 (2026-06-13): a transient rate-limit exited the agent 1 with
        // no output; with no retry, 4 fan-out items AND the final pr_draft
        // died permanently. Every Agent step (parent + child) must retry.
        let wf = build_feasibility_workflow(default_params());
        for name in ["triage", "pr_draft"] {
            let s = wf.steps.iter().find(|s| s.name == name).unwrap();
            assert!(s.retry.as_ref().map(|r| r.max_retries >= 2).unwrap_or(false),
                "parent agent step `{name}` must retry ≥2× on transient failure");
        }
        let child = default_child();
        let imp = child.steps.iter().find(|s| s.name == "implement").unwrap();
        assert!(imp.retry.as_ref().map(|r| r.max_retries >= 2).unwrap_or(false),
            "child `implement` must retry — a rate-limit kill loses one fan-out item otherwise");
    }

    // (child drift_check removed — the parent-level drift_check covers the
    // final worktree once; per-task markers are enforced by completeness_check)

    #[test]
    fn pr_draft_embeds_subworkflow_outputs() {
        // Parent's pr_draft surfaces the child's verdict via the SubWorkflow
        // envelope (status + last_output = drift_check), not child-internal
        // step names (which don't resolve across the boundary).
        let wf = build_feasibility_workflow(default_params());
        let s = wf.steps.iter().find(|s| s.name == "pr_draft").unwrap();
        assert_eq!(s.step_type, StepType::Agent);
        assert!(
            s.prompt_template.contains("{{steps.run_tests.output}}"),
            "pr_draft must quote the dual-suite TEST VERDICT verbatim"
        );
        assert!(
            s.prompt_template.contains("{{steps.feasibility_impl.summary}}"),
            "pr_draft must surface the sub-workflow outcome"
        );
        assert!(
            s.prompt_template.contains("{{steps.drift_check.output}}"),
            "pr_draft must surface the PARENT-level drift_check (regenerated over the final worktree)"
        );
        assert!(
            s.prompt_template.contains("{{steps.feasibility_impl.data.failed}}"),
            "pr_draft must surface failed sub-tasks when the fan-out is PARTIAL"
        );
    }

    #[test]
    fn child_exec_allowlist_covers_runners_parent_has_none() {
        let child = default_child();
        for needed in ["bash", "grep", "make", "cargo", "pnpm", "composer", "pytest"] {
            assert!(
                child.exec_allowlist.iter().any(|s| s == needed),
                "child exec_allowlist must include '{needed}'"
            );
        }
        // Parent no longer runs Exec — the loop is in the child.
        let parent = build_feasibility_workflow(default_params());
        // Parent now runs ONE Exec: the final drift_check (bash+grep only).
        assert_eq!(parent.exec_allowlist, vec!["bash".to_string(), "grep".to_string()]);
        let parent_execs: Vec<&str> = parent.steps.iter()
            .filter(|s| s.step_type == StepType::Exec).map(|s| s.name.as_str()).collect();
        assert_eq!(parent_execs, vec!["plan_lint", "test_baseline", "run_tests", "drift_check"]);
    }

    #[test]
    fn guards_cap_loop_revisits_at_five() {
        let wf = build_feasibility_workflow(default_params());
        let guards = wf.guards.expect("guards must be set");
        assert_eq!(guards.loop_detection_max_revisits, Some(5));
    }

    #[test]
    fn name_includes_ticket_ref_when_provided() {
        let wf = build_feasibility_workflow(default_params());
        assert!(wf.name.contains("TEST-1"), "got name: {}", wf.name);
    }

    #[test]
    fn token_cost_audit_parent_and_child_agents() {
        // Désagentification rule preserved across the split: parent pays for
        // triage + pr_draft, child pays for implement. Everything else is
        // deterministic (Gate / SubWorkflow / Exec / JsonData).
        let parent = build_feasibility_workflow(default_params());
        let parent_agents: Vec<&str> = parent.steps.iter()
            .filter(|s| s.step_type == StepType::Agent).map(|s| s.name.as_str()).collect();
        assert_eq!(parent_agents, vec!["triage", "pr_draft"]);
        let child = default_child();
        let child_agents: Vec<&str> = child.steps.iter()
            .filter(|s| s.step_type == StepType::Agent).map(|s| s.name.as_str()).collect();
        assert_eq!(child_agents, vec!["implement"]);
    }

    #[test]
    fn two_brains_triage_debates_with_reviewer_then_gate() {
        // 2026-06-13 — the plan_review→Goto loop is replaced by a DEBATE inside
        // the triage step (multi_agent_review): the reviewer challenges the
        // converged manifest in a shared transcript until consensus.
        let wf = build_feasibility_workflow(default_params());
        let triage = wf.steps.iter().find(|s| s.name == "triage").unwrap();
        // PLAN comes from the strongest tier — guaranteed.
        assert!(matches!(triage.agent_settings.as_ref().unwrap().tier, Some(ModelTier::Reasoning)));
        // Debate config: reviewer = Codex (cross-model) on the reasoning tier.
        let mar = triage.multi_agent_review.as_ref().expect("triage debates with a reviewer");
        assert_eq!(mar.reviewer_agent, AgentType::Codex);
        assert!(matches!(mar.reviewer_tier, Some(ModelTier::Reasoning)));
        assert!(mar.debate_prompt.contains("manifest"));
        // reviewer override respected
        let custom = build_feasibility_workflow(FeasibilityWorkflowParams {
            reviewer_agent: Some(AgentType::ClaudeCode), ..default_params()
        });
        let t2 = custom.steps.iter().find(|s| s.name == "triage").unwrap();
        assert_eq!(t2.multi_agent_review.as_ref().unwrap().reviewer_agent, AgentType::ClaudeCode);
        // No separate plan_review step anymore.
        assert!(wf.steps.iter().all(|s| s.name != "plan_review"));
        // plan_lint stays as the deterministic 0-token guard.
        let lint = wf.steps.iter().find(|s| s.name == "plan_lint").unwrap();
        assert!(lint.exec_args.last().unwrap().contains("plan_lint.txt"));
        // The human gate sees the converged manifest + lint.
        let gate = wf.steps.iter().find(|s| s.name == "review_triage").unwrap();
        let msg = gate.gate_message.as_deref().unwrap();
        assert!(msg.contains("{{steps.plan_lint.output}}") && msg.contains("{{steps.triage.data}}"));
    }

    #[test]
    fn triage_prompt_frames_the_debate() {
        // The triage author is told a reviewer will debate the manifest.
        let wf = build_feasibility_workflow(default_params());
        let triage = wf.steps.iter().find(|s| s.name == "triage").unwrap();
        assert!(triage.prompt_template.contains("reviewer) will challenge"));
        assert!(triage.prompt_template.contains("re-emit the COMPLETE updated manifest"));
    }
}
