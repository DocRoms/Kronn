// Audit vs briefing capability split (Codex A4 v2).
//
// The audit pipeline writes docs/ (it must reach the project's files) —
// positive allowlist mirroring the backend. Since KT-924 that includes Ollama
// and LiteLLM, whose native file tools Kronn runs scoped to the project. The
// briefing is a pure conversation, so it stays open to every agent but Vibe.

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
  it('audit is a positive allowlist — every HTTP provider is in (KT-980), Vibe is out', () => {
    for (const t of ['ClaudeCode', 'Codex', 'GeminiCli', 'Kiro', 'CopilotCli', 'Ollama', 'LiteLlm', 'Nvidia', 'Custom'] as const) {
      expect(canRunAudit(agent(t)), t).toBe(true);
    }
    expect(canRunAudit(agent('Vibe'))).toBe(false);
  });

  it('briefing keeps Ollama (conversation-only), excludes only Vibe', () => {
    expect(canRunBriefing(agent('Ollama'))).toBe(true);
    expect(canRunBriefing(agent('Vibe'))).toBe(false);
    expect(canRunBriefing(agent('ClaudeCode'))).toBe(true);
  });

  it('an Ollama-only install can audit and brief; a Vibe-only one can do neither', () => {
    const ollama = [agent('Ollama')];
    expect(ollama.filter(canRunAudit)).toHaveLength(1);
    expect(ollama.filter(canRunBriefing)).toHaveLength(1);
    const vibe = [agent('Vibe')];
    expect(vibe.filter(canRunAudit)).toHaveLength(0);
    expect(vibe.filter(canRunBriefing)).toHaveLength(0);
  });

  it('a disabled agent is out of both', () => {
    const off = { ...agent('ClaudeCode'), enabled: false };
    expect(canRunAudit(off)).toBe(false);
    expect(canRunBriefing(off)).toBe(false);
  });
});
