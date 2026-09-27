import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DiscussionMosaicComposer } from '../DiscussionMosaicComposer';
import type { Discussion } from '../../types/generated';
import type * as ApiModule from '../../lib/api';

// The real ChatInput, so drafts go through its own per-discussion storage.
const mocks = vi.hoisted(() => ({ get: vi.fn(), send: vi.fn() }));
vi.mock('../../lib/api', async importOriginal => {
  const actual = await importOriginal<typeof ApiModule>();
  return {
    ...actual,
    agents: { ...actual.agents, detect: vi.fn().mockResolvedValue([]) },
    discussions: { ...actual.discussions, get: mocks.get, sendMessageStream: mocks.send },
  };
});
vi.mock('../../lib/stt-engine', () => ({ audioBufferToFloat32: vi.fn(), transcribeAudio: vi.fn().mockResolvedValue('') }));
vi.mock('../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string, ...args: unknown[]) => args.length ? `${key}:${args.join(',')}` : key }) }));

const discussion = (id: string) => ({
  id, title: id, project_id: null, agent: 'ClaudeCode', language: 'fr', participants: ['ClaudeCode'], messages: [],
  message_count: 0, non_system_message_count: 0, skill_ids: [], profile_ids: [], directive_ids: [], archived: false,
  pinned: false, workspace_mode: 'Direct', workspace_path: null, worktree_branch: null, tier: 'Default',
  created_at: '2026-09-24T00:00:00Z', updated_at: '2026-09-24T00:00:00Z',
}) as unknown as Discussion;

const textarea = () => document.querySelector('textarea') as HTMLTextAreaElement;

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  mocks.get.mockImplementation(async (id: string) => discussion(id));
  mocks.send.mockImplementation(async (...args: unknown[]) => {
    (args[8] as (r: unknown) => void)({ message_id: 'm', sort_order: 1, duplicate: false });
  });
});

describe('DiscussionMosaicComposer with the real ChatInput', () => {
  it('keeps each tile draft in its own discussion and never sends it elsewhere', async () => {
    const view = render(<DiscussionMosaicComposer discussionId="a" title="Alpha" toast={vi.fn()} />);
    await waitFor(() => expect(textarea()).not.toBeNull());
    fireEvent.change(textarea(), { target: { value: 'brouillon pour A' } });

    view.rerender(<DiscussionMosaicComposer discussionId="b" title="Beta" toast={vi.fn()} />);
    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('b'));
    await waitFor(() => expect(textarea()).not.toBeNull());
    expect(textarea().value).toBe('');
    fireEvent.change(textarea(), { target: { value: 'message pour B' } });
    await act(async () => { fireEvent.keyDown(textarea(), { key: 'Enter' }); });

    await waitFor(() => expect(mocks.send).toHaveBeenCalledTimes(1));
    expect(mocks.send.mock.calls[0][0]).toBe('b');
    expect(mocks.send.mock.calls[0][1].content).toBe('message pour B');

    view.rerender(<DiscussionMosaicComposer discussionId="a" title="Alpha" toast={vi.fn()} />);
    await waitFor(() => expect(textarea()?.value).toBe('brouillon pour A'));
    expect(mocks.send).toHaveBeenCalledTimes(1);
  });

  it('keeps a draft typed the moment the input appears', async () => {
    // Types from a MutationObserver callback, before any passive effect has
    // run: the window a slow runner's waitFor can land in.
    const typedEarly = new Promise<void>(resolve => {
      const observer = new MutationObserver(() => {
        const field = textarea();
        if (!field) return;
        observer.disconnect();
        fireEvent.change(field, { target: { value: 'tapé tout de suite' } });
        resolve();
      });
      observer.observe(document.body, { childList: true, subtree: true });
    });
    const view = render(<DiscussionMosaicComposer discussionId="a" title="Alpha" toast={vi.fn()} />);
    await typedEarly;
    await waitFor(() => expect(textarea().value).toBe('tapé tout de suite'));
    view.rerender(<DiscussionMosaicComposer discussionId="b" title="Beta" toast={vi.fn()} />);
    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('b'));
    view.rerender(<DiscussionMosaicComposer discussionId="a" title="Alpha" toast={vi.fn()} />);
    await waitFor(() => expect(textarea()?.value).toBe('tapé tout de suite'));
  });

  it('keeps the draft when the input is collapsed and reopened', async () => {
    render(<DiscussionMosaicComposer discussionId="a" title="Alpha" toast={vi.fn()} />);
    await waitFor(() => expect(textarea()).not.toBeNull());
    fireEvent.change(textarea(), { target: { value: 'à reprendre' } });
    fireEvent.click(screen.getByRole('button', { name: /disc.mosaic.replyIn/ }));
    expect(textarea()).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /disc.mosaic.replyIn/ }));
    await waitFor(() => expect(textarea()?.value).toBe('à reprendre'));
  });
});
