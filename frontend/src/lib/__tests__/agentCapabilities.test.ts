import { describe, it, expect } from 'vitest';
import { canRunAudit, canRunBriefing } from '../agentCapabilities';
import type { AgentDetection } from '../../types/generated';

function agent(overrides: Partial<AgentDetection>): AgentDetection {
  return {
    name: 'Test Agent',
    agent_type: 'OpenCode',
    installed: true,
    enabled: true,
    path: null,
    version: null,
    latest_version: null,
    origin: 'OpenCode',
    install_command: '',
    host_managed: false,
    host_label: null,
    runtime_available: false,
    auth_ready: true,
    rtk_available: false,
    rtk_hook_configured: false,
    ...overrides,
  };
}

describe('agentCapabilities', () => {
  describe('canRunAudit()', () => {
    it('allows OpenCode, mirroring the backend agent_can_audit allowlist (KT-543)', () => {
      expect(canRunAudit(agent({ agent_type: 'OpenCode' }))).toBe(true);
    });

    it('rejects an agent absent from the allowlist even when usable', () => {
      expect(canRunAudit(agent({ agent_type: 'Vibe' }))).toBe(false);
    });

    it('rejects OpenCode when it is not usable (disabled or auth not ready)', () => {
      expect(canRunAudit(agent({ agent_type: 'OpenCode', enabled: false }))).toBe(false);
      expect(canRunAudit(agent({ agent_type: 'OpenCode', auth_ready: false }))).toBe(false);
    });
  });

  describe('canRunBriefing()', () => {
    it('allows OpenCode', () => {
      expect(canRunBriefing(agent({ agent_type: 'OpenCode' }))).toBe(true);
    });

    it('excludes API-only Vibe', () => {
      expect(canRunBriefing(agent({ agent_type: 'Vibe' }))).toBe(false);
    });
  });
});
