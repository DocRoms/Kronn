/**
 * KT-1664 — an exhausted-quota escalation the provider re-check resolved no
 * longer says a human must act: its worker resumed, and the notice reads so in
 * every language instead of "Intervention humaine requise".
 */
import { afterEach, describe, it, expect } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { DiscussionMessage } from '../../types/generated';
import en from '../../lib/i18n/locales/en';
import fr from '../../lib/i18n/locales/fr';
import es from '../../lib/i18n/locales/es';
import zh from '../../lib/i18n/locales/zh';

import { MessageBubble } from '../MessageBubble';

const ESCALATION_ID = 'provider-quota-escalation:job-1';

const escalation = {
  id: ESCALATION_ID, role: 'User', channel: 'main',
  content: '**Intervention humaine requise — quota épuisé**\n\nLe provider `ClaudeCode` a refusé le dispatch `job-1`.',
  agent_type: null, timestamp: new Date().toISOString(), tokens_used: 0, auth_mode: null,
  model_tier: null, author_pseudo: 'Orchestrateur', author_avatar_email: null, kind: 'orchestration',
} as DiscussionMessage;

const LOCALES = { en, fr, es, zh } as const;

function props(language: keyof typeof LOCALES) {
  const dict = LOCALES[language] as Record<string, string>;
  return {
    idx: 0, isLastUser: false, isLastAgent: false, isEditing: false, isCopied: false,
    isTtsActive: false, ttsState: 'idle', isExpandedSummary: false, prevUserTs: null,
    defaultAgent: 'ClaudeCode' as const, summaryCache: null, language, sending: false,
    editingText: '', hasFullAccess: false, onCopy: () => {}, onTts: () => {},
    onEditStart: () => {}, onEditCancel: () => {}, onEditSubmit: () => {},
    onEditTextChange: () => {}, onRetry: () => {}, onExpandSummary: () => {},
    onNavigate: () => {}, discussionId: 'disc-test',
    t: (key: string) => dict[key] ?? key,
  };
}

afterEach(cleanup);

describe('a quota escalation the worker resumed from (KT-1664)', () => {
  it('still asks for a human while the quota is exhausted: shown open, no details toggle', () => {
    render(<I18nProvider><MessageBubble {...props('en')} msg={escalation} /></I18nProvider>);
    expect(screen.getByText(/Intervention humaine requise/)).toBeVisible();
    expect(screen.queryByRole('button', { name: en['disc.orchestratorShowDetails'] })).toBeNull();
  });

  it.each(Object.keys(LOCALES) as (keyof typeof LOCALES)[])(
    'reads as settled in %s, and folds behind its details like any orchestrator update',
    language => {
      render(<I18nProvider><MessageBubble {...props(language)} quotaResumed msg={escalation} /></I18nProvider>);
      const dict = LOCALES[language] as Record<string, string>;
      expect(dict['disc.orchestratorQuotaResumed']).toBeTruthy();
      expect(screen.getByText(dict['disc.orchestratorQuotaResumed'])).toBeVisible();
      expect(screen.queryByText(/Intervention humaine requise/)).toBeNull();
      expect(screen.getByRole('button', { name: dict['disc.orchestratorShowDetails'] })).toBeVisible();
    },
  );
});
