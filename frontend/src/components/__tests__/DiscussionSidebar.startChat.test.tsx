// DiscussionSidebar — click a contact to start a 1:1 chat.
//
// Clicking a contact row fires onStartChat(contact); clicking the row's delete
// (X) button must NOT also start a chat (stopPropagation).

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
import type { Contact } from '../../types/generated';

const noop = () => {};

const contact: Contact = {
  id: 'c-romu',
  pseudo: 'Romu',
  avatar_email: null,
  kronn_url: 'http://100.94.142.6:3140',
  invite_code: 'kronn:Romu@100.94.142.6:3140',
  status: 'accepted',
  created_at: '2026-06-29T00:00:00Z',
  updated_at: '2026-06-29T00:00:00Z',
};

const baseProps = {
  discussions: [],
  projects: [],
  activeId: null,
  sendingMap: {},
  lastSeenMsgCount: {},
  contacts: [contact],
  contactsOnline: { 'c-romu': true },
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

describe('DiscussionSidebar — click contact to start chat', () => {
  beforeEach(() => vi.clearAllMocks());

  it('exposes the WebSocket connection dot as a named status', () => {
    const { getByRole } = render(<DiscussionSidebar {...baseProps} />);

    expect(getByRole('status', { name: 'contacts.wsConnected' })).toBeInTheDocument();
  });

  it('collapses contacts through the persisted sidebar group state', () => {
    const onToggleGroup = vi.fn();
    render(
      <DiscussionSidebar
        {...baseProps}
        collapsedGroups={new Set(['__contacts__'])}
        onToggleGroup={onToggleGroup}
      />,
    );

    const toggle = document.querySelector('.disc-contacts-toggle') as HTMLButtonElement;
    expect(toggle).not.toBeNull();
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(document.querySelector('.disc-contact-row')).toBeNull();

    act(() => { fireEvent.click(toggle); });
    expect(onToggleGroup).toHaveBeenCalledWith('__contacts__');
  });

  it('clicking the contact identity fires onStartChat with that contact', () => {
    const onStartChat = vi.fn();
    render(<DiscussionSidebar {...baseProps} onStartChat={onStartChat} />);
    const open = document.querySelector('.disc-contact-open') as HTMLElement;
    expect(open).not.toBeNull();
    // A real button, so it is focusable and Enter/Space work — the previous
    // `div role="button"` announced a button it could not deliver.
    expect(open.tagName).toBe('BUTTON');
    act(() => { fireEvent.click(open); });
    expect(onStartChat).toHaveBeenCalledTimes(1);
    expect(onStartChat).toHaveBeenCalledWith(expect.objectContaining({ id: 'c-romu', pseudo: 'Romu' }));
  });

  it('the delete button is not nested inside the identity button', () => {
    render(<DiscussionSidebar {...baseProps} onStartChat={vi.fn()} />);
    const del = document.querySelector('.disc-contact-del-btn') as HTMLElement;
    expect(del.closest('.disc-contact-open')).toBeNull();
  });

  it('clicking the delete (X) button does NOT start a chat', () => {
    const onStartChat = vi.fn();
    const onContactDelete = vi.fn().mockResolvedValue(undefined);
    render(<DiscussionSidebar {...baseProps} onStartChat={onStartChat} onContactDelete={onContactDelete} />);
    const del = document.querySelector('.disc-contact-del-btn') as HTMLElement;
    expect(del).not.toBeNull();
    act(() => { fireEvent.click(del); });
    expect(onContactDelete).toHaveBeenCalledTimes(1);
    expect(onStartChat).not.toHaveBeenCalled();
  });

  it('rows are not clickable when onStartChat is not provided', () => {
    render(<DiscussionSidebar {...baseProps} />);
    const row = document.querySelector('.disc-contact-row') as HTMLElement;
    expect(row.getAttribute('role')).toBeNull();
    // No button at all rather than an inert one: nothing to focus, nothing to
    // announce as actionable.
    expect(document.querySelector('.disc-contact-open')!.tagName).toBe('SPAN');
  });
});
