// DiscussionSidebar — unified "join by code" field.
//
// A single input resolves a kr-join token via the parent-supplied
// `onJoinByCode` (which calls POST /discussions/peer-join; the backend
// resolves local OR cross-instance). While it resolves we show a
// "résolution en cours…" state and disable the submit; on failure we toast
// the backend message.

import { describe, it, expect, vi, beforeEach } from 'vitest';

// The sidebar fires a real `projectsApi.discSources()` on mount — unmocked,
// happy-dom performs an actual fetch (ECONNREFUSED in CI) whose late catch
// races the worker teardown (EnvironmentTeardownError). Same shared mock as
// the grouping/sourceBadge suites.
vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});
import { render, fireEvent, act } from '@testing-library/react';
import { DiscussionSidebar } from '../DiscussionSidebar';

const noop = () => {};

const baseProps = {
  discussions: [],
  projects: [],
  activeId: null,
  sendingMap: {},
  lastSeenMsgCount: {},
  contacts: [],
  contactsOnline: {},
  wsConnected: true,
  isMobile: false,
  onSelect: noop,
  onArchive: noop,
  onUnarchive: noop,
  onDelete: noop,
  onTogglePin: noop,
  onNewDiscussion: noop,
  onClose: noop,
  onContactAdd: vi.fn().mockResolvedValue(undefined),
  onContactDelete: vi.fn().mockResolvedValue(undefined),
  toast: vi.fn(),
  t: (key: string) => key,
  collapsedGroups: new Set<string>(),
  onToggleGroup: noop,
};

const openJoinAndType = (code: string) => {
  const toggle = document.querySelector('button[title="contacts.joinByCode"]') as HTMLButtonElement;
  expect(toggle).not.toBeNull();
  act(() => { fireEvent.click(toggle); });
  const input = document.querySelector('.disc-contact-add-input') as HTMLInputElement;
  expect(input).not.toBeNull();
  act(() => { fireEvent.change(input, { target: { value: code } }); });
  return document.querySelector('.disc-contact-add-submit') as HTMLButtonElement;
};

describe('DiscussionSidebar — join by code', () => {
  beforeEach(() => vi.clearAllMocks());

  it('calls onJoinByCode with the pasted token and shows a resolving state', async () => {
    let resolveJoin: (() => void) | undefined;
    const onJoinByCode = vi.fn(() => new Promise<void>(r => { resolveJoin = r; }));
    render(<DiscussionSidebar {...baseProps} onJoinByCode={onJoinByCode} />);

    const submit = openJoinAndType('kr-join-abc123');
    act(() => { fireEvent.click(submit); });

    expect(onJoinByCode).toHaveBeenCalledTimes(1);
    expect(onJoinByCode).toHaveBeenCalledWith('kr-join-abc123');

    // While the (cross-instance) resolution is in flight: resolving label + disabled.
    expect(submit.textContent).toContain('contacts.joinResolving');
    expect(submit.disabled).toBe(true);

    await act(async () => { resolveJoin?.(); });
  });

  it('does not double-fire while a join is already resolving', async () => {
    let resolveJoin: (() => void) | undefined;
    const onJoinByCode = vi.fn(() => new Promise<void>(r => { resolveJoin = r; }));
    render(<DiscussionSidebar {...baseProps} onJoinByCode={onJoinByCode} />);

    const submit = openJoinAndType('kr-join-xyz');
    await act(async () => {
      fireEvent.click(submit);
      fireEvent.click(submit);
    });
    expect(onJoinByCode).toHaveBeenCalledTimes(1);
    await act(async () => { resolveJoin?.(); });
  });

  it('toasts the backend error message when the join fails', async () => {
    const toast = vi.fn();
    const onJoinByCode = vi.fn().mockRejectedValue(new Error('invite token expired'));
    render(<DiscussionSidebar {...baseProps} toast={toast} onJoinByCode={onJoinByCode} />);

    const submit = openJoinAndType('kr-join-dead');
    await act(async () => { fireEvent.click(submit); });

    expect(toast).toHaveBeenCalledWith('invite token expired', 'error');
  });

  it('hides the join button entirely when onJoinByCode is not provided', () => {
    render(<DiscussionSidebar {...baseProps} />);
    expect(document.querySelector('button[title="contacts.joinByCode"]')).toBeNull();
  });
});
