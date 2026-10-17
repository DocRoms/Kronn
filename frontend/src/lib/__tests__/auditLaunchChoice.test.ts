import { describe, it, expect } from 'vitest';
import { effectiveLaunchChoice, runLaunchChoice, type ConnectionInventory } from '../auditLaunchChoice';

const picker = { agent: 'ClaudeCode' as const, tier: 'reasoning' as const, connectionId: null };
const loaded = (...ids: string[]): ConnectionInventory => ({ status: 'ready', list: ids.map(id => ({ id })) });
const loading: ConnectionInventory = { status: 'loading', list: [] };
const failed: ConnectionInventory = { status: 'error', list: [] };

describe('KT-1320 — a retry or resume keeps the launch of the run it continues', () => {
  it("reads the run's agent, tier and connection back", () => {
    expect(runLaunchChoice({ agent_type: 'LiteLlm', tier: 'economy', connection_id: 'c1' }, loaded('c1')))
      .toEqual({ agent: 'LiteLlm', tier: 'economy', connectionId: 'c1', connectionGone: false });
    expect(runLaunchChoice({ agent_type: 'Codex', tier: 'default' }, loaded()))
      .toEqual({ agent: 'Codex', tier: 'default', connectionId: null, connectionGone: false });
  });

  it('keeps a run on a named Custom connection, which is no detected agent', () => {
    expect(runLaunchChoice({ agent_type: 'Custom', tier: 'economy', connection_id: 'named' }, loaded('named', 'other')))
      .toEqual({ agent: 'Custom', tier: 'economy', connectionId: 'named', connectionGone: false });
  });

  it("keeps the agent and the tier when the connection was deleted, and says so", () => {
    for (const agent_type of ['ClaudeCode', 'LiteLlm', 'Custom']) {
      expect(runLaunchChoice({ agent_type, tier: 'economy', connection_id: 'gone' }, loaded('another')))
        .toEqual({ agent: agent_type, tier: 'economy', connectionId: null, connectionGone: true });
      expect(runLaunchChoice({ agent_type, tier: 'economy', connection_id: 'gone' }, loaded()))
        .toEqual({ agent: agent_type, tier: 'economy', connectionId: null, connectionGone: true });
    }
  });

  it('never calls the connection deleted while the list loads or failed to load', () => {
    for (const inventory of [loading, failed]) {
      expect(runLaunchChoice({ agent_type: 'Custom', tier: 'economy', connection_id: 'named' }, inventory))
        .toEqual({ agent: 'Custom', tier: 'economy', connectionId: 'named', connectionGone: false });
    }
  });

  it('gives up only on a run with nothing to inherit: no tier, an unknown tier, or not an agent', () => {
    expect(runLaunchChoice({ agent_type: 'ClaudeCode' }, loaded())).toBeNull();
    expect(runLaunchChoice({ agent_type: 'ClaudeCode', tier: 'turbo' }, loaded())).toBeNull();
    expect(runLaunchChoice({ agent_type: 'repository', tier: 'economy' }, loaded())).toBeNull();
    expect(runLaunchChoice(null, loaded())).toBeNull();
  });

  it("runs on the run's choice while the picker is untouched, on the human's once touched", () => {
    const fromRun = { agent: 'ClaudeCode' as const, tier: 'economy' as const, connectionId: null, connectionGone: false };
    const { connectionGone: _gone, ...choice } = fromRun;
    expect(effectiveLaunchChoice(picker, false, fromRun)).toEqual({ choice, fromRun: true, connectionGone: false });
    expect(effectiveLaunchChoice(picker, true, fromRun)).toEqual({ choice: picker, fromRun: false, connectionGone: false });
    expect(effectiveLaunchChoice(picker, false, null)).toEqual({ choice: picker, fromRun: false, connectionGone: false });
  });

  it('carries the deleted-connection fact to the panel', () => {
    const fromRun = { agent: 'Custom' as const, tier: 'economy' as const, connectionId: null, connectionGone: true };
    expect(effectiveLaunchChoice(picker, false, fromRun).connectionGone).toBe(true);
    // Once the human picks, the note is moot.
    expect(effectiveLaunchChoice(picker, true, fromRun).connectionGone).toBe(false);
  });
});
