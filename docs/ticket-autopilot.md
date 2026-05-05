# Ticket Autopilot — workflow guide (0.7+)

A multi-step workflow that takes **one ticket** as input and drives it all the way to a **pull
request ready to merge**, with human approval gates at the key decision points.

## TL;DR — try it in 30 seconds

1. Open Kronn → **Automatisation → Workflows → Nouveau workflow**
2. In "Démarrer depuis un pattern", click **🎫 Ticket Autopilot**
3. Click **Sauvegarder** (the preset works as-is on the bundled fixture)
4. Click **Lancer** → the run starts streaming step-by-step
5. The first **Gate** pauses on the validation of the plan — read it, click **Approuver**
6. The agent implements, runs tests, gets reviewed by another agent, creates the PR
7. The second **Gate** pauses on "PR ready to merge" — review the PR link, **Approuver** to finalize

That's the happy path. Below: how to wire it on a real ticket, customize each step, and understand
what's happening under the hood.

---

## Pipeline at a glance

```
🎫 fetch_issue (JsonData fixture by default)
     │
     ▼
📋 analyze (Agent — writing-plans + brainstorming + verification)
     │  produces structured plan: subtasks, complexity, test_strategy
     ▼
✋ plan_gate (you validate the plan)
     │
     ▼
🔴 implement (Agent — TDD + debugging + verification + receiving-code-review)
     │  ←─── reads state.last_review if a previous review left feedback
     ▼
⚙ run_tests (Exec — cargo test by default)
     │  ── ERROR ──→ goto implement (max 5 cycles)
     ▼
🔎 review (Agent — requesting-code-review + verification)
     │  ── NEEDS_CHANGES ──→ writes state.last_review, goto implement (max 5)
     │  ── APPROVED ──→ continue
     ▼
🏁 create_pr (Agent — finishing-a-development-branch + verification)
     │  produces state.pr_url, state.pr_number
     ▼
✋ ready_gate (you validate the PR before merge)
     │
     ▼
🔔 notify_done (Notify webhook — Slack by default)

──── on_failure ────
🚨 rollback_notify (alert webhook with failed step name + output)
```

## The 9 steps explained

### 1. `fetch_issue` (JsonData)

**Default**: fixture with a demo ticket (`{key, title, description, labels, priority}`). The
preset runs immediately, no plugin required.

**Real-world swap**: change the step type from `JsonData` to `ApiCall`, point at your tracker
plugin (Jira, GitHub Issues, Linear). The downstream prompts read `{{steps.fetch_issue.data}}`
in a tracker-agnostic way — they don't care which tracker fed them, as long as the JSON
structure has `title` and `description` fields.

If you have several tickets to process, you can also wire the preset on a **Tracker trigger**
(workflow runs automatically on each new matching issue) — but be mindful of the cost.

### 2. `analyze` (Agent)

Loads three vendored skills: `writing-plans`, `brainstorming`, `verification-before-completion`.

The agent:
- explores the ticket's intent (3-5 critical questions)
- breaks it into subtasks (1-3 days each max)
- defines a test strategy
- emits a Structured envelope with `data.subtasks`, `data.complexity`, `data.test_strategy`

### 3. `plan_gate` (Gate)

The plan is shown to you with the subtasks + test strategy. You either:
- **Approve** → implementation starts
- **Request changes** → you write your feedback in the comment, the workflow loops back to
  `analyze` to re-plan with your input
- **Reject** → workflow terminates, no implementation

### 4. `implement` (Agent)

Loads four vendored skills: `test-driven-development`, `systematic-debugging`,
`verification-before-completion`, `receiving-code-review`.

The agent:
- For each subtask in the plan: writes failing tests first (red), then minimal code (green),
  then refactors. This is the **strict TDD ritual** from `obra/superpowers` — no production
  code without a failing test first.
- If a test breaks unexpectedly, applies systematic 4-phase root-cause analysis (no shotgun
  fixes).
- If `state.last_review` is present (we've looped back from review), the agent reads the
  feedback and applies it surgically.
- Refuses to claim "done" without running and verifying the test command output.

### 5. `run_tests` (Exec)

Runs `cargo test` by default. **Adapt to your stack** :
- Rust: `cargo` `test`
- Node: `npm` `test`
- Python: `pytest`
- Make: `make` `test`

The binary must be in `Workflow.exec_allowlist` (the preset declares `cargo`, `npm`, `pytest`,
`make`). If tests fail (non-zero exit code), the step emits `[SIGNAL: ERROR]` and the workflow
loops back to `implement` (capped at 5 cycles).

### 6. `review` (Agent)

Loads `requesting-code-review` + `verification-before-completion`.

**Recommended UX**: change the agent on this step to a **different one** than `implement`. The
preset defaults to ClaudeCode for both (so it works out-of-the-box), but a different agent
(Codex, Gemini CLI) on review fights confirmation bias — the reviewer agent doesn't know what
the implementer agent assumed.

The agent:
- Verifies the implementation covers ALL plan subtasks (not partial)
- YAGNI check: did the implementer add unrequested complexity?
- Security check: injections, secret leaks, input validation
- Edge cases: null, empty, unicode, large input
- On approval: ends with `[SIGNAL: APPROVED]`
- On rejection: writes `state.last_review=<feedback>` + `[SIGNAL: NEEDS_CHANGES]` →
  loops back to `implement` (capped at 5)

### 7. `create_pr` (Agent)

Loads `finishing-a-development-branch` + `verification-before-completion`.

The agent:
- Verifies one last time that all tests pass
- Pushes the current branch
- Creates the PR via `gh pr create` (or equivalent) with structured body (Summary + Test Plan)
- Stores the PR URL in `state.pr_url`

### 8. `ready_gate` (Gate)

The PR URL is shown. You either:
- **Approve** → workflow finalizes (notify)
- **Request changes** → loops back to `implement` with your feedback (which the agent picks
  up via `state.last_review`)

This is your **last chance** to block before any merge action. The default preset doesn't
auto-merge — that's intentional in v1 (Sprint 1). Auto-merge via ApiCall lands in v2.

### 9. `notify_done` (Notify)

Sends a webhook (Slack by default) with the PR URL + review summary. Edit the `notify_config.url`
to point at your team's webhook (Slack, Teams, Discord, custom).

## Customizing the preset

After clicking the preset card, **all fields are editable**. Common customizations:

| Want | How |
|------|-----|
| Wire a real ticket source | Change `fetch_issue` step type from `JsonData` to `ApiCall`, pick your tracker plugin |
| Different test runner | Change `run_tests.exec_command` (and update `exec_allowlist` if needed) |
| Different review agent | Change `review.agent` from `ClaudeCode` to `Codex` / `GeminiCli` |
| Different webhook | Change `notify_done.notify_config.url` |
| Stricter loops (less retries) | Change `max_iterations: 5` to `2` or `3` on the on_result rules |
| Skip the plan gate on simple tickets | Sprint 2 ships `skip_if` — use it with `skip_if: "{{steps.analyze.data.complexity}} == 'low'"` |

## Limitations (v1 — Sprint 1, May 2026)

- **No automatic CI wait**. Today the human validates the PR via `ready_gate`. Sprint 3 ships a
  `Wait` step that polls the CI endpoint until success/failure.
- **No automatic handling of human review comments after merge**. If the reviewer comments on
  the PR after merge, you re-trigger the workflow manually with the same ticket. Sprint 4
  ships a webhook receiver — GitHub will be able to ping Kronn on review_comment events.
- **No "Agent asks human mid-run"**. Today the agent answers its own questions or ships
  potentially-wrong code. Sprint 2 ships `skip_if` + the Ask Human pattern (agent emits
  `state.human_question`, a conditional Gate fires, you answer, the agent resumes).
- **No auto-merge**. Sprint 1 always pauses on `ready_gate`. v2 will let you opt in to auto-
  merge via an `ApiCall` step (PUT `/pulls/:n/merge`).

## Why these specific skills?

The 8 skills bundled in `backend/src/skills/external/` are **methodology** skills (orthogonal
to your domain skills like Rust / React / Python). They're vendored from
[obra/superpowers](https://github.com/obra/superpowers) (MIT, 40K+ ⭐), the most battle-tested
multi-agent development methodology in the ecosystem (174K stars at peak).

The preset combines them into a coherent ritual:

| Skill | Where it kicks in | Why |
|-------|-------------------|-----|
| `writing-plans` + `brainstorming` | analyze | Force exploration of intent before code |
| `test-driven-development` | implement | Strict red-green-refactor — no production code without a failing test |
| `systematic-debugging` | implement | 4-phase root-cause when tests break unexpectedly |
| `verification-before-completion` | every Agent step | Anti "done = compiled" — no claim without evidence |
| `receiving-code-review` | implement (loop) | Technique for applying review feedback rigorously |
| `requesting-code-review` | review | Structures the review (priority, blocking issues, YAGNI) |
| `finishing-a-development-branch` | create_pr | Structured PR body, verify-tests-first |

See `THIRD_PARTY_SKILLS.md` at the repo root for licenses, source URLs, and update process.

## Troubleshooting

### "The agent ignored the plan and went off-script"

Check that the `skill_ids` on the `implement` step include `verification-before-completion`. This
is the skill that forces the agent to verify it covered all subtasks. If it's missing, the
agent may declare done after one subtask.

### "The review never approves, infinite loop"

The on_result rules cap at `max_iterations: 5` — after that, the workflow status becomes
`StoppedByGuard` (orange in the UI, distinct from Failed). Review the captured runs in the
RunDetail page to understand what the reviewer agent keeps requesting.

### "The PR creation failed because the branch wasn't pushed"

The agent uses Bash + `gh` CLI. Ensure your project has `gh` installed and authenticated. Check
the `create_pr` step's stdout in RunDetail for the exact error.

### "I want to use a custom Quick API instead of the JsonData fixture"

After applying the preset, click the `fetch_issue` step → change the type from `JsonData` to
`ApiCall` → in the `ApiCallStepCard`, pick your saved Quick API from the "Depuis un Quick API
existant" dropdown. The downstream prompts (`{{steps.fetch_issue.data}}`) keep working
unchanged.

## Roadmap

| Sprint | What ships | Release |
|--------|-----------|---------|
| 2 | `skip_if` + Ask Human pattern | 0.7.0 |
| 3 | `Wait`/`Poll` step + Webhook receiver | 0.7.0 |
| 4 | Memory across runs (graph + Obsidian-backed MCP) | 0.8.0 |

The current preset (Sprint 1) is the **synchronous** baseline. Async features (CI poll, agent
asks human, memory) layer on top in subsequent sprints.

---

*Vendored skills attribution: [obra/superpowers](https://github.com/obra/superpowers) (MIT,
commit `e7a2d164`, imported 2026-05-04). See `THIRD_PARTY_SKILLS.md` for the full bill of
materials.*
