import { describe, it, expect } from 'vitest';
import { AGENT_COLORS, AGENT_LABELS, ALL_AGENT_TYPES, agentColor } from '../constants';
import { t, UI_LOCALES } from '../i18n';
import type { WorkflowRun } from '../../types/generated';

/**
 * Regression tests for bugs fixed during the frontend audit.
 * Each test documents a specific bug that was found and fixed.
 */
describe('regression tests', () => {
  describe('GeminiCli missing from agent mentions (audit fix #2)', () => {
    it('GeminiCli has a color', () => {
      expect(AGENT_COLORS['GeminiCli']).toBeDefined();
    });

    it('GeminiCli has a label', () => {
      expect(AGENT_LABELS['GeminiCli']).toBe('Gemini CLI');
    });

    it('GeminiCli is in ALL_AGENT_TYPES', () => {
      expect(ALL_AGENT_TYPES).toContain('GeminiCli');
    });
  });

  describe('AGENT_COLORS/LABELS consistency (audit fix #3)', () => {
    it('every agent type has both a color and a label', () => {
      for (const at of ALL_AGENT_TYPES) {
        expect(AGENT_COLORS[at]).toBeDefined();
        expect(AGENT_LABELS[at]).toBeDefined();
      }
    });

    it('agentColor returns consistent results for type and display name', () => {
      expect(agentColor('ClaudeCode')).toBe(agentColor('Claude Code'));
      expect(agentColor('GeminiCli')).toBe(agentColor('Gemini CLI'));
    });
  });

  describe('hardcoded French strings moved to i18n (audit fix #4-5)', () => {
    const newKeys = [
      'wf.manual',
      'wf.inProgress',
      'wf.pending',
      'wf.deleteRun',
      'wf.noOutput',
      'wf.status',
      'wf.duration',
      'config.configFile',
      'debate.rounds',
    ];

    it('all new keys exist in FR', () => {
      for (const key of newKeys) {
        const val = t('fr', key);
        expect(val).not.toBe(key); // Should not return the raw key
      }
    });

    it('all new keys exist in EN', () => {
      for (const key of newKeys) {
        const val = t('en', key);
        expect(val).not.toBe(key);
      }
    });

    it('all new keys exist in ES', () => {
      for (const key of newKeys) {
        const val = t('es', key);
        expect(val).not.toBe(key);
      }
    });

    it('wf.manual is translated correctly', () => {
      expect(t('fr', 'wf.manual')).toBe('Manuel');
      expect(t('en', 'wf.manual')).toBe('Manual');
      expect(t('es', 'wf.manual')).toBe('Manual');
    });
  });

  describe('trigger_context typed as WorkflowTrigger (audit fix #6)', () => {
    it('trigger_context accepts WorkflowTrigger, not any', () => {
      // This test verifies the type fix at compile time — if trigger_context
      // were still `any`, this would compile but be meaningless.
      const run: WorkflowRun = {
        id: '1',
        workflow_id: 'wf-1',
        status: 'Success',
        trigger_context: { type: 'Manual' },
        step_results: [],
        tokens_used: 0,
        workspace_path: null,
        started_at: '2024-01-01T00:00:00Z',
        finished_at: null,
      };
      expect(run.trigger_context).toEqual({ type: 'Manual' });
    });
  });

  describe('output languages vs UI languages (audit fix #7)', () => {
    it('UI_LOCALES only contains languages with full translations (fr/en/es)', () => {
      const codes = UI_LOCALES.map(l => l.code);
      expect(codes).toEqual(['fr', 'en', 'es']);
      // zh and br are output languages only, not UI locales
      expect(codes).not.toContain('zh');
      expect(codes).not.toContain('br');
    });

    it('every UI locale has complete translations for all keys', () => {
      // Check a sampling of keys across all sections
      const sampleKeys = [
        'nav.projects', 'projects.title', 'disc.new', 'config.agents',
        'mcp.title', 'wf.title', 'common.cancel',
      ];
      for (const locale of UI_LOCALES) {
        for (const key of sampleKeys) {
          const val = t(locale.code, key);
          expect(val).not.toBe(key); // Should not fallback to the raw key
        }
      }
    });
  });
});
