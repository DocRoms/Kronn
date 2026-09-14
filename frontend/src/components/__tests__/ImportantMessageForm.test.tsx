import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ImportantMessageForm } from '../ImportantMessageForm';
import { discardImportantDraft, importantMessageFence, takeScalars } from '../../lib/importantMessageDraft';
import type { ImportantPublishOutcome, ImportantSubmission } from '../../lib/submitImportantMessage';

const t = (key: string) => key;
const props = () => ({
  discussionId: 'room-a', grant: 'test-grant', onGrantChange: vi.fn(),
  onPublish: vi.fn<(submission: ImportantSubmission) => Promise<ImportantPublishOutcome>>().mockResolvedValue('confirmed'),
  onOpenSettings: vi.fn(), tasks: [] as Array<{ reference: string; title: string }>, t,
});
const open = async (text = 'A clear update') => {
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'disc.important.openForm' }));
  await user.type(screen.getByLabelText('disc.important.contentLabel'), text);
  return user;
};
const publishButton = () => screen.getByRole('button', { name: 'disc.important.publish' });
const deferred = () => {
  let resolve!: (outcome: ImportantPublishOutcome) => void;
  const promise = new Promise<ImportantPublishOutcome>(done => { resolve = done; });
  return { promise, resolve };
};
afterEach(() => {
  cleanup();
  discardImportantDraft('room-a');
  discardImportantDraft('room-b');
});

describe('ImportantMessageForm', () => {
  it('guards two clicks in the same synchronous render before React disables the button', async () => {
    const input = props();
    input.onPublish.mockReturnValue(deferred().promise);
    render(<ImportantMessageForm {...input} />);
    await open('Exactly once');
    const button = publishButton();
    act(() => { button.click(); button.click(); });
    expect(input.onPublish).toHaveBeenCalledOnce();
  });

  it('keeps an unsent draft when the user closes and reopens the form', async () => {
    render(<ImportantMessageForm {...props()} />);
    const user = await open('Keep this thought');
    await user.click(screen.getByRole('button', { name: 'common.cancel' }));
    expect(screen.getByRole('button', { name: 'disc.important.openForm' })).toHaveFocus();
    await user.click(screen.getByRole('button', { name: 'disc.important.openForm' }));
    expect(screen.getByLabelText('disc.important.contentLabel')).toHaveValue('Keep this thought');
  });

  it('creates information with an optional existing task, then gives the next publication a fresh identity and body', async () => {
    const input = props();
    input.tasks = [{ reference: 'KT-643', title: 'Important messages' }];
    render(<ImportantMessageForm {...input} />);
    const user = await open();
    await user.selectOptions(screen.getByLabelText('disc.important.taskLabel'), 'KT-643');
    await user.click(publishButton());
    const first = input.onPublish.mock.calls[0][0];
    const spec = JSON.parse(first.content.split('\n')[1]);
    expect(spec).toMatchObject({ category: 'information', references: { task_ref: 'KT-643' }, action_required: { required: false } });
    expect(first.content).not.toContain(input.grant);
    expect(first.clientMessageId).toMatch(/^[0-9a-f-]{36}$/);
    await open('Another fact');
    await user.click(publishButton());
    const second = input.onPublish.mock.calls[1][0];
    expect(second.clientMessageId).not.toBe(first.clientMessageId);
    expect(second.dedupKey).not.toBe(first.dedupKey);
    expect(JSON.parse(second.content.split('\n')[1]).highlight).toBe('Another fact');
  });

  it('hides authority by default and requires content and a credential without persisting secrets', async () => {
    const input = props();
    const local = vi.spyOn(Storage.prototype, 'setItem');
    const view = render(<ImportantMessageForm {...input} grant="" />);
    expect(screen.queryByLabelText('disc.important.grantLabel')).toBeNull();
    const user = await open();
    expect(publishButton()).toBeDisabled();
    view.rerender(<ImportantMessageForm {...input} />);
    expect(publishButton()).toBeEnabled();
    await user.click(publishButton());
    expect(local.mock.calls.some(call => call.join('').includes(input.grant))).toBe(false);
    local.mockRestore();
  });

  it('releases its guard after refusal, keeps the text, and permits a successful retry', async () => {
    const input = props();
    input.onPublish.mockResolvedValueOnce('refused');
    render(<ImportantMessageForm {...input} />);
    const user = await open('Still mine');
    await user.click(publishButton());
    expect(screen.getByRole('status')).toHaveTextContent('disc.important.status.refused');
    expect(screen.getByLabelText('disc.important.contentLabel')).toHaveValue('Still mine');
    await user.click(publishButton());
    expect(input.onPublish).toHaveBeenCalledTimes(2);
    expect(input.onPublish.mock.calls[1][0].clientMessageId).toBe(input.onPublish.mock.calls[0][0].clientMessageId);
    expect(screen.queryByLabelText('disc.important.contentLabel')).toBeNull();
  });

  it('aborts preparation without clearing the stable retry draft', async () => {
    const input = props(), waiting = deferred();
    input.onPublish.mockReturnValue(waiting.promise);
    render(<ImportantMessageForm {...input} />);
    const user = await open('Retry me');
    await user.click(publishButton());
    await user.click(screen.getByRole('button', { name: 'disc.important.cancelPreparing' }));
    expect(input.onPublish.mock.calls[0][0].signal.aborted).toBe(true);
    await act(async () => { waiting.resolve('cancelled'); });
    expect(screen.getByRole('status')).toHaveTextContent('disc.important.status.cancelled');
    expect(screen.getByDisplayValue('Retry me')).toBeInTheDocument();
    expect(publishButton()).toBeEnabled();
  });

  it('locks an uncertain attempt and reconciles the same body and UUID on retry', async () => {
    const input = props();
    input.onPublish.mockResolvedValueOnce('uncertain');
    render(<ImportantMessageForm {...input} />);
    const user = await open();
    await user.click(publishButton());
    expect(screen.getByLabelText('disc.important.contentLabel')).toBeDisabled();
    await user.click(publishButton());
    const [first, second] = input.onPublish.mock.calls.map(call => call[0]);
    expect(second).toMatchObject({ content: first.content, clientMessageId: first.clientMessageId, dedupKey: first.dedupKey, reconcileFirst: true });
  });

  it('does not reuse an ordinary text-only row for an explicit new publication', async () => {
    const input = props();
    input.onPublish.mockResolvedValueOnce('text-only');
    render(<ImportantMessageForm {...input} />);
    const user = await open();
    await user.click(publishButton());
    expect(screen.getByRole('status')).toHaveTextContent('disc.important.status.text-only');
    await user.click(publishButton());
    const [first, second] = input.onPublish.mock.calls.map(call => call[0]);
    expect(second.clientMessageId).not.toBe(first.clientMessageId);
    expect(second.dedupKey).toBe(first.dedupKey);
  });

  it('can reconcile an uncertain send even when its linked task has left the plan', async () => {
    const input = props();
    input.tasks = [{ reference: 'KT-643', title: 'Existing task' }];
    input.onPublish.mockResolvedValueOnce('uncertain');
    const view = render(<ImportantMessageForm {...input} />);
    const user = await open();
    await user.selectOptions(screen.getByLabelText('disc.important.taskLabel'), 'KT-643');
    await user.click(publishButton());
    view.rerender(<ImportantMessageForm {...input} tasks={[]} />);
    expect(publishButton()).toBeEnabled();
    await user.click(publishButton());
    const [first, second] = input.onPublish.mock.calls.map(call => call[0]);
    expect(second).toMatchObject({ reconcileFirst: true, clientMessageId: first.clientMessageId, content: first.content });
  });

  it('keeps room drafts separate across navigation and cancels only the abandoned preparation', async () => {
    const input = props(), waiting = deferred();
    input.onPublish.mockReturnValue(waiting.promise);
    const view = render(<ImportantMessageForm key="room-a" {...input} />);
    const user = await open('Draft A');
    await user.click(publishButton());
    view.rerender(<ImportantMessageForm key="room-b" {...input} discussionId="room-b" />);
    expect(input.onPublish.mock.calls[0][0].signal.aborted).toBe(true);
    await open('Draft B');
    await act(async () => { waiting.resolve('cancelled'); });
    expect(screen.getByLabelText('disc.important.contentLabel')).toHaveValue('Draft B');
    view.rerender(<ImportantMessageForm key="room-a" {...input} />);
    expect(screen.getByLabelText('disc.important.contentLabel')).toHaveValue('Draft A');
  });

  it('keeps observing a claimed send after unmount and never enables a second submit in the room', async () => {
    const input = props(), waiting = deferred();
    input.onPublish.mockImplementation(submission => { submission.onSending(); return waiting.promise; });
    const view = render(<ImportantMessageForm {...input} />);
    const user = await open();
    await user.click(publishButton());
    expect(screen.getByRole('button', { name: 'common.cancel' })).toBeDisabled();
    view.unmount();
    expect(input.onPublish.mock.calls[0][0].signal.aborted).toBe(false);
    render(<ImportantMessageForm {...input} />);
    expect(publishButton()).toBeDisabled();
    await act(async () => { waiting.resolve('confirmed'); });
    expect(screen.queryByLabelText('disc.important.contentLabel')).toBeNull();
  });

  it('preserves the draft through Settings and returns keyboard focus after Escape', async () => {
    const input = props();
    const view = render(<ImportantMessageForm {...input} />);
    const user = await open('Settings detour');
    await user.click(screen.getByRole('button', { name: 'disc.important.openSettings' }));
    expect(input.onOpenSettings).toHaveBeenCalledOnce();
    view.unmount();
    render(<ImportantMessageForm {...input} grant="" />);
    expect(screen.getByLabelText('disc.important.contentLabel')).toHaveValue('Settings detour');
    expect(screen.getByLabelText('disc.important.grantLabel')).toHaveValue('');
    screen.getByLabelText('disc.important.contentLabel').focus();
    await user.keyboard('{Escape}');
    expect(screen.getByRole('button', { name: 'disc.important.openForm' })).toHaveFocus();
    expect(screen.queryByLabelText('disc.important.contentLabel')).toBeNull();
  });

  it('requires an explicit choice if the linked task disappears from this discussion plan', async () => {
    const input = props();
    input.tasks = [{ reference: 'KT-643', title: 'Existing task' }];
    const view = render(<ImportantMessageForm {...input} />);
    const user = await open();
    await user.selectOptions(screen.getByLabelText('disc.important.taskLabel'), 'KT-643');
    view.rerender(<ImportantMessageForm {...input} tasks={[]} />);
    expect(screen.getByRole('option', { name: 'KT-643 — disc.important.taskUnavailable' })).toBeDisabled();
    expect(publishButton()).toBeDisabled();
    await user.selectOptions(screen.getByLabelText('disc.important.taskLabel'), '');
    expect(publishButton()).toBeEnabled();
  });

  it('bounds Unicode scalars, replaces lone surrogates and retains embedded fences as JSON text', () => {
    const content = `${'x'.repeat(199)}😊\n\`\`\`kronn-important`;
    expect(takeScalars(content, 200)).toBe(`${'x'.repeat(199)}😊`);
    expect(takeScalars(String.fromCharCode(0xd800) + 'broken' + String.fromCharCode(0xdfff), 100)).toBe('�broken�');
    const spec = JSON.parse(importantMessageFence(content, 'information', '', 'stable', 'Not specified').split('\n')[1]);
    expect(spec.title).toBe(`${'x'.repeat(199)}😊`);
    expect(spec.highlight).toContain('```kronn-important');
    expect(JSON.stringify(spec)).not.toContain('\\ud83d');
    const bounded = JSON.parse(importantMessageFence('😊'.repeat(2600), 'blocking', '', 'bounded', 'Not specified').split('\n')[1]);
    expect(Array.from(bounded.title)).toHaveLength(200);
    expect(Array.from(bounded.highlight)).toHaveLength(500);
    expect(Array.from(bounded.context)).toHaveLength(2000);
    expect(bounded.category).toBe('blocking_alert');
  });
});
