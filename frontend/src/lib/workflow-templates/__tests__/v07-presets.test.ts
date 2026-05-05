// Unit tests for the v07-presets registry (workflow wizard's "Start from a
// pattern" cards). These guard the shape contract that drives the wizard:
//   - all presets have unique stable IDs
//   - the TICKET_TO_PR autopilot ships the expected pipeline (fetch_issue
//     fixture → analyze → plan_gate → implement loop → review → create_pr →
//     ready_gate → notify)
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
    expect(presets).toHaveLength(6);
    const ids = presets.map(p => p.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toEqual(expect.arrayContaining([
      'auto-dev', 'pr-gate', 'deploy-rollback',
      'feature-planner', 'daily-host-audit', 'ticket-to-pr',
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
      'create_pr',
      'ready_gate',
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
