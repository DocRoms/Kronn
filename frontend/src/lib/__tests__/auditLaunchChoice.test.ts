import { describe, it, expect } from 'vitest';
import { effectiveLaunchChoice, runLaunchChoice } from '../auditLaunchChoice';

const picker = { agent: 'ClaudeCode' as const, tier: 'reasoning' as const, connectionId: null };
const usable = ['ClaudeCode', 'LiteLlm'] as const;

describe('KT-1320 — a retry or resume keeps the launch of the run it continues', () => {
  it("reads the run's agent, tier and connection back", () => {
    expect(runLaunchChoice({ agent_type: 'LiteLlm', tier: 'economy', connection_id: 'c1' }, usable))
      .toEqual({ agent: 'LiteLlm', tier: 'economy', connectionId: 'c1' });
  });

  it('gives up on a run that kept no tier, an unknown tier, or an agent that cannot run', () => {
    expect(runLaunchChoice({ agent_type: 'ClaudeCode' }, usable)).toBeNull();
    expect(runLaunchChoice({ agent_type: 'ClaudeCode', tier: 'turbo' }, usable)).toBeNull();
    expect(runLaunchChoice({ agent_type: 'repository', tier: 'economy' }, usable)).toBeNull();
    expect(runLaunchChoice({ agent_type: 'Codex', tier: 'economy' }, usable)).toBeNull();
    expect(runLaunchChoice(null, usable)).toBeNull();
  });

  it("runs on the run's tier while the picker is untouched, on the human's once touched", () => {
    const fromRun = { agent: 'ClaudeCode' as const, tier: 'economy' as const, connectionId: null };
    expect(effectiveLaunchChoice(picker, false, fromRun)).toEqual({ choice: fromRun, fromRun: true });
    expect(effectiveLaunchChoice(picker, true, fromRun)).toEqual({ choice: picker, fromRun: false });
    expect(effectiveLaunchChoice(picker, false, null)).toEqual({ choice: picker, fromRun: false });
  });
});
