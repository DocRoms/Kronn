import { describe, it, expect } from 'vitest';
import { isRoomAgentDisabled } from '../constants';

const usable = (agent_type: string) => ({
  agent_type, installed: true, runtime_available: false, enabled: true, auth_ready: true,
});

describe('isRoomAgentDisabled', () => {
  it('never disables a room backed by a custom external connection', () => {
    // Regression: `Custom` has no local binary and is absent from KNOWN_AGENTS,
    // so detection returns nothing for it. Reading that silence as
    // "uninstalled" disabled the composer permanently on an OpenRouter room.
    expect(isRoomAgentDisabled('Custom', [usable('ClaudeCode')], false)).toBe(false);
    expect(isRoomAgentDisabled('Custom', [], false)).toBe(false);
  });

  it('disables a room whose native agent is missing from detection', () => {
    expect(isRoomAgentDisabled('Codex', [usable('ClaudeCode')], false)).toBe(true);
  });

  it('disables a room whose agent is detected but not usable', () => {
    const off = { ...usable('Codex'), enabled: false };
    expect(isRoomAgentDisabled('Codex', [off], false)).toBe(true);
    const unauth = { ...usable('Codex'), auth_ready: false };
    expect(isRoomAgentDisabled('Codex', [unauth], false)).toBe(true);
  });

  it('keeps a usable native agent enabled, including runtime-only availability', () => {
    expect(isRoomAgentDisabled('Codex', [usable('Codex')], false)).toBe(false);
    const runtimeOnly = { ...usable('Codex'), installed: false, runtime_available: true };
    expect(isRoomAgentDisabled('Codex', [runtimeOnly], false)).toBe(false);
  });

  it('never disables a human/CLI-only room, whatever detection says', () => {
    expect(isRoomAgentDisabled('Codex', [usable('ClaudeCode')], true)).toBe(false);
  });

  it('stays silent until detection has loaded', () => {
    expect(isRoomAgentDisabled('Codex', [], false)).toBe(false);
  });
});
