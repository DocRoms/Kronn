// KT-1324 — only the refusal for a process table Kronn could not read can be
// lifted by the human's word; the one naming a surviving process cannot. The
// kind is the backend's typed `error_code`, never a reading of the text.
import { describe, it, expect } from 'vitest';
import { isUnverifiedAgentsRefusal } from '../auditRefusal';

describe('isUnverifiedAgentsRefusal', () => {
  it('recognises the unreadable-table refusal by its code', () => {
    expect(isUnverifiedAgentsRefusal('agents_unverified')).toBe(true);
  });

  it('does not recognise a surviving process, another error, or no code at all', () => {
    expect(isUnverifiedAgentsRefusal('agents_still_running')).toBe(false);
    expect(isUnverifiedAgentsRefusal('conflict')).toBe(false);
    expect(isUnverifiedAgentsRefusal('')).toBe(false);
    expect(isUnverifiedAgentsRefusal(null)).toBe(false);
    expect(isUnverifiedAgentsRefusal(undefined)).toBe(false);
  });

  it('does not read the message text: a message that names the flag is not the code', () => {
    expect(isUnverifiedAgentsRefusal('launch again with `accept_unverified_agents: true`')).toBe(false);
  });
});
