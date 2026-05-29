import { describe, it, expect } from 'vitest';
import { findLastAgentMessage } from '../discussionHelpers';
import type { DiscussionMessage, MessageRole } from '../../types/generated';

let seq = 0;
function msg(role: MessageRole, content = ''): DiscussionMessage {
  seq += 1;
  return {
    id: `m${seq}`,
    role,
    content,
    agent_type: role === 'Agent' ? 'ClaudeCode' : null,
    timestamp: '2026-05-29T00:00:00Z',
    tokens_used: 0,
    auth_mode: null,
  };
}

describe('findLastAgentMessage', () => {
  it('returns null for an empty slice', () => {
    expect(findLastAgentMessage([])).toBeNull();
  });

  it('returns null when there is no Agent message', () => {
    expect(findLastAgentMessage([msg('User'), msg('System'), msg('User')])).toBeNull();
  });

  it('returns the only Agent message', () => {
    const a = msg('Agent', 'hi');
    expect(findLastAgentMessage([msg('User'), a])).toBe(a);
  });

  it('returns the LAST Agent message when several are present', () => {
    const first = msg('Agent', 'first');
    const last = msg('Agent', 'last');
    const found = findLastAgentMessage([first, msg('User'), last, msg('System')]);
    expect(found).toBe(last);
    expect(found?.content).toBe('last');
  });

  it('skips trailing User/System messages to find the prior Agent reply', () => {
    const agent = msg('Agent', 'reply');
    const found = findLastAgentMessage([agent, msg('User'), msg('System')]);
    expect(found).toBe(agent);
  });

  it('does not mutate the input array', () => {
    const arr = [msg('Agent'), msg('User')];
    const snapshot = [...arr];
    findLastAgentMessage(arr);
    expect(arr).toEqual(snapshot);
  });
});
