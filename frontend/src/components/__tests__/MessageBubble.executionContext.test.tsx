import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// `vi.mock` is hoisted above module-level declarations, so the mock fn must
// be created inside `vi.hoisted` to be available when the factory runs.
const { reveal } = vi.hoisted(() => ({ reveal: vi.fn().mockResolvedValue('temporary-secret') }));
vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return {
    ...real,
    config: { getUiLanguage: vi.fn().mockResolvedValue('en') },
    executionVariables: { reveal, metadata: vi.fn(), extend: vi.fn() },
  };
});

import { MessageBubble } from '../MessageBubble';
import type { DiscussionMessage } from '../../types/generated';

const message: DiscussionMessage = {
  id: 'context-message',
  role: 'System',
  channel: 'main',
  content: `execution_context:${JSON.stringify({
    run_kind: 'workflow', run_id: 'run-1', snapshot_id: 'snapshot-1',
    resolved_at: '2026-08-31T10:00:00Z', expires_at: null, purged: false,
    variables: [{ name: 'token', effective_source_ref: '<env.TOKEN>', overridden: false }],
  })}`,
  agent_type: null,
  timestamp: '2026-08-31T10:00:00Z',
  tokens_used: 0,
  auth_mode: null,
  model_tier: null,
  author_pseudo: null,
  author_avatar_email: null,
};

const props = {
  idx: 0, isLastUser: false, isLastAgent: false, isEditing: false, isCopied: false,
  isTtsActive: false, ttsState: 'idle' as const, isExpandedSummary: false,
  prevUserTs: null, defaultAgent: 'Codex' as const, summaryCache: null,
  language: 'en', sending: false, editingText: '', hasFullAccess: false,
  onCopy: vi.fn(), onTts: vi.fn(), onEditStart: vi.fn(), onEditCancel: vi.fn(),
  onEditSubmit: vi.fn(), onEditTextChange: vi.fn(), onRetry: vi.fn(),
  onExpandSummary: vi.fn(), onNavigate: vi.fn(), t: (key: string) => key,
};

describe('MessageBubble execution context', () => {
  it('reveals only on demand and remasks immediately', async () => {
    render(<I18nProvider><MessageBubble {...props} msg={message} /></I18nProvider>);
    expect(screen.queryByText('temporary-secret')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Reveal token temporarily' }));
    await waitFor(() => expect(screen.getByText('temporary-secret')).toBeTruthy());
    expect(reveal).toHaveBeenCalledWith('workflow', 'run-1', 'token');
    fireEvent.click(screen.getByRole('button', { name: 'Remask token' }));
    expect(screen.queryByText('temporary-secret')).toBeNull();
  });
});
