// KT-2084 — an assistant conversation is a discussion, deleted through the shared path
// (server refusal word for word, kept worktree said, already gone counted as deleted).
import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, cleanup, fireEvent, act, waitFor } from '@testing-library/react';

vi.mock('../../lib/api', () => ({
  discussions: { delete: vi.fn() },
  assistantConversations: { list: vi.fn() },
}));

import { AssistantConversationList } from '../AssistantConversationList';
import { ASSISTANT_CONVERSATIONS_CHANGED } from '../assistantConversation';
import { assistantConversations, discussions } from '../../lib/api';
import { ApiRequestError } from '../../lib/apiRequestError';
import type { AssistantConversation } from '../../types/generated';

const t = (key: string, ...args: (string | number)[]) =>
  (args.length === 0 ? key : `${key}(${args.join(',')})`);

const conversation: AssistantConversation = {
  discussion_id: 'conv-1',
  kind: 'custom_api',
  target_id: null,
  target_step: null,
  plugin_id: null,
  target_label: 'Weather API',
  last_proposal_signature: null,
  last_applied_signature: null,
  last_applied_at: null,
  created_at: '2026-11-01T00:00:00Z',
  title: 'helper',
  agent: 'ClaudeCode',
  archived: false,
  message_count: 2,
  updated_at: '2026-11-01T00:00:00Z',
};

const REFUSAL = '1 uncommitted file(s) in /repo/.kronn/worktrees/p--helper (notes.md): commit or discard them first';

/** The list opened, the delete of its conversation clicked and confirmed. */
const deleteTheConversation = async () => {
  render(
    <AssistantConversationList
      filter={{ kind: 'custom_api' }}
      activeDiscussionId={null}
      onResume={vi.fn()}
      t={t}
    />,
  );
  fireEvent.click(await screen.findByText('aiHelper.conversations.title(1)'));
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'aiHelper.conversations.delete' }));
  });
};

beforeEach(() => {
  vi.mocked(assistantConversations.list).mockResolvedValue([conversation]);
  vi.stubGlobal('confirm', vi.fn(() => true));
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.mocked(discussions.delete).mockReset();
});

describe('AssistantConversationList delete', () => {
  it('shows the server refusal word for word and keeps the conversation', async () => {
    vi.mocked(discussions.delete).mockRejectedValue(new ApiRequestError(REFUSAL, 'conflict'));
    const changed = vi.fn();
    window.addEventListener(ASSISTANT_CONVERSATIONS_CHANGED, changed);

    await deleteTheConversation();

    expect(await screen.findByRole('alert')).toHaveTextContent(`disc.deleteRefused(${REFUSAL})`);
    expect(changed).not.toHaveBeenCalled();
    window.removeEventListener(ASSISTANT_CONVERSATIONS_CHANGED, changed);
  });

  it('says where the server kept a worktree it could not remove', async () => {
    const kept = 'worktree kept at /repo/.kronn/worktrees/p--helper: git kept it: fatal: busy';
    vi.mocked(discussions.delete).mockResolvedValue(kept as never);

    await deleteTheConversation();

    expect(await screen.findByRole('status')).toHaveTextContent(`disc.deleteWorktreeKept(${kept})`);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('counts a conversation already gone as deleted', async () => {
    vi.mocked(discussions.delete).mockRejectedValue(new ApiRequestError('Discussion not found', 'not_found'));
    const changed = vi.fn();
    window.addEventListener(ASSISTANT_CONVERSATIONS_CHANGED, changed);

    await deleteTheConversation();

    await waitFor(() => expect(changed).toHaveBeenCalled());
    expect(screen.queryByRole('alert')).toBeNull();
    window.removeEventListener(ASSISTANT_CONVERSATIONS_CHANGED, changed);
  });
});
