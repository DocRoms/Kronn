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

  // 2026-06-11 — DECOMPOSED: the implement↔test↔review loop now lives in a
  // child workflow (`implement-verify`), referenced by the parent's
  // `implement_verify` SubWorkflow step via `@bundle:implement-verify`. The
  // child shares the parent's worktree (Phase 2 handoff) so `create_pr` (parent)
  // sees the implementation. See docs/design/decomposed-autopilot-presets.md.
  const findChild = () => {
    const p = findTicketToPr();
    const child = p.childWorkflows?.find(c => c.bundleId === 'implement-verify');
    if (!child) throw new Error('implement-verify child workflow not found');
    return child;
  };

  it('parent ships the decomposed 7-step pipeline + on_failure', () => {
    const p = findTicketToPr();
    const stepNames = p.steps.map(s => s.name);
    expect(stepNames).toEqual([
      'fetch_issue',
      'analyze',
      'plan_gate',
      'implement_verify',
      'ready_gate',
      'create_pr',
      'notify_done',
    ]);
    expect(p.onFailure).toBeDefined();
    expect(p.onFailure!.length).toBe(1);
    expect(p.onFailure![0].name).toBe('rollback_notify');
  });

  it('implement_verify is a SubWorkflow referencing the @bundle child', () => {
    const p = findTicketToPr();
    const sub = p.steps.find(s => s.name === 'implement_verify')!;
    expect(sub.step_type).toEqual({ type: 'SubWorkflow' });
    expect(sub.sub_workflow_id).toBe('@bundle:implement-verify');
    // On child failure, re-run the whole child once, else fall through to on_failure.
    const failRule = sub.on_result?.find(r => r.contains === 'SUBWF_FAILED');
    expect(failRule!.action).toEqual({
      type: 'Goto', step_name: 'implement_verify', max_iterations: 1,
    });
  });

  it('ships the implement-verify child workflow (loop, no Gate inside)', () => {
    const child = findChild();
    expect(child.steps.map(s => s.name)).toEqual(['implement', 'run_tests', 'review', 'commit']);
    // `commit` (Exec) persists the reviewed work to the parent branch so create_pr can push it.
    expect((child.steps.at(-1)!.step_type as { type: string }).type).toBe('Exec');
    // Gate is forbidden inside a sub-workflow (validated server-side); the
    // preset must never ship one in the child.
    expect(child.steps.some(s => s.step_type?.type === 'Gate')).toBe(false);
    // Internal Gotos stay inside the child (target `implement`, which exists here).
    const runTests = child.steps.find(s => s.name === 'run_tests')!;
    expect(runTests.on_result?.find(r => r.contains === 'ERROR')!.action).toEqual({
      type: 'Goto', step_name: 'implement', max_iterations: 2,
    });
    const review = child.steps.find(s => s.name === 'review')!;
    expect(review.on_result?.find(r => r.contains === 'NEEDS_CHANGES')!.action).toEqual({
      type: 'Goto', step_name: 'implement', max_iterations: 2,
    });
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
    const parent = (n: string) => p.steps.find(s => s.name === n)!;
    const child = findChild();
    const childStep = (n: string) => child.steps.find(s => s.name === n)!;

    // analyze (parent): writing-plans + brainstorming + verification
    expect(parent('analyze').skill_ids).toEqual(expect.arrayContaining([
      'writing-plans', 'brainstorming', 'verification-before-completion',
    ]));
    // create_pr (parent): finishing-a-development-branch + verification
    expect(parent('create_pr').skill_ids).toEqual(expect.arrayContaining([
      'finishing-a-development-branch', 'verification-before-completion',
    ]));
    // implement (child): tdd + debugging + verification + receiving-code-review
    expect(childStep('implement').skill_ids).toEqual(expect.arrayContaining([
      'test-driven-development',
      'systematic-debugging',
      'verification-before-completion',
      'receiving-code-review',
    ]));
    // review (child): requesting-code-review + verification
    expect(childStep('review').skill_ids).toEqual(expect.arrayContaining([
      'requesting-code-review', 'verification-before-completion',
    ]));
  });

  it('Gates point back to recoverable steps (no dead-ends)', () => {
    const p = findTicketToPr();
    const planGate = p.steps.find(s => s.name === 'plan_gate')!;
    const readyGate = p.steps.find(s => s.name === 'ready_gate')!;
    expect(planGate.gate_request_changes_target).toBe('analyze');
    // INV-1: a parent Gate cannot target a step INSIDE the child — request
    // changes re-runs the whole SubWorkflow step (which re-enters its loop).
    expect(readyGate.gate_request_changes_target).toBe('implement_verify');
  });

  it('child exec_allowlist covers common test runners (parent has none)', () => {
    const child = findChild();
    expect(child.execAllowlist).toEqual(expect.arrayContaining([
      'bash', 'cargo', 'pnpm', 'npm', 'yarn', 'pytest', 'make', 'composer',
    ]));
    // The parent no longer runs Exec — the test loop is in the child.
    const p = findTicketToPr();
    expect(p.steps.some(s => s.step_type?.type === 'Exec')).toBe(false);
  });

  it('child run_tests uses the generic auto-detect bash script', () => {
    const child = findChild();
    const runTests = child.steps.find(s => s.name === 'run_tests')!;
    expect(runTests.exec_command).toBe('bash');
    const args = runTests.exec_args ?? [];
    expect(args[0]).toBe('-c');
    const script = args[1] ?? '';
    expect(script).toContain('Makefile');
    expect(script).toContain('Cargo.toml');
    expect(script).toContain('package.json');
    expect(script).toContain('composer.json');
    expect(script).toContain('pyproject.toml');
    expect(script).toContain('[SIGNAL: SKIPPED]');
  });

  it('child implement and review steps carry an auto-retry on transient CLI exits', () => {
    const child = findChild();
    const implement = child.steps.find(s => s.name === 'implement')!;
    const review = child.steps.find(s => s.name === 'review')!;
    expect(implement.retry).toEqual({ max_retries: 1, backoff: 'exponential' });
    expect(review.retry).toEqual({ max_retries: 1, backoff: 'exponential' });
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

  // 2026-06-11 (PR-C) — DECOMPOSED, mirror of the Rust split (parent +
  // build_feasibility_child). Parent keeps the human-gated triage; the
  // implement/test/drift loop is a child sub-workflow sharing the worktree.
  const findFAChild = () => {
    const child = findFA().childWorkflows?.find(c => c.bundleId === 'fa-implement-verify');
    if (!child) throw new Error('fa-implement-verify child not found');
    return child;
  };

  it('parent ships the two-brains 9-step pipeline (mirror of the Rust endpoint)', () => {
    const p = findFA();
    expect(p.steps.map(s => s.name)).toEqual([
      'fetch_issue',
      'triage',
      'plan_lint',
      'review_triage',
      'test_baseline',
      'feasibility_impl',
      'run_tests',
      'drift_check',
      'pr_draft',
    ]);
  });

  it('two-brains: triage debates with a Codex reviewer (multi_agent_review), no separate plan_review step', () => {
    const p = findFA();
    const triage = p.steps.find(s => s.name === 'triage')!;
    expect(triage.agent_settings?.tier).toBe('reasoning');
    expect(triage.multi_agent_review?.reviewer_agent).toBe('Codex');
    expect(triage.multi_agent_review?.reviewer_tier).toBe('reasoning');
    expect(p.steps.some(s => s.name === 'plan_review')).toBe(false);
  });

  it('plan_lint is a 0-token Exec surfacing the engine-written lint report', () => {
    const lint = findFA().steps.find(s => s.name === 'plan_lint')!;
    expect((lint.step_type as { type: string }).type).toBe('Exec');
    expect(lint.exec_args?.[1]).toContain('plan_lint.txt');
  });

  it('child = implement → static_checks → scope_check → completeness_check → commit (no Gate, suites at parent)', () => {
    const child = findFAChild();
    expect(child.steps.map(s => s.name)).toEqual(['implement', 'item_tests', 'scope_check', 'completeness_check', 'commit']);
    expect(child.steps.some(s => (s.step_type as { type: string }).type === 'Gate')).toBe(false);
    // commit (Exec) persists the Phase-0 work to the parent branch (survives cleanup).
    expect((child.steps.at(-1)!.step_type as { type: string }).type).toBe('Exec');
    // completeness_check (Exec, 0 token) loops back to implement on a missing marker.
    const cc = child.steps.find(s => s.name === 'completeness_check')!;
    expect((cc.step_type as { type: string }).type).toBe('Exec');
    const rule = cc.on_result?.find(r => r.contains === 'exit_3');
    expect((rule?.action as { step_name: string }).step_name).toBe('implement');
  });

  it('désagentification preserved across the split (triage/pr_draft parent, implement child)', () => {
    // [[feedback_kronn_deagentify_first]] — never regress to all-Agent.
    const p = findFA();
    const parentAgents = p.steps
      .filter(s => (s.step_type as { type: string }).type === 'Agent')
      .map(s => s.name);
    expect(parentAgents).toEqual(['triage', 'pr_draft']);
    const childAgents = findFAChild().steps
      .filter(s => (s.step_type as { type: string }).type === 'Agent')
      .map(s => s.name);
    expect(childAgents).toEqual(['implement']);
  });

  it('triage step uses TypedSchema with on_invalid=Fail', () => {
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

  it('feasibility_impl is a SubWorkflow re-triaging on SUBWF_FAILED (cap 3)', () => {
    // The old `implement BLOCKED → Goto(triage)` is reconstructed at the
    // parent: a failed child run re-triages.
    const p = findFA();
    const sub = p.steps.find(s => s.name === 'feasibility_impl')!;
    expect((sub.step_type as { type: string }).type).toBe('SubWorkflow');
    expect(sub.sub_workflow_id).toBe('@bundle:fa-implement-verify');
    const rule = sub.on_result?.[0];
    expect(rule?.contains).toBe('SUBWF_FAILED');
    expect((rule?.action as { step_name: string }).step_name).toBe('triage');
    expect((rule?.action as { max_iterations: number }).max_iterations).toBe(3);
  });

  it('child item_tests runs scoped tests, loops back to implement until green (cap 3)', () => {
    const sc = findFAChild().steps.find(s => s.name === 'item_tests')!;
    expect((sc.step_type as { type: string }).type).toBe('Exec');
    expect(sc.exec_command).toBe('bash');
    const script = sc.exec_args?.[1] ?? '';
    expect(script).toContain('php -l');
    expect(script).toContain('jest --findRelatedTests'); // JS scoped to changed files
    expect(script).toContain('--filter'); // PHP scoped to changed test classes
    expect(script).toContain('item-test-failures.txt'); // failures fed back to implement
    // Exec inline [SIGNAL] is swallowed by the envelope — exit codes branch (run-4 lesson): exit 2 → `exit_2`.
    expect(script).toContain('exit 2');
    const rule = sc.on_result?.find(r => r.contains === 'exit_2');
    expect((rule?.action as { step_name: string }).step_name).toBe('implement');
    expect((rule?.action as { max_iterations: number }).max_iterations).toBe(3); // "until green", bounded
  });

  it('parent run_tests v3: JS in-container (coverage≠fail) + PHP via project docker stack', () => {
    const rt = findFA().steps.find(s => s.name === 'run_tests')!;
    expect((rt.step_type as { type: string }).type).toBe('Exec');
    const script = rt.exec_args?.[1] ?? '';
    // JS: jest in the Kronn container; a coverage-gate exit is NOT a test failure (run-10)
    expect(script).toContain('--coverage=false');
    expect(script).toContain('lint/coverage gate');
    // PHP: project's dockerized php service, worktree-mounted (no local install)
    expect(script).toContain('docker compose -f');
    expect(script).toContain('vendor/bin/phpunit -c phpunit.xml.dist');
    expect(script).toContain('hosttr'); // container→host path translation for bind mounts
    expect(script).toContain('no dockerized php stack'); // honest SKIP fallback, not a false FAIL
    // verdict quoted by pr_draft; exit 0 always (failures documented, not fatal)
    expect(script).toContain('TEST VERDICT — JS: $js | PHP: $php_v');
    expect(script.trimEnd().endsWith('exit 0')).toBe(true);
    // read-only integration verdict — no parent fix loop (the test→fix loop is in the child item_tests)
    expect(rt.on_result ?? []).toHaveLength(0);
    expect(findFA().steps.some(s => s.name === 'fix_tests')).toBe(false);
  });

  it('test_baseline records the pre-existing failures before the fan-out', () => {
    const p = findFA();
    const tb = p.steps.find(s => s.name === 'test_baseline')!;
    expect((tb.step_type as { type: string }).type).toBe('Exec');
    expect(tb.exec_args?.[1]).toContain('known-failing.txt');
    const pos = (n: string) => p.steps.findIndex(s => s.name === n);
    expect(pos('test_baseline')).toBeLessThan(pos('feasibility_impl'));
    // child item_tests is baseline-aware (loops only on net-new)
    const it = findFAChild().steps.find(s => s.name === 'item_tests')!;
    expect(it.exec_args?.[1]).toContain('known-failing.txt');
    expect(it.exec_args?.[1]).toContain('NET-NEW');
  });

  it('parent drift_check greps KRONN markers over the FINAL worktree, skipping heavy dirs', () => {
    const dc = findFA().steps.find(s => s.name === 'drift_check')!;
    expect((dc.step_type as { type: string }).type).toBe('Exec');
    const script = dc.exec_args?.[1] ?? '';
    expect(script).toContain('KRONN-(ASSUMED|MOCKED|TODO)');
    expect(script).toContain('--exclude-dir=node_modules');
    expect(script).toContain('--exclude-dir=vendor');
    expect(findFAChild().steps.some(s => s.name === 'drift_check')).toBe(false);
  });

  it('pr_draft wires prompt + description i18n keys', () => {
    const p = findFA();
    const pr = p.steps.find(s => s.name === 'pr_draft')!;
    expect(pr.prompt_template).toBe('wiz.preset.feasibilityAutopilot.prDraftPrompt');
    expect(pr.description).toBe('wiz.preset.feasibilityAutopilot.prDraftDesc');
  });

  it('exec allowlists: child covers the checkers, parent covers the suites', () => {
    const child = findFAChild();
    for (const bin of ['bash', 'grep', 'git']) {
      expect(child.execAllowlist).toContain(bin);
    }
    const p = findFA();
    for (const bin of ['bash', 'grep', 'git', 'yarn', 'npm', 'php']) {
      expect(p.execAllowlist).toContain(bin);
    }
  });
});
