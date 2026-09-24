import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { DiscussionMosaicComposer } from '../DiscussionMosaicComposer';
import { agents, autoTriggersApi, config, discussions } from '../../lib/api';
import { loadDraft } from '../../lib/chat-drafts';
import type { Discussion } from '../../types/generated';

vi.mock('../../lib/stt-engine', () => ({ audioBufferToFloat32: vi.fn(), transcribeAudio: vi.fn().mockResolvedValue('') }));
vi.mock('../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string, ...args: unknown[]) => args.length ? `${key}:${args.join(',')}` : key }) }));

const disc = (id: string) => ({
  id, title: id, project_id: null, agent: 'ClaudeCode', language: 'fr', participants: ['ClaudeCode'],
  messages: [], message_count: 0, non_system_message_count: 0, skill_ids: [], profile_ids: [], directive_ids: [],
  archived: false, pinned: false, workspace_mode: 'Direct', workspace_path: null, worktree_branch: null,
  tier: 'Default', pin_first_message: false, summary_cache: null, summary_up_to_msg_idx: null,
  shared_id: null, shared_with: [], workflow_run_id: null, created_at: '2026-09-24T09:00:00Z', updated_at: '2026-09-24T09:00:00Z',
}) as unknown as Discussion;
const props = (id: string) => ({ discussionId: id, title: id, toast: vi.fn() });
const input = () => screen.getByRole('textbox') as HTMLTextAreaElement;

beforeEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
  vi.spyOn(agents, 'detect').mockResolvedValue([]);
  vi.spyOn(discussions, 'get').mockImplementation(async id => disc(id));
  vi.spyOn(discussions, 'participants').mockResolvedValue([]);
  vi.spyOn(discussions, 'nativeAgentMode').mockResolvedValue({ disabled: false });
  vi.spyOn(autoTriggersApi, 'listDisabled').mockResolvedValue([]);
  vi.spyOn(config, 'getServerConfig').mockResolvedValue({ discussion_notes_enabled: true } as never);
});

describe('mosaic with its real ChatInput', () => {
  it('retains separate drafts on rapid tile switches and collapse/reopen', async () => {
    const view = render(<DiscussionMosaicComposer {...props('a')} />);
    await screen.findByRole('textbox');
    fireEvent.change(input(), { target: { value: 'draft A' } });
    await act(async () => { view.rerender(<DiscussionMosaicComposer {...props('b')} />); });
    await waitFor(() => expect(input().value).toBe(''));
    expect(discussions.nativeAgentMode).toHaveBeenLastCalledWith('b');
    fireEvent.change(input(), { target: { value: 'draft B' } });
    await act(async () => { view.rerender(<DiscussionMosaicComposer {...props('a')} />); });
    await waitFor(() => expect(input().value).toBe('draft A'));
    expect(loadDraft('b')?.text).toBe('draft B');
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.replyIn:a' }));
    expect(screen.queryByRole('textbox')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.replyIn:a' }));
    await waitFor(() => expect(input().value).toBe('draft A'));
  });

  it('does not restore an accepted note when the receipt arrives while collapsed', async () => {
    let acknowledge!: () => void;
    const send = vi.spyOn(discussions, 'sendMessageStream').mockImplementation((...args) => new Promise<void>(resolve => {
      acknowledge = () => { args[8]?.({ message_id: 'saved-note', sort_order: 1, duplicate: false }); resolve(); };
    }));
    render(<DiscussionMosaicComposer {...props('a')} />);
    await screen.findByRole('textbox');
    fireEvent.click(await screen.findByLabelText('disc.note.sendAsNote'));
    fireEvent.change(input(), { target: { value: 'a persisted note' } });
    fireEvent.click(screen.getByLabelText('Send message'));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0][1].channel).toBe('note');
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.replyIn:a' }));
    await act(async () => { acknowledge(); });
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.replyIn:a' }));
    await waitFor(() => expect(input().value).toBe(''));
    expect(loadDraft('a')).toBeNull();
  });

  it('clears the restored submitted note when the composer is reopened before receipt', async () => {
    let acknowledge!: () => void;
    const send = vi.spyOn(discussions, 'sendMessageStream').mockImplementation((...args) => new Promise<void>(resolve => {
      acknowledge = () => { args[8]?.({ message_id: 'saved-note', sort_order: 1, duplicate: false }); resolve(); };
    }));
    render(<DiscussionMosaicComposer {...props('a')} />);
    await screen.findByRole('textbox');
    fireEvent.click(await screen.findByLabelText('disc.note.sendAsNote'));
    fireEvent.change(input(), { target: { value: 'a persisted note' } });
    fireEvent.click(screen.getByLabelText('Send message'));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.replyIn:a' }));
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.replyIn:a' }));
    await waitFor(() => expect(input().value).toBe('a persisted note'));
    await act(async () => { acknowledge(); });
    expect(input().value).toBe('');
    expect(loadDraft('a')).toBeNull();
  });

  it('preserves a newer draft in A when the note receipt arrives while B is mounted', async () => {
    let acknowledge!: () => void;
    const send = vi.spyOn(discussions, 'sendMessageStream').mockImplementation((...args) => new Promise<void>(resolve => {
      acknowledge = () => { args[8]?.({ message_id: 'saved-note', sort_order: 1, duplicate: false }); resolve(); };
    }));
    const view = render(<DiscussionMosaicComposer {...props('a')} />);
    await screen.findByRole('textbox');
    fireEvent.click(await screen.findByLabelText('disc.note.sendAsNote'));
    fireEvent.change(input(), { target: { value: 'sent note A' } });
    fireEvent.click(screen.getByLabelText('Send message'));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    fireEvent.change(input(), { target: { value: 'new unsent draft A' } });
    await act(async () => { view.rerender(<DiscussionMosaicComposer {...props('b')} />); });
    await waitFor(() => expect(input().value).toBe(''));
    expect(loadDraft('a')?.text).toBe('new unsent draft A');
    fireEvent.change(input(), { target: { value: 'draft B' } });
    await act(async () => { acknowledge(); });
    expect(loadDraft('a')?.text).toBe('new unsent draft A');
    expect(input().value).toBe('draft B');
    await act(async () => { view.rerender(<DiscussionMosaicComposer {...props('a')} />); });
    await waitFor(() => expect(input().value).toBe('new unsent draft A'));
  });

  it('keeps a newer draft typed before collapsing when the late receipt arrives', async () => {
    let acknowledge!: () => void;
    vi.spyOn(discussions, 'sendMessageStream').mockImplementation((...args) => new Promise<void>(resolve => {
      acknowledge = () => { args[8]?.({ message_id: 'saved-note', sort_order: 1, duplicate: false }); resolve(); };
    }));
    render(<DiscussionMosaicComposer {...props('a')} />);
    await screen.findByRole('textbox');
    fireEvent.click(await screen.findByLabelText('disc.note.sendAsNote'));
    fireEvent.change(input(), { target: { value: 'the sent note' } });
    fireEvent.click(screen.getByLabelText('Send message'));
    fireEvent.change(input(), { target: { value: 'a newer draft' } });
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.replyIn:a' }));
    await act(async () => { acknowledge(); });
    expect(loadDraft('a')?.text).toBe('a newer draft');
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.replyIn:a' }));
    await waitFor(() => expect(input().value).toBe('a newer draft'));
  });
});
