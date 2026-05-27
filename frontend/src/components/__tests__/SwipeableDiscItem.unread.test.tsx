/**
 * 0.8.7 — unread-badge regression.
 *
 * Reported live: 26 discussions × ~2 user-facing messages each showed a
 * top-level "à lire" badge of 400+. Workflow runs persist tool calls +
 * cached-summary lines as `MessageRole::System` rows, which inflate
 * `message_count`. The badge basis must be `non_system_message_count`.
 *
 * These tests pin the `unseenBasis` contract that drives every badge
 * (per-disc, group-header, top-of-app aggregate, mark-all-read seed).
 */
import { describe, it, expect } from 'vitest';
import { unseenBasis } from '../SwipeableDiscItem';
import type { Discussion, DiscussionMessage } from '../../types/generated';

const skel = {
  id: 'd', project_id: null, title: '', agent: 'ClaudeCode' as const,
  language: 'fr', participants: [], archived: false, pinned: false,
  workspace_mode: 'Direct', created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
} satisfies Partial<Discussion>;

function disc(p: Partial<Discussion>): Discussion {
  return { ...skel, messages: [], message_count: 0, non_system_message_count: 0, ...p } as Discussion;
}

function msg(role: DiscussionMessage['role']): DiscussionMessage {
  return {
    id: Math.random().toString(36).slice(2), role, content: '', agent_type: null,
    timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null,
  } as DiscussionMessage;
}

describe('unseenBasis (0.8.7 badge contract)', () => {
  it('prefers non_system_message_count over message_count', () => {
    // Real workflow run shape: 2 user-facing messages + 50 tool/system rows.
    expect(unseenBasis(disc({ message_count: 52, non_system_message_count: 2 }))).toBe(2);
  });

  it('aggregate across 26 such discussions reads ~52, not ~1352', () => {
    const fleet = Array.from({ length: 26 }, () =>
      disc({ message_count: 52, non_system_message_count: 2 }));
    const sum = fleet.reduce((acc, d) => acc + unseenBasis(d), 0);
    expect(sum).toBe(52);          // 26 × 2 real messages
    // Pre-fix the same fleet would have summed message_count = 1352 — the user-reported bug.
    const buggy = fleet.reduce((acc, d) => acc + d.message_count, 0);
    expect(buggy).toBe(1352);
  });

  it('falls back to filtering messages[] when non_system_message_count is absent', () => {
    // Legacy server / partial rollout: field undefined on the wire.
    const d = { ...skel, messages: [msg('User'), msg('Agent'), msg('System'), msg('System')], message_count: 4 } as unknown as Discussion;
    // @ts-expect-error — simulating server response without the new field
    delete d.non_system_message_count;
    expect(unseenBasis(d)).toBe(2); // User + Agent
  });

  it('last-resort falls back to message_count when both signals are missing', () => {
    const d = { ...skel, messages: [], message_count: 7 } as unknown as Discussion;
    // @ts-expect-error — simulating wire-level absence
    delete d.non_system_message_count;
    expect(unseenBasis(d)).toBe(7);
  });

  it('handles the empty discussion as zero (not undefined / NaN)', () => {
    expect(unseenBasis(disc({}))).toBe(0);
  });
});
