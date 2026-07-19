// Audit vs briefing capability split (Codex A4 v2).
//
// The audit pipeline writes docs/ (filesystem required) — positive
// allowlist mirroring the backend. The briefing is a pure conversation —
// Ollama stays eligible there. An Ollama-only install must therefore show
// NO audit agents but still allow the briefing.

import { describe, it, expect } from 'vitest';
import { canRunAudit, canRunBriefing } from '../../lib/agentCapabilities';
import type { AgentDetection } from '../../types/generated';

const agent = (agent_type: AgentDetection['agent_type']): AgentDetection => ({
  name: agent_type, agent_type, installed: true, enabled: true,
  path: '/bin/x', version: '1', latest_version: null, origin: 'host',
  install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
});

describe('audit/briefing capability predicates', () => {
  it('audit is a positive allowlist — Ollama, Vibe and Custom are out', () => {
    for (const t of ['ClaudeCode', 'Codex', 'GeminiCli', 'Kiro', 'CopilotCli'] as const) {
      expect(canRunAudit(agent(t)), t).toBe(true);
    }
    for (const t of ['Ollama', 'Vibe', 'Custom'] as const) {
      expect(canRunAudit(agent(t)), t).toBe(false);
    }
  });

  it('briefing keeps Ollama (conversation-only), excludes only Vibe', () => {
    expect(canRunBriefing(agent('Ollama'))).toBe(true);
    expect(canRunBriefing(agent('Vibe'))).toBe(false);
    expect(canRunBriefing(agent('ClaudeCode'))).toBe(true);
  });

  it('an Ollama-only install: zero audit agents, briefing still possible', () => {
    const fleet = [agent('Ollama')];
    expect(fleet.filter(canRunAudit)).toHaveLength(0);
    expect(fleet.filter(canRunBriefing)).toHaveLength(1);
  });

  it('a disabled agent is out of both', () => {
    const off = { ...agent('ClaudeCode'), enabled: false };
    expect(canRunAudit(off)).toBe(false);
    expect(canRunBriefing(off)).toBe(false);
  });
});
