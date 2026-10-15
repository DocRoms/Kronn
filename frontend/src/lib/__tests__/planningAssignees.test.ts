import { describe, expect, it } from 'vitest';
import type { ParticipantView, PlanningAssignee } from '../../types/generated';
import {
  assigneeFor,
  assigneeKey,
  assigneeLabel,
  assigneeOptions,
  nextRoleOnToggle,
} from '../planningAssignees';

function participant(overrides: Record<string, unknown>): ParticipantView {
  return {
    id: 1,
    disc_id: 'disc-1',
    agent_type: 'Codex',
    status: 'active',
    cli_ordinal: 1,
    workflow_step: null,
    ...overrides,
  } as unknown as ParticipantView;
}

describe('planningAssignees', () => {
  it('offers the discussion agent and each joined CLI by exact session', () => {
    const options = assigneeOptions('ClaudeCode', [
      participant({ id: 11, cli_ordinal: 1 }),
      participant({ id: 12, cli_ordinal: 2 }),
    ]);

    expect(options.map(option => option.kind)).toEqual(['discussion_agent', 'cli', 'cli']);
    // Two Codex CLIs are two options, never one "Codex".
    expect(new Set(options.map(option => option.key)).size).toBe(3);
    expect(options[2].cli_session_id).toBe(12);
    expect(options[2].cliOrdinal).toBe(2);
  });

  it('skips workflow steps, left sessions and unknown providers', () => {
    const options = assigneeOptions(null, [
      participant({ id: 1, workflow_step: { step_id: 's' } }),
      participant({ id: 2, status: 'left' }),
      participant({ id: 3, agent_type: 'Unknown' }),
      participant({ id: 4 }),
    ]);

    expect(options.map(option => option.cli_session_id)).toEqual([4]);
  });

  it('labels a CLI with its room ordinal so two of one provider differ', () => {
    expect(assigneeLabel({
      kind: 'cli', agent_type: 'Codex', cli_session_id: 1, connection_id: null, cli_ordinal: 1,
    }, 'CLI')).toBe('Codex CLI');
    expect(assigneeLabel({
      kind: 'cli', agent_type: 'Codex', cli_session_id: 2, connection_id: null, cli_ordinal: 2,
    }, 'CLI')).toBe('Codex CLI 2');
    expect(assigneeLabel({
      kind: 'agent', agent_type: 'ClaudeCode', cli_session_id: null, connection_id: null,
    }, 'CLI')).toBe('Claude Code');
  });

  it('matches an assignee to an option on the whole typed identity', () => {
    const assignees: PlanningAssignee[] = [{
      kind: 'cli',
      agent_type: 'Codex',
      cli_session_id: 12,
      role: 'owner',
      source: 'manual',
    }];
    const sameSession = { kind: 'cli' as const, agent_type: 'Codex' as const, cli_session_id: 12 };
    const otherSession = { ...sameSession, cli_session_id: 11 };
    const punctual = { kind: 'agent' as const, agent_type: 'Codex' as const, cli_session_id: null };

    expect(assigneeFor(assignees, sameSession)?.role).toBe('owner');
    expect(assigneeFor(assignees, otherSession)).toBeUndefined();
    expect(assigneeFor(assignees, punctual)).toBeUndefined();
    expect(assigneeKey(sameSession)).not.toBe(assigneeKey(punctual));
  });

  it('toggles a role off when the active one is pressed again', () => {
    expect(nextRoleOnToggle(undefined, 'owner')).toBe('owner');
    expect(nextRoleOnToggle('owner', 'reviewer')).toBe('reviewer');
    expect(nextRoleOnToggle('owner', 'owner')).toBeNull();
  });
});
