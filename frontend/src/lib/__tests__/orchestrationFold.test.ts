// KT-1330 — the orchestration fold: which messages collapse, how runs are
// cut, and the rule that nothing needing the user's action is ever hidden.

import { describe, it, expect } from 'vitest';
import { groupMessagesWithToolFold } from '../discussionMessageGrouping';
import {
  foldOrchestrationRuns,
  isOrchestrationMessage,
  requiresUserAction,
  type FoldedRenderItem,
} from '../orchestrationFold';
import type { DiscussionMessage } from '../../types/generated';

let tick = 0;
function mk(id: string, over: Partial<DiscussionMessage> = {}): DiscussionMessage {
  tick += 1;
  return {
    id,
    role: 'User',
    channel: 'main',
    content: `content of ${id}`,
    agent_type: null,
    timestamp: `2026-10-17T10:00:${String(tick % 60).padStart(2, '0')}Z`,
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    author_pseudo: null,
    author_avatar_email: null,
    ...over,
  } as DiscussionMessage;
}

const orch = (id: string, over: Partial<DiscussionMessage> = {}) =>
  mk(id, { kind: 'orchestration', author_pseudo: 'Orchestrateur', ...over });

const fold = (
  messages: DiscussionMessage[],
  opts: Partial<Parameters<typeof foldOrchestrationRuns>[1]> = {},
) =>
  foldOrchestrationRuns(groupMessagesWithToolFold(messages), {
    hide: true,
    expandedRunIds: new Set(),
    ...opts,
  });

const shape = (items: FoldedRenderItem[]) =>
  items.map(item => {
    if (item.kind === 'orchestration-marker') return `fold(${item.messages.length}${item.expanded ? ',open' : ''})`;
    if (item.kind === 'tool-group') return 'tools';
    return item.msg.id;
  });

describe('isOrchestrationMessage', () => {
  it('trusts the typed kind and nothing else', () => {
    expect(isOrchestrationMessage(orch('a'))).toBe(true);
    expect(isOrchestrationMessage(mk('b'))).toBe(false);
    // Not text matching: the id prefix and the pseudonym alone prove nothing
    // on the client — the backend resolves legacy rows into `kind`.
    expect(isOrchestrationMessage(mk('orch-review-request:x:0', { author_pseudo: 'Orchestrateur' }))).toBe(false);
  });
});

describe('foldOrchestrationRuns', () => {
  it('leaves everything untouched while the toggle is off', () => {
    const msgs = [mk('h1'), orch('o1'), orch('o2'), mk('h2')];
    expect(shape(fold(msgs, { hide: false }))).toEqual(['h1', 'o1', 'o2', 'h2']);
  });

  it('collapses one run of consecutive orchestration messages into one marker', () => {
    const msgs = [mk('h1'), orch('o1'), orch('o2'), orch('o3'), mk('h2')];
    const items = fold(msgs);
    expect(shape(items)).toEqual(['h1', 'fold(3)', 'h2']);
    const marker = items[1];
    expect(marker.kind === 'orchestration-marker' && marker.runId).toBe('o1');
  });

  it('cuts a run at every non-orchestration message', () => {
    const msgs = [orch('o1'), orch('o2'), mk('h1'), orch('o3'), mk('a1', { role: 'Agent' }), orch('o4'), orch('o5')];
    expect(shape(fold(msgs))).toEqual(['fold(2)', 'h1', 'fold(1)', 'a1', 'fold(2)']);
  });

  it('never folds a note into a run, even an orchestration-kind one', () => {
    const msgs = [orch('o1'), orch('n1', { channel: 'note' }), orch('o2')];
    expect(shape(fold(msgs))).toEqual(['fold(1)', 'n1', 'fold(1)']);
  });

  it('opens a run on demand and keeps the marker so it can be closed again', () => {
    const msgs = [orch('o1'), orch('o2'), mk('h1')];
    expect(shape(fold(msgs, { expandedRunIds: new Set(['o1']) }))).toEqual(['fold(2,open)', 'o1', 'o2', 'h1']);
  });

  it('keeps an expanded run open when more orchestration lands after it', () => {
    const msgs = [orch('o1'), orch('o2'), orch('o3')];
    expect(shape(fold(msgs, { expandedRunIds: new Set(['o1']) }))).toEqual(['fold(3,open)', 'o1', 'o2', 'o3']);
  });

  it('does not let a run span two calendar days', () => {
    const msgs = [
      orch('o1', { timestamp: '2026-10-16T12:00:00Z' }),
      orch('o2', { timestamp: '2026-10-18T12:00:00Z' }),
    ];
    expect(shape(fold(msgs))).toEqual(['fold(1)', 'fold(1)']);
  });

  it('breaks a run at a tool-call group', () => {
    const tool = mk('t1', { role: 'System', content: '[kronn-internal: qa_run] ok' });
    const items = fold([orch('o1'), tool, orch('o2')]);
    expect(shape(items)).toEqual(['fold(1)', 'tools', 'fold(1)']);
  });

  describe('never hides what needs the user', () => {
    it('keeps an orchestration message carrying a kronn-question card', () => {
      const card = orch('o2', {
        content: 'Pick one\n\n```kronn-question\n{"version":1,"key":"k","question":"Q?","options":[]}\n```',
      });
      expect(shape(fold([orch('o1'), card, orch('o3')]))).toEqual(['fold(1)', 'o2', 'fold(1)']);
    });

    it('keeps a campaign gate parked on a human, folds its release', () => {
      const gate = orch('orch-campaign-gate:run-1:exec-1:awaiting:0');
      const released = orch('orch-campaign-gate:run-1:exec-1:resumed:0');
      expect(shape(fold([gate, released]))).toEqual([gate.id, 'fold(1)']);
    });

    it('keeps a message that still has an action attached', () => {
      const msgs = [orch('o1'), orch('o2'), orch('o3')];
      const items = fold(msgs, { hasActions: id => id === 'o2' });
      expect(shape(items)).toEqual(['fold(1)', 'o2', 'fold(1)']);
    });

    it('keeps a message a search hit or deep link points at', () => {
      const items = fold([orch('o1'), orch('o2')], { forceVisibleIds: new Set(['o2']) });
      expect(shape(items)).toEqual(['fold(1)', 'o2']);
    });
  });
});

describe('requiresUserAction', () => {
  it('is false for plain orchestration chatter', () => {
    expect(requiresUserAction(orch('orch-review-request:e:0'))).toBe(false);
    expect(requiresUserAction(orch('delivery-summary:e:0', { content: 'Delivery accepted' }))).toBe(false);
  });

  it('recognises a kronn-question fence and a human gate', () => {
    expect(requiresUserAction(mk('x', { content: '```kronn-question\n{}\n```' }))).toBe(true);
    expect(requiresUserAction(mk('orch-campaign-gate:r:e:awaiting:2'))).toBe(true);
  });
});
