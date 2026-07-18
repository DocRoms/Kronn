import { describe, expect, it } from 'vitest';
import {
  buildBatchTriageRows,
  buildContinuationDraft,
  parseBatchTriageResult,
} from '../batchTriage';
import type { Discussion } from '../../types/generated';

const baseDiscussion: Discussion = {
  id: 'disc-1',
  project_id: 'project-1',
  title: 'EW-1234 — Batch child',
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: 0,
  non_system_message_count: 0, tier: "default" as const, summary_strategy: "Auto" as const, introspection_call_count: 0,
  archived: false,
  pinned: false, pin_first_message: false,
  workspace_mode: 'Direct',
  created_at: '2026-06-01T10:00:00Z',
  updated_at: '2026-06-01T10:00:00Z',
  awaiting_agent: false,
};

// The triage QP emits Markdown (verdict-first), NOT JSON. These fixtures mirror
// the two verdict styles the prompt allows.
const MD_REPORT = [
  '## EW-7395 — Triage',
  '**🟢 Prêt à framer** · confiance haute',
  '👉 Cause racine localisée — réponds-moi puis dis « applique ». · **Jira :** À faire',
  '',
  '### ⚠️ Ce qui cloche',
  '- La ligne 82 applique le padding sans l\'exclusion (`_objects.site-main.scss:82-83`)',
  '- ✅ Rien de bloquant.',
  '',
  '### ❓ À trancher',
  '1. On fixe partout ou juste la page tag ?',
  '2. Faut-il purger le cache ?',
  '',
  '_En bref : régression CSS de marge sur la page tag ; le pattern d\'exclusion existe déjà ailleurs._',
  '',
  '---',
  '`_objects.site-main.scss:82` · méta : statut `Unspecified`',
].join('\n');

describe('batchTriage (Markdown parser)', () => {
  it('parses the verdict, confidence, ticket id and summary from the Markdown report', () => {
    const { result, error } = parseBatchTriageResult(MD_REPORT);
    expect(error).toBeNull();
    expect(result?.ticket_id).toBe('EW-7395');
    expect(result?.verdict).toBe('Prêt à framer');
    expect(result?.confidence).toBe('haute');
    expect(result?.human_summary).toContain('régression CSS');
    expect(result?.next_action).toContain('Cause racine localisée');
  });

  it('extracts the numbered "À trancher" questions and skips the "Rien de bloquant" sentinel', () => {
    const { result } = parseBatchTriageResult(MD_REPORT);
    expect(result?.open_questions).toEqual([
      'On fixe partout ou juste la page tag ?',
      'Faut-il purger le cache ?',
    ]);
    // "✅ Rien de bloquant." is a sentinel, not a real issue → dropped.
    expect(result?.issues).toEqual([
      'La ligne 82 applique le padding sans l\'exclusion (`_objects.site-main.scss:82-83`)',
    ]);
  });

  it('also handles the explicit "**Verdict :** X · **Confiance :** Y" style', () => {
    const { result } = parseBatchTriageResult(
      '## EW-9 — Triage\n**Verdict :** Décision humaine requise · **Confiance :** moyenne\n\n_En bref : à arbitrer._',
    );
    expect(result?.verdict).toBe('Décision humaine requise');
    expect(result?.confidence).toBe('moyenne');
  });

  it('flags an unrecognized (non-triage) reply', () => {
    expect(parseBatchTriageResult('just some prose, no triage structure').error).toBe('unrecognized_format');
    expect(parseBatchTriageResult('').error).toBe('empty');
  });

  it('builds rows from the LAST agent message', () => {
    const rows = buildBatchTriageRows([{
      ...baseDiscussion,
      messages: [
        { id: 'u1', role: 'User', content: 'EW-1234', agent_type: null, timestamp: '2026-06-01T10:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a1', role: 'Agent', content: '## EW-OLD — Triage\n**Verdict :** Bloqué', agent_type: 'ClaudeCode', timestamp: '2026-06-01T10:01:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a2', role: 'Agent', content: MD_REPORT, agent_type: 'ClaudeCode', timestamp: '2026-06-01T10:02:00Z', tokens_used: 0, auth_mode: null },
      ],
    }]);
    expect(rows[0].result?.ticket_id).toBe('EW-7395');
    expect(rows[0].result?.open_questions?.length).toBe(2);
  });

  it('builds an interactive continuation draft from the parsed report', () => {
    const rows = buildBatchTriageRows([{
      ...baseDiscussion,
      messages: [
        { id: 'a1', role: 'Agent', content: MD_REPORT, agent_type: 'ClaudeCode', timestamp: '2026-06-01T10:01:00Z', tokens_used: 0, auth_mode: null },
      ],
    }]);
    const draft = buildContinuationDraft(rows[0]);
    expect(draft).toContain('Ticket: EW-7395');
    expect(draft).toContain('Source discussion: disc-1');
    expect(draft).toContain('write_intent: preview_only_until_explicit_yes');
    expect(draft).toContain('requires_human_confirmation_before_write: true');
    expect(draft).toContain('- On fixe partout ou juste la page tag ?');
  });
});
