// 0.8.6 phase 4 — Pure tests for the discussion grouping fold.
//
// Pins the algorithm so a future refactor of DiscussionsPage can be
// validated against these expectations. Closes audit gap #5 (2026-05-22).

import { describe, it, expect } from 'vitest';
import { groupMessagesWithToolFold } from '../discussionMessageGrouping';
import type { DiscussionMessage } from '../../types/generated';

function mkMsg(
  role: 'User' | 'Agent' | 'System',
  content: string,
  idSuffix: string,
): DiscussionMessage {
  return {
    id: `msg-${idSuffix}`,
    role,
    content,
    agent_type: null,
    timestamp: `2026-05-22T15:00:${idSuffix.padStart(2, '0')}Z`,
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    author_pseudo: null,
    author_avatar_email: null,
  };
}

describe('groupMessagesWithToolFold', () => {
  it('empty list returns empty', () => {
    expect(groupMessagesWithToolFold([])).toEqual([]);
  });

  it('list with only non-tool messages passes through 1:1', () => {
    const msgs = [
      mkMsg('User', 'Hello', '01'),
      mkMsg('Agent', 'Hi there', '02'),
    ];
    const items = groupMessagesWithToolFold(msgs);
    expect(items).toHaveLength(2);
    expect(items[0]).toEqual({ kind: 'message', msg: msgs[0], idx: 0 });
    expect(items[1]).toEqual({ kind: 'message', msg: msgs[1], idx: 1 });
  });

  it('single tool group at the start is followed by tail flush', () => {
    const msgs = [
      mkMsg('System', '[kronn-internal: qa_list()]', '01'),
      mkMsg('System', '[kronn-internal: api_call({})]', '02'),
    ];
    const items = groupMessagesWithToolFold(msgs);
    expect(items).toHaveLength(1);
    expect(items[0].kind).toBe('tool-group');
    expect((items[0] as { kind: 'tool-group'; messages: DiscussionMessage[] }).messages).toHaveLength(2);
  });

  it('tool group renders BEFORE the next non-tool message (banner-above-reply position)', () => {
    // The whole point of the grouping : the user expects the banner
    // to summarise the tool calls the agent made BEFORE writing its
    // reply, displayed just ABOVE that reply.
    const msgs = [
      mkMsg('User', 'Audit this', '01'),
      mkMsg('System', '[kronn-internal: qa_run({})]', '02'),
      mkMsg('System', '[kronn-internal: api_call({})]', '03'),
      mkMsg('Agent', 'Done', '04'),
    ];
    const items = groupMessagesWithToolFold(msgs);
    expect(items.map(i => i.kind)).toEqual(['message', 'tool-group', 'message']);
    // The tool-group at position [1] sits just before the Agent
    // message at position [2]. Critical ordering.
    expect((items[0] as { msg: DiscussionMessage }).msg.role).toBe('User');
    expect((items[2] as { msg: DiscussionMessage }).msg.role).toBe('Agent');
  });

  it('multi-turn : each turn gets its own tool group', () => {
    // Real-world : a long disc with 2 user prompts, each producing
    // tool calls + an agent reply. The fold must emit TWO groups,
    // each between its triggering user/agent and the next non-tool
    // message.
    const msgs = [
      mkMsg('User', 'Q1', '01'),
      mkMsg('System', '[kronn-internal: qa_run({})]', '02'),
      mkMsg('Agent', 'A1', '03'),
      mkMsg('User', 'Q2', '04'),
      mkMsg('System', '[agent-native: Read({})]', '05'),
      mkMsg('System', '[agent-native: Bash({})]', '06'),
      mkMsg('Agent', 'A2', '07'),
    ];
    const items = groupMessagesWithToolFold(msgs);
    expect(items.map(i => i.kind)).toEqual([
      'message',     // User Q1
      'tool-group',  // 1 kronn-internal call
      'message',     // Agent A1
      'message',     // User Q2
      'tool-group',  // 2 agent-native calls
      'message',     // Agent A2
    ]);
    // Each tool group contains only the calls from its turn.
    const firstGroup = items[1] as { kind: 'tool-group'; messages: DiscussionMessage[] };
    expect(firstGroup.messages).toHaveLength(1);
    const secondGroup = items[4] as { kind: 'tool-group'; messages: DiscussionMessage[] };
    expect(secondGroup.messages).toHaveLength(2);
  });

  it('tail flush : tool calls at the very end produce a final group', () => {
    // Edge case : the disc ends mid-stream (cancelled, crashed) and
    // the last messages are tool calls with no following non-tool.
    // Without the tail flush, those calls would silently disappear.
    const msgs = [
      mkMsg('User', 'Do x', '01'),
      mkMsg('Agent', 'Working', '02'),
      mkMsg('System', '[kronn-internal: qa_run({})]', '03'),
      mkMsg('System', '[kronn-internal: api_call({})]', '04'),
    ];
    const items = groupMessagesWithToolFold(msgs);
    expect(items.map(i => i.kind)).toEqual(['message', 'message', 'tool-group']);
    expect((items[2] as { messages: DiscussionMessage[] }).messages).toHaveLength(2);
  });

  it('mixed kronn-internal AND agent-native messages stay in ONE group (no split)', () => {
    // The grouping is by ORIGIN-IN-TIME, not by source. Sub-banner
    // splitting (kronn vs native) happens inside ToolCallsGroup.tsx ;
    // the grouping fold only cares about contiguity.
    const msgs = [
      mkMsg('User', 'Q', '01'),
      mkMsg('System', '[kronn-internal: qa_run({})]', '02'),
      mkMsg('System', '[agent-native: Read({})]', '03'),
      mkMsg('System', '[kronn-internal: api_call({})]', '04'),
      mkMsg('Agent', 'A', '05'),
    ];
    const items = groupMessagesWithToolFold(msgs);
    expect(items.map(i => i.kind)).toEqual(['message', 'tool-group', 'message']);
    expect((items[1] as { messages: DiscussionMessage[] }).messages).toHaveLength(3);
  });

  it('non-tool System messages (e.g. summary cached) do NOT join the buffer', () => {
    // System messages with a content not matching the tool regex
    // (summary cache notices, error blocks) keep their own bubble.
    const msgs = [
      mkMsg('User', 'Q', '01'),
      mkMsg('System', '[kronn-internal: qa_run({})]', '02'),
      mkMsg('System', 'summary cached at 14:32', '03'),  // NOT a tool call
      mkMsg('System', '[kronn-internal: api_call({})]', '04'),
      mkMsg('Agent', 'A', '05'),
    ];
    const items = groupMessagesWithToolFold(msgs);
    // The summary message FLUSHES the first tool group, renders itself
    // as a regular message, then opens a NEW buffer for the next tool
    // call. Result : 2 separate tool groups.
    expect(items.map(i => i.kind)).toEqual([
      'message',     // User Q
      'tool-group',  // qa_run (1 call)
      'message',     // System "summary cached"
      'tool-group',  // api_call (1 call)
      'message',     // Agent A
    ]);
  });

  it('respects isAutoPrompt callback : hides matching indices entirely', () => {
    // Briefing / Validation / Bootstrap discs auto-generate their
    // first User message — the caller hides it via this hook. The
    // hidden message must NOT appear in render items AND must NOT
    // flush any pending tool buffer.
    const msgs = [
      mkMsg('User', 'auto-generated briefing prompt', '01'),
      mkMsg('System', '[kronn-internal: qa_list()]', '02'),
      mkMsg('Agent', 'My briefing', '03'),
    ];
    const items = groupMessagesWithToolFold(msgs, {
      isAutoPrompt: (idx) => idx === 0,
    });
    // Auto-prompt user msg dropped. Only the tool group + agent remain.
    expect(items.map(i => i.kind)).toEqual(['tool-group', 'message']);
  });

  it('preserves the original message idx in the render item', () => {
    // The renderer uses idx for `isLastUser` / `isLastAgent` / prev-ts
    // computations. The fold must NOT renumber — original positions
    // stay authoritative.
    const msgs = [
      mkMsg('User', 'Q1', '01'),
      mkMsg('System', '[kronn-internal: qa_list()]', '02'),
      mkMsg('Agent', 'A1', '03'),
    ];
    const items = groupMessagesWithToolFold(msgs);
    expect((items[0] as { idx: number }).idx).toBe(0);
    expect((items[2] as { idx: number }).idx).toBe(2);
  });

  it('handles 20+ consecutive tool calls without producing 20 groups', () => {
    // QP run that fires plein de calls (the original user complaint
    // from 2026-05-22). All consecutive calls must land in ONE group,
    // not become a stream of singletons.
    const msgs: DiscussionMessage[] = [];
    msgs.push(mkMsg('User', 'Audit + report', '01'));
    for (let i = 0; i < 20; i++) {
      msgs.push(mkMsg('System', `[kronn-internal: qa_run({"i":${i}})]`, String(i + 2).padStart(2, '0')));
    }
    msgs.push(mkMsg('Agent', 'Done', '99'));
    const items = groupMessagesWithToolFold(msgs);
    // Exactly 3 render items : User, ONE big tool-group, Agent.
    expect(items.map(i => i.kind)).toEqual(['message', 'tool-group', 'message']);
    expect((items[1] as { messages: DiscussionMessage[] }).messages).toHaveLength(20);
  });
});
