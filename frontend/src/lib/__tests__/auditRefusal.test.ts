// KT-1324 — only the refusal for a process table Kronn could not read can be
// lifted by the human's word; the one naming a surviving process cannot.
import { describe, it, expect } from 'vitest';
import { isUnverifiedAgentsRefusal } from '../auditRefusal';

describe('isUnverifiedAgentsRefusal', () => {
  it('recognises the unreadable-table refusal, whatever wraps it', () => {
    const blind = 'Kronn could not read the process table. Launch again with `accept_unverified_agents: true`.';
    expect(isUnverifiedAgentsRefusal(blind)).toBe(true);
    expect(isUnverifiedAgentsRefusal(`Audit failed: ${blind}`)).toBe(true);
  });

  it('does not recognise a surviving process or any other error', () => {
    expect(isUnverifiedAgentsRefusal('An agent is still running (process 4242). Stop it (`kill -9 4242`).')).toBe(false);
    expect(isUnverifiedAgentsRefusal('agent crashed')).toBe(false);
    expect(isUnverifiedAgentsRefusal('')).toBe(false);
    expect(isUnverifiedAgentsRefusal(null)).toBe(false);
    expect(isUnverifiedAgentsRefusal(undefined)).toBe(false);
  });
});
