import { describe, expect, it } from 'vitest';
import { supersededActivationCards } from '../activationCard';

const card = (id: string, quickExecId: string, role = 'System') => ({
  id,
  role,
  content: `[kronn-activate: ${JSON.stringify({ quick_exec_id: quickExecId, name: 'Top 5xx', program: 'aws' })}]`,
});

describe('supersededActivationCards (KT-1695)', () => {
  it('keeps the latest card of a Quick Exec and drops the earlier ones', () => {
    const messages = [card('m1', 'qe-1'), card('m2', 'qe-2'), card('m3', 'qe-1'), card('m4', 'qe-1')];
    expect([...supersededActivationCards(messages)].sort()).toEqual(['m1', 'm3']);
  });

  it('never touches another message', () => {
    const messages = [
      { id: 'u1', role: 'User', content: 'hello' },
      card('m1', 'qe-1'),
      card('forged', 'qe-1', 'User'),
      { id: 'a1', role: 'System', content: '[kronn-planning: {}]' },
    ];
    expect(supersededActivationCards(messages).size).toBe(0);
  });
});
