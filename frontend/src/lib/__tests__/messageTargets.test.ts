import { describe, expect, it } from 'vitest';
import { composerMentions, targetsFromComposerText } from '../messageTargets';
import type { ParticipantView } from '../../types/generated';

const cli = (id: number, agent_type: string): ParticipantView => ({
  id,
  disc_id: 'disc-1',
  agent_type,
  session_id: `session-${id}`,
  role: 'peer',
  status: 'active',
  joined_at: '2026-07-29T00:00:00Z',
  left_at: null,
  last_seen: null,
  activity: null,
  presence_state: 'listening',
  read_live: true,
  write_state: 'ok',
  wake_mode: 'external_poll',
  next_poll_at: null,
  last_write_at: null,
  model: null,
  conversation_id: null,
});

const labels = {
  discussionAgent: 'agent de discussion',
  punctualAgent: 'agent ponctuel',
  cli: 'CLI',
  all: 'tous les intervenants',
};

describe('typed composer targets', () => {
  it('distinguishes the configured agent, punctual agents and an exact CLI', () => {
    const mentions = composerMentions(
      'ClaudeCode',
      ['ClaudeCode', 'Codex', 'Vibe'],
      [cli(42, 'Codex')],
      labels,
    );

    expect(mentions).toEqual(expect.arrayContaining([
      expect.objectContaining({
        trigger: '@claude',
        label: 'agent de discussion',
        target: expect.objectContaining({ kind: 'discussion_agent', agent_type: 'ClaudeCode' }),
      }),
      expect.objectContaining({
        trigger: '@codex',
        label: 'agent ponctuel',
        target: expect.objectContaining({ kind: 'agent', agent_type: 'Codex' }),
      }),
      expect.objectContaining({
        trigger: '@codex-cli',
        // KT-211: the CLI entry shows its real alias, never the bare
        // provider trigger it would share with the punctual agent.
        displayTrigger: '@codex-cli',
        label: 'CLI',
        target: expect.objectContaining({ kind: 'cli', cli_session_id: 42 }),
      }),
    ]));
  });

  it('preserves textual order, deduplicates and keeps @all explicit', () => {
    const mentions = composerMentions(
      'ClaudeCode',
      ['ClaudeCode', 'Codex', 'Vibe'],
      [cli(42, 'Codex')],
      labels,
    );
    const result = targetsFromComposerText(
      '@vibe puis @codex-cli et @codex-cli — @all',
      mentions,
    );

    expect(result.targetAll).toBe(true);
    expect(result.targets).toEqual([
      expect.objectContaining({ kind: 'agent', agent_type: 'Vibe' }),
      expect.objectContaining({ kind: 'cli', agent_type: 'Codex', cli_session_id: 42 }),
    ]);
  });

  it('treats mentions in inline and fenced code as documentation', () => {
    const mentions = composerMentions(
      'ClaudeCode',
      ['ClaudeCode', 'Codex', 'Vibe'],
      [cli(42, 'Codex')],
      labels,
    );
    const result = targetsFromComposerText(
      'Explique `@codex` puis ```md\n@codex-cli\n@all\n``` à @vibe.',
      mentions,
    );

    expect(result.targetAll).toBe(false);
    expect(result.targets).toEqual([
      expect.objectContaining({ kind: 'agent', agent_type: 'Vibe' }),
    ]);
  });
});
