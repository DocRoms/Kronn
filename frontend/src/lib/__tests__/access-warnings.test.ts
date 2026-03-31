import { describe, it, expect } from 'vitest';
import { t } from '../i18n';

// ─── i18n keys for access warnings ──────────────────────────────────────────

const ACCESS_KEYS = [
  'config.restrictedAgent',
  'config.restrictedAgentLink',
  'config.restrictedDebate',
  'config.restrictedStep',
  'config.fullAccessBadge',
  'disc.agentDisabled',
  'disc.agentDisabledLink',
] as const;

describe('access warning i18n keys', () => {
  for (const locale of ['fr', 'en', 'es'] as const) {
    describe(`locale: ${locale}`, () => {
      for (const key of ACCESS_KEYS) {
        it(`has key "${key}"`, () => {
          const val = t(locale, key);
          // t() returns the raw key if missing — ensure we get a real translation
          expect(val).not.toBe(key);
          expect(val.length).toBeGreaterThan(0);
        });
      }
    });
  }

  it('restrictedAgent supports interpolation in all locales', () => {
    for (const locale of ['fr', 'en', 'es'] as const) {
      const val = t(locale, 'config.restrictedAgent', 'Codex');
      expect(val).toContain('Codex');
    }
  });

  it('fullAccessBadge is "Full access" in all locales', () => {
    for (const locale of ['fr', 'en', 'es'] as const) {
      expect(t(locale, 'config.fullAccessBadge')).toBe('Full access');
    }
  });

  it('agentDisabled supports interpolation in all locales', () => {
    for (const locale of ['fr', 'en', 'es'] as const) {
      const val = t(locale, 'disc.agentDisabled', 'Claude Code');
      expect(val).toContain('Claude Code');
      expect(val).not.toBe('disc.agentDisabled');
    }
  });
});

// ─── checkAgentRestricted logic (extracted from WorkflowsPage) ──────────────

// Replicate the helper logic here to unit-test it independently
import type { AgentsConfig, AgentType } from '../../types/generated';

function checkAgentRestricted(agentAccess: AgentsConfig | undefined, agentType: AgentType): boolean {
  if (!agentAccess) return false;
  const map: Record<string, boolean | undefined> = {
    ClaudeCode: agentAccess.claude_code?.full_access,
    Codex: agentAccess.codex?.full_access,
    GeminiCli: agentAccess.gemini_cli?.full_access,
    Vibe: agentAccess.vibe?.full_access,
  };
  return map[agentType] === false;
}

function hasFullAccess(agentAccess: AgentsConfig | undefined, agentType: AgentType): boolean {
  if (!agentAccess) return false;
  const map: Record<string, boolean | undefined> = {
    ClaudeCode: agentAccess.claude_code?.full_access,
    Codex: agentAccess.codex?.full_access,
    GeminiCli: agentAccess.gemini_cli?.full_access,
    Vibe: agentAccess.vibe?.full_access,
  };
  return map[agentType] === true;
}

const defaultModelTiers = {
  claude_code: { economy: null, reasoning: null },
  codex: { economy: null, reasoning: null },
  gemini_cli: { economy: null, reasoning: null },
  kiro: { economy: null, reasoning: null },
  vibe: { economy: null, reasoning: null },
  copilot_cli: { economy: null, reasoning: null },
};

const makeConfig = (overrides: Partial<Record<'claude' | 'codex' | 'gemini' | 'kiro' | 'vibe' | 'copilot', boolean>>): AgentsConfig => ({
  claude_code: { path: null, installed: true, version: null, full_access: overrides.claude ?? false },
  codex: { path: null, installed: true, version: null, full_access: overrides.codex ?? false },
  gemini_cli: { path: null, installed: true, version: null, full_access: overrides.gemini ?? false },
  kiro: { path: null, installed: false, version: null, full_access: overrides.kiro ?? false },
  vibe: { path: null, installed: false, version: null, full_access: overrides.vibe ?? false },
  copilot_cli: { path: null, installed: false, version: null, full_access: overrides.copilot ?? false },
  model_tiers: defaultModelTiers,
});

describe('checkAgentRestricted', () => {
  it('returns false when agentAccess is undefined', () => {
    expect(checkAgentRestricted(undefined, 'ClaudeCode')).toBe(false);
    expect(checkAgentRestricted(undefined, 'Codex')).toBe(false);
  });

  it('returns true when agent has full_access=false', () => {
    const config = makeConfig({ claude: false, codex: false, gemini: false });
    expect(checkAgentRestricted(config, 'ClaudeCode')).toBe(true);
    expect(checkAgentRestricted(config, 'Codex')).toBe(true);
    expect(checkAgentRestricted(config, 'GeminiCli')).toBe(true);
  });

  it('returns false when agent has full_access=true', () => {
    const config = makeConfig({ claude: true, codex: true, gemini: true });
    expect(checkAgentRestricted(config, 'ClaudeCode')).toBe(false);
    expect(checkAgentRestricted(config, 'Codex')).toBe(false);
    expect(checkAgentRestricted(config, 'GeminiCli')).toBe(false);
  });

  it('returns true for Vibe when full_access is false', () => {
    const config = makeConfig({ claude: false });
    expect(checkAgentRestricted(config, 'Vibe')).toBe(true);
  });

  it('returns false for Vibe when full_access is true', () => {
    const config = makeConfig({ vibe: true });
    expect(checkAgentRestricted(config, 'Vibe')).toBe(false);
  });

  it('handles mixed access states', () => {
    const config = makeConfig({ claude: true, codex: false, gemini: true });
    expect(checkAgentRestricted(config, 'ClaudeCode')).toBe(false);
    expect(checkAgentRestricted(config, 'Codex')).toBe(true);
    expect(checkAgentRestricted(config, 'GeminiCli')).toBe(false);
  });
});

describe('hasFullAccess', () => {
  it('returns false when agentAccess is undefined', () => {
    expect(hasFullAccess(undefined, 'ClaudeCode')).toBe(false);
  });

  it('returns true when agent has full_access=true', () => {
    const config = makeConfig({ claude: true, codex: true, gemini: true });
    expect(hasFullAccess(config, 'ClaudeCode')).toBe(true);
    expect(hasFullAccess(config, 'Codex')).toBe(true);
    expect(hasFullAccess(config, 'GeminiCli')).toBe(true);
  });

  it('returns false when agent has full_access=false', () => {
    const config = makeConfig({ claude: false, codex: false });
    expect(hasFullAccess(config, 'ClaudeCode')).toBe(false);
    expect(hasFullAccess(config, 'Codex')).toBe(false);
  });

  it('returns false for Vibe (not in map)', () => {
    const config = makeConfig({ claude: true });
    expect(hasFullAccess(config, 'Vibe')).toBe(false);
  });

  it('isRestricted and hasFullAccess are mutually exclusive for known agents', () => {
    const config = makeConfig({ claude: true, codex: false, gemini: true });
    for (const agent of ['ClaudeCode', 'Codex', 'GeminiCli'] as AgentType[]) {
      const restricted = checkAgentRestricted(config, agent);
      const full = hasFullAccess(config, agent);
      expect(restricted && full).toBe(false);
      // For known agents, exactly one must be true
      expect(restricted || full).toBe(true);
    }
  });
});

// ─── isAgentDisabled logic (Dashboard: active discussion with disabled agent) ─

import type { AgentDetection } from '../../types/generated';
import { isUsable } from '../constants';

function isAgentDisabled(agentType: AgentType, agents: AgentDetection[]): boolean {
  if (agents.length === 0) return false;
  const det = agents.find(a => a.agent_type === agentType);
  return !det || !isUsable(det);
}

const makeAgent = (type: AgentType, overrides: Partial<AgentDetection> = {}): AgentDetection => ({
  agent_type: type,
  name: type,
  installed: true,
  runtime_available: false,
  enabled: true,
  path: null,
  version: null,
  latest_version: null,
  origin: 'local',
  install_command: null,
  host_managed: false,
  host_label: null,
  ...overrides,
});

describe('isAgentDisabled (conversation input guard)', () => {
  it('returns false when agents list is empty (not yet loaded)', () => {
    expect(isAgentDisabled('ClaudeCode', [])).toBe(false);
  });

  it('returns false for installed + enabled agent', () => {
    const agents = [makeAgent('ClaudeCode')];
    expect(isAgentDisabled('ClaudeCode', agents)).toBe(false);
  });

  it('returns true for uninstalled agent', () => {
    const agents = [makeAgent('ClaudeCode', { installed: false, runtime_available: false })];
    expect(isAgentDisabled('ClaudeCode', agents)).toBe(true);
  });

  it('returns true for disabled agent', () => {
    const agents = [makeAgent('ClaudeCode', { enabled: false })];
    expect(isAgentDisabled('ClaudeCode', agents)).toBe(true);
  });

  it('returns false for runtime-available agent (not locally installed)', () => {
    const agents = [makeAgent('Codex', { installed: false, runtime_available: true })];
    expect(isAgentDisabled('Codex', agents)).toBe(false);
  });

  it('returns true for agent not in list at all', () => {
    const agents = [makeAgent('ClaudeCode')];
    expect(isAgentDisabled('GeminiCli', agents)).toBe(true);
  });

  it('returns true for disabled + runtime available', () => {
    const agents = [makeAgent('Codex', { installed: false, runtime_available: true, enabled: false })];
    expect(isAgentDisabled('Codex', agents)).toBe(true);
  });
});
