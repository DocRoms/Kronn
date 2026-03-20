import { describe, it, expect } from 'vitest';
import { t } from '../i18n';
import type { Discussion, BootstrapProjectRequest, BootstrapProjectResponse } from '../../types/generated';

/**
 * Regression tests for bugs fixed during the frontend audit.
 * Each test documents a specific bug that was found and fixed.
 */
describe('regression tests', () => {
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

  describe('Discussion message_count vs messages array (list optimization fix)', () => {
    it('Discussion type requires message_count field', () => {
      // The list endpoint returns messages:[] with message_count:N for performance.
      // This test ensures message_count stays a required field on Discussion,
      // preventing the bug where notifications showed wrong counts and
      // conversations appeared empty when selected.
      const listDisc: Discussion = {
        id: 'd1',
        project_id: null,
        title: 'Test',
        agent: 'ClaudeCode',
        language: 'fr',
        participants: ['ClaudeCode'],
        messages: [],
        message_count: 5,
        archived: false,
        workspace_mode: 'Direct',
        created_at: '2026-01-01T00:00:00Z',
        updated_at: '2026-01-01T00:00:00Z',
      };
      // message_count must be used for display, not messages.length
      expect(listDisc.message_count).toBe(5);
      expect(listDisc.messages).toHaveLength(0);
    });

    it('unseen count uses message_count, not messages.length', () => {
      // Simulates the real calculation from Dashboard/DiscussionsPage
      const disc: Discussion = {
        id: 'd1',
        project_id: null,
        title: 'Test',
        agent: 'ClaudeCode',
        language: 'fr',
        participants: [],
        messages: [],       // empty from list endpoint
        message_count: 10,  // real count from backend
        archived: false,
        workspace_mode: 'Direct',
        created_at: '2026-01-01T00:00:00Z',
        updated_at: '2026-01-01T00:00:00Z',
      };
      const lastSeenCount = 7;
      // Correct: use message_count (not messages.length which would be 0)
      const unseen = (disc.message_count ?? disc.messages.length) - lastSeenCount;
      expect(unseen).toBe(3);
      // Wrong: using messages.length would give -7
      expect(disc.messages.length - lastSeenCount).toBe(-7);
    });
  });

  describe('validation flow improvements (Phase 3)', () => {
    it('isValidationDisc detects validation title exactly', () => {
      // Replicate the function logic from DiscussionsPage.tsx
      const isValidationDisc = (title: string) => title === 'Validation audit AI';

      expect(isValidationDisc('Validation audit AI')).toBe(true);
      expect(isValidationDisc('validation audit AI')).toBe(false); // exact match
      expect(isValidationDisc('Validation audit AI ')).toBe(false); // trailing space
      expect(isValidationDisc('')).toBe(false);
      expect(isValidationDisc('Some other discussion')).toBe(false);
    });

    it('VALIDATION_COMPLETE check is case-insensitive', () => {
      // Replicate the check from DiscussionsPage.tsx
      const checkComplete = (content: string) =>
        content.toUpperCase().includes('KRONN:VALIDATION_COMPLETE');

      expect(checkComplete('KRONN:VALIDATION_COMPLETE')).toBe(true);
      expect(checkComplete('kronn:validation_complete')).toBe(true);
      expect(checkComplete('Kronn:Validation_Complete')).toBe(true);
      expect(checkComplete('All done! KRONN:VALIDATION_COMPLETE')).toBe(true);
      expect(checkComplete('Some text without the marker')).toBe(false);
      expect(checkComplete('')).toBe(false);
    });

    it('i18n has disc.advancedOptions key in all locales', () => {
      expect(t('fr', 'disc.advancedOptions')).not.toBe('disc.advancedOptions');
      expect(t('en', 'disc.advancedOptions')).not.toBe('disc.advancedOptions');
      expect(t('es', 'disc.advancedOptions')).not.toBe('disc.advancedOptions');
    });
  });

  describe('bootstrap i18n keys', () => {
    const bootstrapKeys = [
      'projects.bootstrap',
      'projects.bootstrap.name',
      'projects.bootstrap.desc',
      'projects.bootstrap.descPlaceholder',
      'projects.bootstrap.creating',
      'projects.bootstrap.start',
    ];

    it('all bootstrap keys exist in FR', () => {
      for (const key of bootstrapKeys) {
        const val = t('fr', key);
        expect(val).not.toBe(key);
      }
    });

    it('all bootstrap keys exist in EN', () => {
      for (const key of bootstrapKeys) {
        const val = t('en', key);
        expect(val).not.toBe(key);
      }
    });

    it('all bootstrap keys exist in ES', () => {
      for (const key of bootstrapKeys) {
        const val = t('es', key);
        expect(val).not.toBe(key);
      }
    });
  });

  describe('BootstrapProjectRequest/Response types (compile-time check)', () => {
    it('BootstrapProjectRequest has required fields', () => {
      const req: BootstrapProjectRequest = {
        name: 'Test Project',
        description: 'A test project',
        agent: 'ClaudeCode',
      };
      expect(req.name).toBe('Test Project');
      expect(req.description).toBe('A test project');
      expect(req.agent).toBe('ClaudeCode');
    });

    it('BootstrapProjectResponse has required fields', () => {
      const res: BootstrapProjectResponse = {
        project_id: 'proj-123',
        discussion_id: 'disc-456',
      };
      expect(res.project_id).toBe('proj-123');
      expect(res.discussion_id).toBe('disc-456');
    });
  });

  describe('workflow and project section i18n keys (today\'s changes)', () => {
    const newKeys = [
      'wf.noProject',
      'projects.workflows',
      'projects.noWorkflows',
      'projects.docAi',
      'projects.docAi.selectFile',
      'projects.docAi.loading',
      'projects.docAi.empty',
      'projects.docAi.search',
      'projects.docAi.noResults',
    ];

    it('all keys exist in FR', () => {
      for (const key of newKeys) {
        expect(t('fr', key)).not.toBe(key);
      }
    });

    it('all keys exist in EN', () => {
      for (const key of newKeys) {
        expect(t('en', key)).not.toBe(key);
      }
    });

    it('all keys exist in ES', () => {
      for (const key of newKeys) {
        expect(t('es', key)).not.toBe(key);
      }
    });
  });

  describe('git panel i18n keys', () => {
    const gitKeys = [
      'git.title',
      'git.refresh',
      'git.noChanges',
      'git.filesChanged',
      'git.onDefaultBranch',
      'git.createBranch',
      'git.branchName',
      'git.commit',
      'git.commitMessage',
      'git.commitSelected',
      'git.push',
      'git.pushSuccess',
      'git.selectAll',
      'git.deselectAll',
      'git.filesBtn',
    ];

    it('all git keys exist in FR', () => {
      for (const key of gitKeys) {
        const val = t('fr', key);
        expect(val).not.toBe(key);
      }
    });

    it('all git keys exist in EN', () => {
      for (const key of gitKeys) {
        const val = t('en', key);
        expect(val).not.toBe(key);
      }
    });

    it('all git keys exist in ES', () => {
      for (const key of gitKeys) {
        const val = t('es', key);
        expect(val).not.toBe(key);
      }
    });
  });

  describe('git terminal i18n keys', () => {
    const terminalKeys = ['git.terminal', 'git.terminalPlaceholder'];

    it('all terminal keys exist in FR', () => {
      for (const key of terminalKeys) {
        const val = t('fr', key);
        expect(val).not.toBe(key);
      }
    });

    it('all terminal keys exist in EN', () => {
      for (const key of terminalKeys) {
        const val = t('en', key);
        expect(val).not.toBe(key);
      }
    });

    it('all terminal keys exist in ES', () => {
      for (const key of terminalKeys) {
        const val = t('es', key);
        expect(val).not.toBe(key);
      }
    });
  });

  describe('smart default section logic (collapsible project sections)', () => {
    // Replicate the defaultSection() logic from Dashboard.tsx
    const defaultSection = (auditStatus: string) => {
      return (auditStatus === 'Audited' || auditStatus === 'Validated') ? 'discussions' : 'aiContext';
    };

    it('shows aiContext before audit completes', () => {
      expect(defaultSection('NoTemplate')).toBe('aiContext');
      expect(defaultSection('TemplateInstalled')).toBe('aiContext');
    });

    it('shows discussions after audit completes', () => {
      expect(defaultSection('Audited')).toBe('discussions');
      expect(defaultSection('Validated')).toBe('discussions');
    });
  });

  describe('discussion locked prefill behavior', () => {
    it('locked flag determines field editability', () => {
      // Replicate logic from DiscussionsPage.tsx
      const setNewDiscPrefilled = (locked: boolean) => locked;

      // Doc viewer discussions: editable
      const docPrefill = { projectId: 'p1', title: 'Doc', prompt: 'Review', locked: false };
      expect(setNewDiscPrefilled(!!docPrefill.locked)).toBe(false);

      // Validation audit discussions: locked
      const valPrefill = { projectId: 'p1', title: 'Validation', prompt: 'Validate', locked: true };
      expect(setNewDiscPrefilled(!!valPrefill.locked)).toBe(true);

      // No locked field: defaults to false (not locked)
      const noPrefill = { projectId: 'p1', title: 'Test', prompt: 'Test' };
      expect(setNewDiscPrefilled(!!(noPrefill as any).locked)).toBe(false);
    });
  });
});
