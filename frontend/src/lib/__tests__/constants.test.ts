import { describe, it, expect } from 'vitest';
import { AGENT_COLORS, AGENT_LABELS, ALL_AGENT_TYPES, agentColor, getProjectGroup } from '../constants';

describe('constants', () => {
  describe('AGENT_COLORS', () => {
    it('has colors for all agent types', () => {
      for (const at of ALL_AGENT_TYPES) {
        expect(AGENT_COLORS[at]).toBeDefined();
        expect(AGENT_COLORS[at]).toMatch(/^#/);
      }
    });

    it('has display-name aliases for Claude and Gemini', () => {
      expect(AGENT_COLORS['Claude Code']).toBe(AGENT_COLORS['ClaudeCode']);
      expect(AGENT_COLORS['Gemini CLI']).toBe(AGENT_COLORS['GeminiCli']);
    });
  });

  describe('AGENT_LABELS', () => {
    it('has labels for all agent types', () => {
      for (const at of ALL_AGENT_TYPES) {
        expect(AGENT_LABELS[at]).toBeDefined();
        expect(typeof AGENT_LABELS[at]).toBe('string');
      }
    });

    it('maps type names to display names', () => {
      expect(AGENT_LABELS['ClaudeCode']).toBe('Claude Code');
      expect(AGENT_LABELS['GeminiCli']).toBe('Gemini CLI');
      expect(AGENT_LABELS['Codex']).toBe('Codex');
      expect(AGENT_LABELS['Vibe']).toBe('Vibe');
    });
  });

  describe('ALL_AGENT_TYPES', () => {
    it('contains the 5 real agent types — Custom is intentionally excluded', () => {
      // ALL_AGENT_TYPES lists only the concrete, installable agent types.
      // AgentType (from generated.ts) also includes "Custom" (a 6th variant)
      // which is a generic escape-hatch type, not a selectable agent in the UI.
      // Therefore ALL_AGENT_TYPES has 5 entries and Custom is excluded on purpose.
      expect(ALL_AGENT_TYPES).toHaveLength(5);
      expect(ALL_AGENT_TYPES).toContain('ClaudeCode');
      expect(ALL_AGENT_TYPES).toContain('Codex');
      expect(ALL_AGENT_TYPES).toContain('Vibe');
      expect(ALL_AGENT_TYPES).toContain('GeminiCli');
      expect(ALL_AGENT_TYPES).toContain('Kiro');
      expect(ALL_AGENT_TYPES).not.toContain('Custom');
    });
  });

  describe('agentColor()', () => {
    it('returns correct color for known agent', () => {
      expect(agentColor('ClaudeCode')).toBe('#D4714E');
      expect(agentColor('Codex')).toBe('#10a37f');
      expect(agentColor('GeminiCli')).toBe('#4285f4');
    });

    it('returns fallback purple for unknown agent', () => {
      expect(agentColor('UnknownAgent')).toBe('#8b5cf6');
    });

    it('returns fallback for null/undefined', () => {
      expect(agentColor(null)).toBe('#8b5cf6');
      expect(agentColor(undefined)).toBe('#8b5cf6');
    });

    it('handles display name keys', () => {
      expect(agentColor('Claude Code')).toBe('#D4714E');
      expect(agentColor('Gemini CLI')).toBe('#4285f4');
    });
  });

  describe('getProjectGroup()', () => {
    it('extracts GitHub org from SSH URL', () => {
      expect(getProjectGroup({ repo_url: 'git@github.com:Euronews-tech/front_euronews.git' }))
        .toBe('Euronews-tech');
    });

    it('extracts GitHub org from HTTPS URL', () => {
      expect(getProjectGroup({ repo_url: 'https://github.com/DocRoms/Kronn.git' }))
        .toBe('DocRoms');
    });

    it('extracts GitLab org from SSH URL', () => {
      expect(getProjectGroup({ repo_url: 'git@gitlab.com:myorg/myproject.git' }))
        .toBe('myorg');
    });

    it('returns local label when no repo_url', () => {
      expect(getProjectGroup({ repo_url: null })).toBe('Local');
      expect(getProjectGroup({ repo_url: null }, 'Perso')).toBe('Perso');
    });

    it('returns other label on invalid URL', () => {
      expect(getProjectGroup({ repo_url: 'not-a-url' })).toBe('Other');
      expect(getProjectGroup({ repo_url: 'not-a-url' }, 'Local', 'Divers')).toBe('Divers');
    });

    it('returns empty string repo_url as local', () => {
      expect(getProjectGroup({ repo_url: '' })).toBe('Local');
    });
  });
});
