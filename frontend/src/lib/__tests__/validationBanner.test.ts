// KT-1122 — what a validation discussion's banner offers: decided from the
// server's verdict, so the UI never proposes what the gate would refuse.
import { describe, it, expect } from 'vitest';
import { validationBanner } from '../validationBanner';
import type { ValidationState } from '../../types/generated';

const state = (over: Partial<ValidationState> = {}): ValidationState => ({
  project_id: 'p1', run_id: 'r1', run_status: 'Completed', discussion_id: 'd-latest',
  archived: false, finished: true, completed_at: '2026-08-28T21:37:00+00:00',
  completed_message_id: 'm1', started_by: 'Romu', validated_by: null, validated_at: null,
  can_validate: true, blocked: null, ...over,
});

describe('validationBanner', () => {
  it('offers to validate a finished validation the server accepts', () => {
    expect(validationBanner(state(), 'd-latest', 'Audited')).toEqual({ kind: 'validate' });
  });

  it('keeps a finished validation finished whatever followed the signal', () => {
    // The server's `finished` is durable: a commit note after the signal does
    // not change what the banner reads.
    expect(validationBanner(state({ finished: true }), 'd-latest', 'Audited').kind).toBe('validate');
  });

  it('shows the reason instead of an action the server would refuse', () => {
    expect(
      validationBanner(state({ can_validate: false, blocked: 'audit_running' }), 'd-latest', 'Audited'),
    ).toEqual({ kind: 'blocked', block: 'audit_running' });
  });

  it('flags a validation of an older audit run and points at the current one', () => {
    expect(validationBanner(state(), 'd-april', 'Audited')).toEqual({
      kind: 'superseded', currentDiscussionId: 'd-latest',
    });
    expect(validationBanner(state({ discussion_id: null }), 'd-april', 'Audited')).toEqual({
      kind: 'superseded', currentDiscussionId: null,
    });
  });

  it('offers nothing while unfinished, unknown, or once the project is validated', () => {
    expect(validationBanner(state({ finished: false, can_validate: false, blocked: 'not_finished' }), 'd-latest', 'Audited').kind).toBe('none');
    expect(validationBanner(null, 'd-latest', 'Audited').kind).toBe('none');
    expect(validationBanner(state(), 'd-latest', 'Validated').kind).toBe('none');
  });
});
