import { describe, it, expect } from 'vitest';
import {
  effectiveLaunchChoice, runLaunchChoice, usableAuditTargets, type AuditTarget,
} from '../auditLaunchChoice';
import type { AgentDetection } from '../../types/generated';
import type { ExternalApiConnectionView } from '../api';

const picker = { agent: 'ClaudeCode' as const, tier: 'reasoning' as const, connectionId: null };
const targets: AuditTarget[] = [
  { agent: 'ClaudeCode', connectionId: null },
  { agent: 'LiteLlm', connectionId: 'c1' },
  { agent: 'Custom', connectionId: 'named' },
];

const detection = (agent_type: AgentDetection['agent_type']): AgentDetection => ({
  name: agent_type, agent_type, installed: true, enabled: true, path: '/bin/x', version: '1',
  latest_version: null, origin: 'host', install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
});
const connection = (id: string, origin_preset: string, endpoint: string | null = 'https://x') =>
  ({ id, origin_preset, endpoint, display_name: id }) as unknown as ExternalApiConnectionView;

describe('KT-1320 — a retry or resume keeps the launch of the run it continues', () => {
  it("reads the run's agent, tier and connection back", () => {
    expect(runLaunchChoice({ agent_type: 'LiteLlm', tier: 'economy', connection_id: 'c1' }, targets))
      .toEqual({ agent: 'LiteLlm', tier: 'economy', connectionId: 'c1' });
  });

  it('finds a run on a named Custom connection, which is no detected agent', () => {
    expect(runLaunchChoice({ agent_type: 'Custom', tier: 'economy', connection_id: 'named' }, targets))
      .toEqual({ agent: 'Custom', tier: 'economy', connectionId: 'named' });
  });

  it("shows a run whose connection is gone on its agent's default one", () => {
    expect(runLaunchChoice({ agent_type: 'ClaudeCode', tier: 'economy', connection_id: 'gone' }, targets))
      .toEqual({ agent: 'ClaudeCode', tier: 'economy', connectionId: null });
    // A named-only agent with its connection gone has no target left to show.
    expect(runLaunchChoice({ agent_type: 'Custom', tier: 'economy', connection_id: 'gone' }, targets)).toBeNull();
  });

  it('gives up on a run that kept no tier, an unknown tier, or an agent that cannot run', () => {
    expect(runLaunchChoice({ agent_type: 'ClaudeCode' }, targets)).toBeNull();
    expect(runLaunchChoice({ agent_type: 'ClaudeCode', tier: 'turbo' }, targets)).toBeNull();
    expect(runLaunchChoice({ agent_type: 'repository', tier: 'economy' }, targets)).toBeNull();
    expect(runLaunchChoice({ agent_type: 'Codex', tier: 'economy' }, targets)).toBeNull();
    expect(runLaunchChoice(null, targets)).toBeNull();
  });

  it("runs on the run's tier while the picker is untouched, on the human's once touched", () => {
    const fromRun = { agent: 'ClaudeCode' as const, tier: 'economy' as const, connectionId: null };
    expect(effectiveLaunchChoice(picker, false, fromRun)).toEqual({ choice: fromRun, fromRun: true });
    expect(effectiveLaunchChoice(picker, true, fromRun)).toEqual({ choice: picker, fromRun: false });
    expect(effectiveLaunchChoice(picker, false, null)).toEqual({ choice: picker, fromRun: false });
  });
});

describe('usableAuditTargets — the inventory the picker offers', () => {
  it('lists detected agents and the named connections that have an endpoint', () => {
    expect(usableAuditTargets(
      [detection('ClaudeCode')],
      [connection('named', 'other'), connection('no-endpoint', 'other', null)],
    )).toEqual([
      { agent: 'ClaudeCode', connectionId: null },
      { agent: 'Custom', connectionId: 'named' },
    ]);
  });

  it('lets a named connection replace the bare slot of its provider', () => {
    expect(usableAuditTargets(
      [detection('LiteLlm'), detection('Custom')],
      [connection('lite', 'lite_llm')],
    )).toEqual([
      { agent: 'Custom', connectionId: null },
      { agent: 'LiteLlm', connectionId: 'lite' },
    ]);
  });
});
