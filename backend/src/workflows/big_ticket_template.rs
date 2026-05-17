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
    AgentSettings, AgentType, ConditionAction, CreateWorkflowRequest, StepConditionRule, StepMode,
    StepOutputFormat, StepType, WorkflowGuards, WorkflowStep, WorkflowTrigger,
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
}

/// Common shell allowlist for the Exec steps (`run_tests` +
/// `drift_check`). Mirror of the `ticket-to-pr` preset list, plus the
/// `grep` we need for `drift_check`. `bash` is the entry point for the
/// auto-detect script.
const EXEC_ALLOWLIST: &[&str] = &[
    "bash", "cargo", "pnpm", "npm", "yarn", "pytest", "make", "composer", "grep",
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

    let steps = vec![
        build_fetch_issue_step(&ticket_ref, &ticket_body),
        build_triage_step(agent.clone(), &ticket_ref),
        build_gate_step(),
        build_implement_step(agent.clone(), &ticket_ref),
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
            timeout_seconds: None,
            max_llm_calls: None,
            loop_detection_max_revisits: Some(5),
        }),
        artifacts: HashMap::new(),
        on_failure: vec![],
        // Both Exec steps (run_tests + drift_check) need the allowlist
        // wired in here, otherwise the validator rejects the workflow.
        exec_allowlist: EXEC_ALLOWLIST.iter().map(|s| s.to_string()).collect(),
        variables: vec![],
        // 0.8.5 — `enabled: None` lets the handler default to true (the
        // big-ticket template path is "user clicked the button to create
        // this", not "agent autonomously drafted it" — same semantics as
        // every UI-driven create).
        enabled: None,
    }
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
        exec_command: None,
        exec_args: vec![],
        exec_timeout_secs: None,
        exec_setup_command: None,
        exec_setup_args: vec![],
        quick_prompt_id: None,
        json_data_payload: None,
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
fn build_triage_step(agent: AgentType, ticket_ref: &str) -> WorkflowStep {
    let prompt = format!(
        "You are the TRIAGE step of a Feasibility-Gated AutoPilot run.\n\
         Ticket: {ticket_ref}\n\
         \n\
         Ticket body (from `fetch_issue`):\n\
         ---\n\
         {{{{steps.fetch_issue.data.body}}}}\n\
         ---\n\
         \n\
         Read the project (current cwd is its worktree). Classify every sub-task into clear / decided / mocked / blocked per the schema below. Emit the JSON manifest only.",
    );
    let mut s = blank_step("triage", StepType::Agent, agent);
    s.description = Some(
        "[TRIAGE] Feasibility audit — classify every sub-task before any code is written.".into(),
    );
    s.prompt_template = prompt;
    s.output_format = crate::workflows::triage::triage_output_format();
    s.agent_settings = Some(blank_agent_settings());
    s.stall_timeout_secs = Some(900);
    s
}

/// 3. `review_triage` — **Gate**. Renders the manifest for human review.
fn build_gate_step() -> WorkflowStep {
    let mut s = blank_step("review_triage", StepType::Gate, AgentType::ClaudeCode);
    s.description = Some("Human review of the triage manifest before code is written.".into());
    s.gate_message = Some(
        "Triage manifest produced. Review the four categories below before approving:\n\
         \n\
         {{steps.triage.data}}\n\
         \n\
         - Approve  : continue to implementation as-is.\n\
         - Request changes : send back to triage with your notes (loops up to 5 times).\n\
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
         The triage step produced and a human (or auto-budget) approved this manifest:\n\
         \n\
         CLEAR entries (implement directly, no marker needed):\n\
         {{{{steps.triage.data.clear}}}}\n\
         \n\
         DECIDED entries (implement the `chosen` option, insert `// KRONN-ASSUMED(<id>): <chosen> — <why>` at the primary touch point):\n\
         {{{{steps.triage.data.decided}}}}\n\
         \n\
         MOCKED entries (implement the `placeholder` strategy, insert `// KRONN-MOCKED(<id>): <strategy>`):\n\
         {{{{steps.triage.data.mocked}}}}\n\
         \n\
         BLOCKED entries (do NOT implement the feature, only insert `// KRONN-TODO(<id>): waiting on <needed_from> — <why>` where the feature would have been):\n\
         {{{{steps.triage.data.blocked}}}}\n\
         \n\
         Files the manifest claims you'll touch:\n\
         {{{{steps.triage.data.files_touched}}}}\n\
         \n\
         RULES:\n\
         1. Stay within the files_touched list unless absolutely necessary.\n\
         2. Every entry in decided/mocked/blocked MUST have its corresponding KRONN-* marker in the code, referencing the `id` from the manifest. The next step (`drift_check`) will grep for these — anything missing will be visible in the PR.\n\
         3. If you discover an entry classified as `clear` or `decided` is actually impossible to implement, STOP immediately and emit `[SIGNAL: BLOCKED <id>]` as the last line of your response. The workflow will jump back to triage. Do NOT silently mock or fake it.\n\
         4. Do not invent values that aren't in the manifest (URLs, secrets, IDs). If the manifest says it's mocked/blocked, respect that.\n\
         5. Do NOT write tests in this step. The next step (`run_tests`) is an Exec that runs the project's existing test suite. The agent that writes tests will run separately if needed — keep your scope to implementation.\n\
         6. If a manifest entry's `why` or `strategy` cites `evidence: <linked_repo>/<path>:<line>`, READ that file in the linked repo and lift the concrete value (color hex, URL, constant) — do NOT invent or paraphrase. The `## Linked repositories` block below this prompt lists each linked repo's location. Linked repos are READ-ONLY references — NEVER modify them.\n\
         \n\
         Use your tools to edit files. End your response with a brief summary of what you implemented + the [SIGNAL: ...] line.",
    );
    let mut s = blank_step("implement", StepType::Agent, agent);
    s.description = Some("Implementation constrained by the validated triage manifest.".into());
    s.prompt_template = prompt;
    s.agent_settings = Some(blank_agent_settings());
    s.stall_timeout_secs = Some(1800);
    s.on_result = vec![StepConditionRule {
        contains: "BLOCKED".into(),
        action: ConditionAction::Goto {
            step_name: "triage".into(),
            max_iterations: Some(3),
        },
    }];
    s
}

/// 5. `run_tests` — **Exec**.
///    Generic auto-detect from `ticket-to-pr`. 0 tokens, real verdict.
///    `ERROR` → `Goto(implement)`, max 2 iters.
fn build_run_tests_step() -> WorkflowStep {
    let script = [
        "set -e",
        // Make has highest priority — opinionated `test` target wires up linters, type-checks, the right runner.
        "if [ -f Makefile ] && grep -qE '^test:' Makefile; then echo '→ make test'; exec make test; fi",
        // Rust
        "if [ -f Cargo.toml ]; then echo '→ cargo test --lib'; exec cargo test --lib; fi",
        // JS/TS
        "if [ -f package.json ] && grep -q '\"test\"' package.json; then",
        "  if [ ! -d node_modules ]; then echo '⚠ node_modules absent — skip'; echo '[SIGNAL: SKIPPED]'; exit 0; fi",
        "  if [ -f pnpm-lock.yaml ]; then echo '→ pnpm test'; exec pnpm test",
        "  elif [ -f yarn.lock ]; then echo '→ yarn test'; exec yarn test",
        "  else echo '→ npm test'; exec npm test; fi",
        "fi",
        // PHP
        "if [ -f composer.json ]; then",
        "  if [ ! -d vendor ]; then echo '⚠ vendor/ absent — skip'; echo '[SIGNAL: SKIPPED]'; exit 0; fi",
        "  if grep -q '\"test\"' composer.json; then echo '→ composer test'; exec composer test",
        "  elif [ -x vendor/bin/phpunit ]; then echo '→ vendor/bin/phpunit'; exec vendor/bin/phpunit",
        "  else echo 'no PHP test runner'; echo '[SIGNAL: SKIPPED]'; exit 0; fi",
        "fi",
        // Python
        "if [ -f pyproject.toml ] || [ -f setup.py ]; then echo '→ pytest'; exec pytest; fi",
        // Fallback
        "echo '→ aucun framework de tests détecté — skip'",
        "echo '[SIGNAL: SKIPPED]'",
        "exit 0",
    ]
    .join("\n");
    let mut s = blank_step("run_tests", StepType::Exec, AgentType::ClaudeCode);
    s.description = Some(
        "Run the project's test suite. 0 tokens — deterministic real verdict, not an agent's claim."
            .into(),
    );
    s.exec_command = Some("bash".into());
    s.exec_args = vec!["-c".into(), script];
    s.exec_timeout_secs = Some(900);
    s.on_result = vec![StepConditionRule {
        contains: "ERROR".into(),
        action: ConditionAction::Goto {
            step_name: "implement".into(),
            max_iterations: Some(2),
        },
    }];
    s
}

/// 6. `drift_check` — **Exec**.
///    Greps `KRONN-(ASSUMED|MOCKED|TODO)` in the worktree. 0 tokens.
///    Output flows into `pr_draft` via `{{steps.drift_check.output}}`.
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

/// 7. `pr_draft` — **Agent**.
///    PR body = manifest checklist + open items + drift_check output verbatim.
fn build_pr_draft_step(agent: AgentType, ticket_ref: &str) -> WorkflowStep {
    let prompt = format!(
        "You are the PR-DRAFT step of a Feasibility-Gated AutoPilot run.\n\
         Ticket: {ticket_ref}\n\
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
         <one-line summary of `{{{{steps.run_tests.output}}}}` — passed / skipped / errored>\n\
         \n\
         ## Markers audit (drift_check output)\n\
         <embed `{{{{steps.drift_check.output}}}}` verbatim inside a ``` code fence so reviewers see every traced freedom>\n\
         \n\
         ## Test plan\n\
         <bullet list of how to verify each implemented entry manually, beyond what the test suite covers>\n\
         \n\
         Pull values from `{{{{steps.triage.data}}}}`. End with `[SIGNAL: CONTINUE]`.",
    );
    let mut s = blank_step("pr_draft", StepType::Agent, agent);
    s.description = Some(
        "PR description listing implemented + open + decided + mocked + test verdict + drift audit."
            .into(),
    );
    s.prompt_template = prompt;
    s.agent_settings = Some(blank_agent_settings());
    s.stall_timeout_secs = Some(600);
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
        }
    }

    #[test]
    fn template_has_seven_steps_in_order() {
        let wf = build_feasibility_workflow(default_params());
        let names: Vec<&str> = wf.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["fetch_issue", "triage", "review_triage", "implement", "run_tests", "drift_check", "pr_draft"]
        );
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
        let s = &wf.steps[2];
        assert_eq!(s.step_type, StepType::Gate);
        assert_eq!(s.gate_request_changes_target.as_deref(), Some("triage"));
    }

    #[test]
    fn implement_step_signals_blocked_to_triage() {
        let wf = build_feasibility_workflow(default_params());
        let s = &wf.steps[3];
        assert_eq!(s.step_type, StepType::Agent);
        let rule = s.on_result.first().expect("implement step needs a BLOCKED rule");
        assert_eq!(rule.contains, "BLOCKED");
        match &rule.action {
            ConditionAction::Goto { step_name, max_iterations } => {
                assert_eq!(step_name, "triage");
                assert_eq!(*max_iterations, Some(3));
            }
            other => panic!("expected Goto(triage), got {other:?}"),
        }
    }

    #[test]
    fn implement_step_teaches_linked_repos_evidence_lift() {
        // Lock the implement prompt's cross-repo rule so an unrelated
        // edit can't silently drop it. The whole reason linked_repos
        // are auto-injected (see runner.rs:124 / steps.rs:50) is so the
        // implement step lifts concrete values rather than inventing
        // them. If this rule disappears, v5+ runs regress to v4 levels
        // of MOCKED items even with linked_repos set on the project.
        let wf = build_feasibility_workflow(default_params());
        let s = &wf.steps[3];
        let prompt = &s.prompt_template;
        assert!(
            prompt.contains("evidence:"),
            "implement prompt must teach the `evidence: <repo>/<path>:<line>` lookup format"
        );
        assert!(
            prompt.contains("Linked repositories"),
            "implement prompt must point the agent to the `## Linked repositories` block"
        );
        assert!(
            prompt.contains("READ-ONLY"),
            "implement prompt must mark linked repos as read-only to prevent accidental writes there"
        );
        assert!(
            prompt.contains("lift"),
            "implement prompt must teach `lift` rather than `invent` for evidence-backed values"
        );
    }

    #[test]
    fn run_tests_is_exec_bash_with_auto_detect() {
        let wf = build_feasibility_workflow(default_params());
        let s = &wf.steps[4];
        assert_eq!(s.step_type, StepType::Exec);
        assert_eq!(s.exec_command.as_deref(), Some("bash"));
        let script = s.exec_args.last().expect("bash needs a -c script");
        // Sanity check: the auto-detect covers all 5 framework families
        // we expect to see in production projects.
        for needle in ["make test", "cargo test", "pnpm test", "composer test", "pytest"] {
            assert!(script.contains(needle), "run_tests script must cover '{needle}'");
        }
    }

    #[test]
    fn run_tests_goto_implement_on_error() {
        let wf = build_feasibility_workflow(default_params());
        let s = &wf.steps[4];
        let rule = s.on_result.first().expect("run_tests needs ERROR rule");
        assert_eq!(rule.contains, "ERROR");
        match &rule.action {
            ConditionAction::Goto { step_name, max_iterations } => {
                assert_eq!(step_name, "implement");
                assert_eq!(*max_iterations, Some(2));
            }
            other => panic!("expected Goto(implement), got {other:?}"),
        }
    }

    #[test]
    fn drift_check_is_exec_grep_kronn_markers() {
        let wf = build_feasibility_workflow(default_params());
        let s = &wf.steps[5];
        assert_eq!(s.step_type, StepType::Exec);
        assert_eq!(s.exec_command.as_deref(), Some("bash"));
        let script = s.exec_args.last().unwrap();
        assert!(script.contains("KRONN-(ASSUMED|MOCKED|TODO)"));
        // Must skip heavy dirs.
        assert!(script.contains("--exclude-dir=node_modules"));
        assert!(script.contains("--exclude-dir=vendor"));
    }

    #[test]
    fn pr_draft_embeds_drift_check_and_run_tests_outputs() {
        let wf = build_feasibility_workflow(default_params());
        let s = &wf.steps[6];
        assert_eq!(s.step_type, StepType::Agent);
        // The whole point of having Exec steps before pr_draft is so
        // their output flows verbatim into the PR body. If these
        // template references go missing, the run_tests + drift_check
        // results vanish from the human-facing artifact.
        assert!(
            s.prompt_template.contains("{{steps.run_tests.output}}"),
            "pr_draft must surface run_tests verdict"
        );
        assert!(
            s.prompt_template.contains("{{steps.drift_check.output}}"),
            "pr_draft must surface drift_check markers"
        );
    }

    #[test]
    fn exec_allowlist_covers_all_expected_runners() {
        let wf = build_feasibility_workflow(default_params());
        for needed in ["bash", "grep", "make", "cargo", "pnpm", "composer", "pytest"] {
            assert!(
                wf.exec_allowlist.iter().any(|s| s == needed),
                "exec_allowlist must include '{needed}'"
            );
        }
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
    fn token_cost_audit_three_agent_steps_only() {
        // 0.8.3 design rule (see [[feedback_kronn_deagentify_first]]):
        // we pay LLM tokens only for genuine reasoning — triage,
        // implement, pr_draft. Everything else is deterministic.
        // If a future refactor reverts a step to Agent, this test
        // fails loudly.
        let wf = build_feasibility_workflow(default_params());
        let agent_steps: Vec<&str> = wf
            .steps
            .iter()
            .filter(|s| s.step_type == StepType::Agent)
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(
            agent_steps,
            vec!["triage", "implement", "pr_draft"],
            "Only triage / implement / pr_draft should be Agent steps"
        );
    }
}
