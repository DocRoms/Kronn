// Unit tests for the v07-presets registry (workflow wizard's "Start from a
// pattern" cards). These guard the shape contract that drives the wizard:
//   - all presets have unique stable IDs
//   - the TICKET_TO_PR autopilot ships the expected pipeline (fetch_issue
//     fixture → analyze → plan_gate → implement loop → review → ready_gate →
//     create_pr → notify) — gate BEFORE the push/PR
//   - skill_ids on Agent steps point at known external skills (vendored)
//
// Drift on any of these = a broken preset at click time. The wizard renders
// these cards as the user's "first 30 seconds" experience, so silent drift
// here is the worst kind of UX bug.

import { describe, it, expect } from 'vitest';
import { buildV07Presets } from '../v07-presets';

const t = (key: string) => key;

describe('v07 presets registry', () => {
  it('exposes 6 presets with unique IDs', () => {
    const presets = buildV07Presets(t);
    expect(presets).toHaveLength(7);
    const ids = presets.map(p => p.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toEqual(expect.arrayContaining([
      'auto-dev', 'pr-gate', 'deploy-rollback',
      'feature-planner', 'daily-host-audit', 'ticket-to-pr',
      'feasibility-autopilot',
    ]));
  });

  it('every preset has a non-empty steps array', () => {
    const presets = buildV07Presets(t);
    for (const p of presets) {
      expect(p.steps.length, `preset ${p.id} has no steps`).toBeGreaterThan(0);
    }
  });
});

describe('TICKET_TO_PR preset', () => {
  const findTicketToPr = () => {
    const p = buildV07Presets(t).find(x => x.id === 'ticket-to-pr');
    if (!p) throw new Error('ticket-to-pr preset not found');
    return p;
  };

  it('ships the expected 9-step pipeline + on_failure', () => {
    const p = findTicketToPr();
    const stepNames = p.steps.map(s => s.name);
    expect(stepNames).toEqual([
      'fetch_issue',
      'analyze',
      'plan_gate',
      'implement',
      'run_tests',
      'review',
      'ready_gate',
      'create_pr',
      'notify_done',
    ]);
    expect(p.onFailure).toBeDefined();
    expect(p.onFailure!.length).toBe(1);
    expect(p.onFailure![0].name).toBe('rollback_notify');
  });

  it('starts with a JsonData fixture (testable without tracker plugin)', () => {
    const p = findTicketToPr();
    const fetchIssue = p.steps[0];
    expect(fetchIssue.step_type).toEqual({ type: 'JsonData' });
    expect(fetchIssue.output_format).toEqual({ type: 'Structured' });
    expect(fetchIssue.json_data_payload).toBeDefined();
    // Payload has the canonical ticket shape that downstream prompts expect.
    const payload = fetchIssue.json_data_payload as Record<string, unknown>;
    expect(payload).toHaveProperty('key');
    expect(payload).toHaveProperty('title');
    expect(payload).toHaveProperty('description');
  });

  it('Agent steps reference vendored external skills', () => {
    const p = findTicketToPr();
    const byName = (n: string) => p.steps.find(s => s.name === n)!;

    // analyze: writing-plans + brainstorming + verification
    expect(byName('analyze').skill_ids).toEqual(expect.arrayContaining([
      'writing-plans', 'brainstorming', 'verification-before-completion',
    ]));
    // implement: tdd + debugging + verification + receiving-code-review
    expect(byName('implement').skill_ids).toEqual(expect.arrayContaining([
      'test-driven-development',
      'systematic-debugging',
      'verification-before-completion',
      'receiving-code-review',
    ]));
    // review: requesting-code-review + verification
    expect(byName('review').skill_ids).toEqual(expect.arrayContaining([
      'requesting-code-review', 'verification-before-completion',
    ]));
    // create_pr: finishing-a-development-branch + verification
    expect(byName('create_pr').skill_ids).toEqual(expect.arrayContaining([
      'finishing-a-development-branch', 'verification-before-completion',
    ]));
  });

  it('implement → run_tests loop is wired with Goto + max_iterations=2', () => {
    // 0.7.0 — lowered from 5 to 2 to bound token burn on heavy tickets.
    // Two retries on test failure is enough for typical TDD red-green
    // recoveries; beyond that something deeper is wrong and the loop
    // would just rack up Claude tokens for nothing.
    const p = findTicketToPr();
    const runTests = p.steps.find(s => s.name === 'run_tests')!;
    const errorRule = runTests.on_result?.find(r => r.contains === 'ERROR');
    expect(errorRule).toBeDefined();
    expect(errorRule!.action).toEqual({
      type: 'Goto', step_name: 'implement', max_iterations: 2,
    });
  });

  it('review → implement loop on NEEDS_CHANGES, fall-through on APPROVED', () => {
    // 0.7.0 — APPROVED no longer has an explicit Stop rule.
    // The natural fall-through to the next step (`create_pr`) is what we
    // want; the prior `APPROVED → Stop` action terminated the workflow
    // prematurely so create_pr / ready_gate / notify_done never ran.
    const p = findTicketToPr();
    const review = p.steps.find(s => s.name === 'review')!;
    const needsChanges = review.on_result?.find(r => r.contains === 'NEEDS_CHANGES');
    const approved = review.on_result?.find(r => r.contains === 'APPROVED');
    expect(needsChanges?.action).toEqual({
      type: 'Goto', step_name: 'implement', max_iterations: 2,
    });
    expect(approved).toBeUndefined();
  });

  it('Gates point back to recoverable steps (no dead-ends)', () => {
    const p = findTicketToPr();
    const planGate = p.steps.find(s => s.name === 'plan_gate')!;
    const readyGate = p.steps.find(s => s.name === 'ready_gate')!;
    expect(planGate.gate_request_changes_target).toBe('analyze');
    expect(readyGate.gate_request_changes_target).toBe('implement');
  });

  it('exec_allowlist covers common test runners', () => {
    // 0.7.0 — added bash (for the generic test-detect script) plus pnpm,
    // yarn, composer so the preset works on any project, not only Rust.
    const p = findTicketToPr();
    expect(p.execAllowlist).toEqual(expect.arrayContaining([
      'bash', 'cargo', 'pnpm', 'npm', 'yarn', 'pytest', 'make', 'composer',
    ]));
  });

  it('run_tests step uses the generic auto-detect bash script', () => {
    // 0.7.0 — replaces hardcoded `cargo test` with a bash probe that adapts
    // to whatever stack the worktree carries. Without this, the preset
    // only worked on Rust projects (front_euronews regressed silently).
    const p = findTicketToPr();
    const runTests = p.steps.find(s => s.name === 'run_tests')!;
    expect(runTests.exec_command).toBe('bash');
    const args = runTests.exec_args ?? [];
    expect(args[0]).toBe('-c');
    const script = args[1] ?? '';
    // Sanity: each major framework probe is present.
    expect(script).toContain('Makefile');
    expect(script).toContain('Cargo.toml');
    expect(script).toContain('package.json');
    expect(script).toContain('composer.json');
    expect(script).toContain('pyproject.toml');
    expect(script).toContain('[SIGNAL: SKIPPED]');
  });

  it('implement and review steps carry an auto-retry on transient CLI exits', () => {
    // 0.7.0 — Claude Code CLI sometimes silently exits 1 mid-stream on
    // long sessions (~25 min mark, no stderr). Retrying once almost
    // always recovers since the failure is transient.
    const p = findTicketToPr();
    const implement = p.steps.find(s => s.name === 'implement')!;
    const review = p.steps.find(s => s.name === 'review')!;
    expect(implement.retry).toEqual({ max_retries: 1, backoff: 'exponential' });
    expect(review.retry).toEqual({ max_retries: 1, backoff: 'exponential' });
    // 30 min stall ceiling on these two — heavy enterprise tickets
    // routinely hit 20-25 min of streamed activity.
    expect(implement.stall_timeout_secs).toBe(1800);
    expect(review.stall_timeout_secs).toBe(1800);
  });
});

// 0.8.3 — Feasibility-Gated AutoPilot preset. Mirror of the Rust
// `build_feasibility_workflow` shape (see backend/src/workflows/
// big_ticket_template.rs). If the two drift, the AutoPilot CTA path
// produces a different workflow than the `/api/workflows/templates/
// feasibility-autopilot` endpoint — caught here.
describe('FEASIBILITY_AUTOPILOT preset', () => {
  const findFA = () => {
    const p = buildV07Presets(t).find(x => x.id === 'feasibility-autopilot');
    if (!p) throw new Error('feasibility-autopilot preset not found');
    return p;
  };

  it('ships the expected 7-step pipeline (mixed primitives)', () => {
    const p = findFA();
    expect(p.steps.map(s => s.name)).toEqual([
      'fetch_issue',
      'triage',
      'review_triage',
      'implement',
      'run_tests',
      'drift_check',
      'pr_draft',
    ]);
  });

  it('only triage / implement / pr_draft are Agent — désagentification rule', () => {
    // [[feedback_kronn_deagentify_first]] — never let this preset
    // regress to all-Agent. Token cost = 0 on fetch_issue (JsonData),
    // review_triage (Gate), run_tests (Exec), drift_check (Exec).
    const p = findFA();
    const agentSteps = p.steps
      .filter(s => (s.step_type as { type: string }).type === 'Agent')
      .map(s => s.name);
    expect(agentSteps).toEqual(['triage', 'implement', 'pr_draft']);
  });

  it('triage step uses TypedSchema with on_invalid=Fail', () => {
    // The `[TRIAGE]` marker prefix lives in the i18n string at runtime
    // (see `wiz.preset.feasibilityAutopilot.triageDesc` in i18n.ts).
    // This test only asserts the structural contract — the literal
    // marker substring is checked by `i18n.test.ts` and by
    // `triage::is_triage_step` on the backend.
    const p = findFA();
    const triage = p.steps.find(s => s.name === 'triage')!;
    expect(triage.description).toBe('wiz.preset.feasibilityAutopilot.triageDesc');
    const fmt = triage.output_format as { type: string; on_invalid?: string; schema?: unknown };
    expect(fmt.type).toBe('TypedSchema');
    expect(fmt.on_invalid).toBe('Fail');
  });

  it('gate routes RequestChanges back to triage', () => {
    const p = findFA();
    const gate = p.steps.find(s => s.name === 'review_triage')!;
    expect(gate.gate_request_changes_target).toBe('triage');
  });

  it('implement signals BLOCKED → Goto(triage) capped at 3', () => {
    const p = findFA();
    const impl = p.steps.find(s => s.name === 'implement')!;
    const rule = impl.on_result?.[0];
    expect(rule?.contains).toBe('BLOCKED');
    expect((rule?.action as { step_name: string }).step_name).toBe('triage');
    expect((rule?.action as { max_iterations: number }).max_iterations).toBe(3);
  });

  it('run_tests is Exec bash with auto-detect across stacks', () => {
    const p = findFA();
    const rt = p.steps.find(s => s.name === 'run_tests')!;
    expect((rt.step_type as { type: string }).type).toBe('Exec');
    expect(rt.exec_command).toBe('bash');
    const script = rt.exec_args?.[1] ?? '';
    for (const needle of ['make test', 'cargo test', 'pnpm test', 'composer test', 'pytest']) {
      expect(script).toContain(needle);
    }
  });

  it('drift_check is Exec greping KRONN markers, skipping heavy dirs', () => {
    const p = findFA();
    const dc = p.steps.find(s => s.name === 'drift_check')!;
    expect((dc.step_type as { type: string }).type).toBe('Exec');
    const script = dc.exec_args?.[1] ?? '';
    expect(script).toContain('KRONN-(ASSUMED|MOCKED|TODO)');
    expect(script).toContain('--exclude-dir=node_modules');
    expect(script).toContain('--exclude-dir=vendor');
  });

  it('pr_draft wires prompt + description i18n keys', () => {
    // Content is in i18n.ts; here we just guard the wiring contract.
    // A drift between the key emitted by the preset and what the
    // wizard renders breaks the run silently — surfacing the key
    // here lets a CI search match the actual i18n.ts entry.
    const p = findFA();
    const pr = p.steps.find(s => s.name === 'pr_draft')!;
    expect(pr.prompt_template).toBe('wiz.preset.feasibilityAutopilot.prDraftPrompt');
    expect(pr.description).toBe('wiz.preset.feasibilityAutopilot.prDraftDesc');
  });

  it('exec_allowlist covers every runner the Exec steps may need', () => {
    const p = findFA();
    for (const bin of ['bash', 'grep', 'make', 'cargo', 'pnpm', 'composer', 'pytest']) {
      expect(p.execAllowlist).toContain(bin);
    }
  });
});
