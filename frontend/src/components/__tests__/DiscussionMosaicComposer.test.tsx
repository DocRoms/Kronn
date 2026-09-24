import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DiscussionMosaicComposer } from '../DiscussionMosaicComposer';
import { MESSAGE_SEND_SETTLED_EVENT } from '../../lib/messageSendLifecycle';
import type { Discussion, MessageChannel } from '../../types/generated';

const mocks = vi.hoisted(() => ({ get: vi.fn(), send: vi.fn(), detect: vi.fn(), props: vi.fn() }));
vi.mock('../../lib/api', () => ({
  agents: { detect: mocks.detect },
  discussions: { get: mocks.get, sendMessageStream: mocks.send },
}));
vi.mock('../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string, ...args: unknown[]) => args.length ? `${key}:${args.join(',')}` : key }) }));
vi.mock('../ChatInput', () => ({
  ChatInput: (props: { discussion: Discussion; onSend: (text: string, targets?: unknown, all?: boolean, reply?: string, channel?: MessageChannel) => void }) => {
    mocks.props(props);
    return <div data-testid="chat-input" data-discussion={props.discussion.id}>
      <button type="button" onClick={() => props.onSend(`hello ${props.discussion.id}`)}>send</button>
      <button type="button" onClick={() => props.onSend('a note', undefined, false, undefined, 'note')}>note</button>
    </div>;
  },
}));

const discussion = (id: string) => ({ id, title: id, agent: 'Codex', participants: [], messages: [] }) as unknown as Discussion;
const accept = async (...args: unknown[]) => { (args[8] as (r: unknown) => void)({ message_id: 'm', sort_order: 1, duplicate: false }); };
const settledEvents = () => {
  const settled = vi.fn();
  window.addEventListener(MESSAGE_SEND_SETTLED_EVENT, event => settled((event as CustomEvent).detail));
  return settled;
};

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  mocks.detect.mockResolvedValue([]);
  mocks.get.mockImplementation(async (id: string) => discussion(id));
  mocks.send.mockImplementation(accept);
});

describe('DiscussionMosaicComposer', () => {
  it('offers no input and loads nothing until a tile is selected', async () => {
    render(<DiscussionMosaicComposer discussionId={null} title="" toast={vi.fn()} />);
    await act(async () => {});
    expect(screen.queryByTestId('chat-input')).toBeNull();
    expect(screen.getAllByText('disc.mosaic.selectToReply').length).toBeGreaterThan(0);
    expect(mocks.get).not.toHaveBeenCalled();
  });

  it('hides the controls this page cannot serve', async () => {
    render(<DiscussionMosaicComposer discussionId="a" title="Alpha" toast={vi.fn()} />);
    await screen.findByTestId('chat-input');
    expect(mocks.props).toHaveBeenLastCalledWith(expect.objectContaining({ showVoiceControls: false, showDebate: false, sending: false }));
  });

  it('never shows the previous discussion input while the newly selected one loads, and ignores its late answer', async () => {
    const view = render(<DiscussionMosaicComposer discussionId="a" title="Alpha" toast={vi.fn()} />);
    expect(await screen.findByTestId('chat-input')).toHaveAttribute('data-discussion', 'a');
    let resolveB!: (value: Discussion) => void;
    let resolveC!: (value: Discussion) => void;
    mocks.get.mockReturnValueOnce(new Promise(r => { resolveB = r; })).mockReturnValueOnce(new Promise(r => { resolveC = r; }));
    view.rerender(<DiscussionMosaicComposer discussionId="b" title="Beta" toast={vi.fn()} />);
    expect(screen.queryByTestId('chat-input')).toBeNull();
    view.rerender(<DiscussionMosaicComposer discussionId="c" title="Gamma" toast={vi.fn()} />);
    await act(async () => { resolveB(discussion('b')); });
    expect(screen.queryByTestId('chat-input')).toBeNull();
    await act(async () => { resolveC(discussion('c')); });
    expect(screen.getByTestId('chat-input')).toHaveAttribute('data-discussion', 'c');
  });

  it('queues the message durably for its own discussion and sends it with the outbox identity', async () => {
    const toast = vi.fn();
    const settled = settledEvents();
    render(<DiscussionMosaicComposer discussionId="a" title="Alpha" toast={toast} />);
    await screen.findByTestId('chat-input');
    fireEvent.click(screen.getByText('send'));
    expect(settled).toHaveBeenCalledWith({ discussionId: 'a', message: 'hello a', settlement: 'accepted' });
    await waitFor(() => expect(mocks.send).toHaveBeenCalledTimes(1));
    const [id, request] = mocks.send.mock.calls[0];
    expect(id).toBe('a');
    expect(request).toMatchObject({ content: 'hello a', channel: 'main', defer_dispatch: true, targets: [] });
    await waitFor(() => expect(toast).toHaveBeenCalledWith('disc.mosaic.sent:Alpha', 'success'));
    await waitFor(() => expect(localStorage.getItem('kronn:message-outbox:a')).toBeNull());
  });

  it('keeps a failed message visible and retries it with the same identity', async () => {
    mocks.send.mockImplementationOnce(async (...args: unknown[]) => { (args[4] as (e: string) => void)('backend restarting'); });
    render(<DiscussionMosaicComposer discussionId="a" title="Alpha" toast={vi.fn()} />);
    await screen.findByTestId('chat-input');
    fireEvent.click(screen.getByText('send'));
    await screen.findByText('disc.mosaic.sendFailed');
    expect(localStorage.getItem('kronn:message-outbox:a')).toContain('hello a');
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.retry' }));
    await waitFor(() => expect(mocks.send).toHaveBeenCalledTimes(2));
    expect(mocks.send.mock.calls[1][1].client_message_id).toBe(mocks.send.mock.calls[0][1].client_message_id);
    await waitFor(() => expect(screen.queryByText('disc.mosaic.sendFailed')).toBeNull());
  });

  it('sends a note on its own channel without the dispatch outbox, and restores it on refusal', async () => {
    const settled = settledEvents();
    mocks.send.mockImplementationOnce(async (...args: unknown[]) => { (args[4] as (e: string) => void)('locked'); });
    const toast = vi.fn();
    render(<DiscussionMosaicComposer discussionId="a" title="Alpha" toast={toast} />);
    await screen.findByTestId('chat-input');
    fireEvent.click(screen.getByText('note'));
    await waitFor(() => expect(settled).toHaveBeenCalledWith({ discussionId: 'a', message: 'a note', settlement: 'refused' }));
    expect(mocks.send.mock.calls[0][1]).toMatchObject({ channel: 'note', defer_dispatch: true });
    expect(localStorage.getItem('kronn:message-outbox:a')).toBeNull();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining('locked'), 'error');
  });
});
