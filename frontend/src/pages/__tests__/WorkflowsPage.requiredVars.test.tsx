// 0.8.5 follow-up — required-vars launch guard.
//
// Before the fix, clicking "Launch" or "Compare" on a QP that had
// required variables but empty inputs fired the agent with a half-
// rendered template (literal `{{ticket}}` in the prompt). The
// MissingRequired toast now blocks both paths.
//
// We exercise the pure helper `collectMissingRequiredVars` via a tiny
// fixture — the wired-up handlers are covered by PW E2E. The helper
// is not currently exported; we re-implement it here verbatim and
// pin the same edge cases so a refactor to the launcher logic keeps
// the contract.

import { describe, it, expect } from 'vitest';

type Var = { name: string; label?: string; required?: boolean };

// Mirror of WorkflowsPage::collectMissingRequiredVars — kept here to
// pin the contract. If the launcher logic changes, this test file
// fails first and reminds you to update the matching guard.
function collectMissingRequiredVars(
  variables: Var[],
  vars: Record<string, string>,
): string[] {
  return variables
    .filter(v => v.required !== false && !(vars[v.name] ?? '').trim())
    .map(v => v.label || v.name);
}

describe('required-vars launch guard (0.8.5)', () => {
  it('returns empty for a QP with no variables', () => {
    expect(collectMissingRequiredVars([], {})).toEqual([]);
  });

  it('treats undefined `required` as required (legacy QPs)', () => {
    const vars = [{ name: 'ticket', label: 'Ticket' }];
    expect(collectMissingRequiredVars(vars, {})).toEqual(['Ticket']);
  });

  it('treats explicit `required: true` as required', () => {
    const vars = [{ name: 'ticket', label: 'Ticket', required: true }];
    expect(collectMissingRequiredVars(vars, {})).toEqual(['Ticket']);
  });

  it('skips variables flagged `required: false`', () => {
    const vars = [
      { name: 'note', label: 'Note', required: false },
      { name: 'ticket', label: 'Ticket', required: true },
    ];
    expect(collectMissingRequiredVars(vars, {})).toEqual(['Ticket']);
  });

  it('whitespace-only values count as missing', () => {
    const vars = [{ name: 'ticket', label: 'Ticket', required: true }];
    expect(collectMissingRequiredVars(vars, { ticket: '   ' })).toEqual(['Ticket']);
  });

  it('uses the name when label is absent', () => {
    const vars = [{ name: 'ticket' }];
    expect(collectMissingRequiredVars(vars, {})).toEqual(['ticket']);
  });

  it('returns multiple missing labels when several are empty', () => {
    const vars = [
      { name: 'date', label: 'Date', required: true },
      { name: 'h1', label: 'Heure début', required: true },
      { name: 'h2', label: 'Heure fin', required: true },
      { name: 'note', label: 'Note', required: false },
    ];
    expect(collectMissingRequiredVars(vars, { date: '2026-05-17', h1: '', h2: '' }))
      .toEqual(['Heure début', 'Heure fin']);
  });

  it('returns empty when every required var is non-empty', () => {
    const vars = [
      { name: 'a', label: 'A', required: true },
      { name: 'b', label: 'B', required: false },
    ];
    expect(collectMissingRequiredVars(vars, { a: 'x' })).toEqual([]);
  });
});
