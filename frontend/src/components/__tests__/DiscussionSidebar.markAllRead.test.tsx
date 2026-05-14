// DiscussionSidebar — "Mark all as read" button (0.8.3 #277).
//
// Historical users accumulate a fictitious unread count because
// `lastSeenMsgCount` is only seeded when a discussion is explicitly
// opened. Archived discs + batch children rarely get opened → their
// full `message_count` shows up as unread, sometimes in the hundreds.
//
// The fix surfaces a "Mark all as read" icon button in the sidebar
// header that wipes the backlog by seeding `lastSeenMsgCount[d.id] =
// d.message_count` for EVERY discussion at once. The tests below
// verify:
//   1. Button visible only when (a) handler wired AND (b) unread > 0
//   2. Button click invokes the handler exactly once
//   3. Button hidden once everything is seen (no false bait)
//   4. Button hidden when the parent didn't wire the handler at all
//      (defensive — older callers compile fine without it)

import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent, screen } from '@testing-library/react';
import { DiscussionSidebar } from '../DiscussionSidebar';
import type { Discussion } from '../../types/generated';

const noop = () => {};

const baseProps = {
  projects: [],
  activeId: null,
  sendingMap: {},
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
  t: (key: string, ...args: (string | number)[]) =>
    args.length > 0 ? `${key}:${args.join(',')}` : key,
  collapsedGroups: new Set<string>(),
  onToggleGroup: noop,
};

const mkDisc = (id: string, msgCount: number, archived = false): Discussion => ({
  id,
  project_id: null,
  title: `Discussion ${id}`,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: msgCount,
  archived,
  pinned: false,
  workspace_mode: 'Direct',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
});

describe('DiscussionSidebar — Mark all as read (0.8.3 #277)', () => {
  it('renders the button when there is unread + handler wired', () => {
    // 100 messages, none seen → 100 unread → button must render.
    const discs = [mkDisc('d1', 100, false)];
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discs}
        lastSeenMsgCount={{}}
        onMarkAllRead={vi.fn()}
      />
    );
    expect(screen.getByLabelText('disc.markAllRead')).toBeInTheDocument();
  });

  it('button title carries the total unread count (UI affordance)', () => {
    // Tooltip must surface the count so the user knows what they'd
    // clear before clicking. 50 + 30 = 80 across two discs.
    const discs = [mkDisc('d1', 50), mkDisc('d2', 30)];
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discs}
        lastSeenMsgCount={{}}
        onMarkAllRead={vi.fn()}
      />
    );
    const btn = screen.getByLabelText('disc.markAllRead');
    // Tooltip uses `disc.markAllReadTooltip` with the count param.
    expect(btn.getAttribute('title')).toContain('80');
  });

  it('click invokes onMarkAllRead exactly once', () => {
    const onMarkAllRead = vi.fn();
    const discs = [mkDisc('d1', 100)];
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discs}
        lastSeenMsgCount={{}}
        onMarkAllRead={onMarkAllRead}
      />
    );
    fireEvent.click(screen.getByLabelText('disc.markAllRead'));
    expect(onMarkAllRead).toHaveBeenCalledTimes(1);
  });

  it('hidden when everything is already seen', () => {
    // 50 messages, 50 seen → 0 unread → button must NOT render
    // (otherwise it's clutter on an already-clean inbox).
    const discs = [mkDisc('d1', 50)];
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discs}
        lastSeenMsgCount={{ d1: 50 }}
        onMarkAllRead={vi.fn()}
      />
    );
    expect(screen.queryByLabelText('disc.markAllRead')).not.toBeInTheDocument();
  });

  it('hidden when onMarkAllRead handler is not wired (backwards compat)', () => {
    // Legacy callers / tests that pre-date this feature must compile
    // and render without breakage — the button simply hides.
    const discs = [mkDisc('d1', 100)];
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discs}
        lastSeenMsgCount={{}}
        // onMarkAllRead INTENTIONALLY OMITTED
      />
    );
    expect(screen.queryByLabelText('disc.markAllRead')).not.toBeInTheDocument();
  });

  it('counts archived discussions too — they are the worst offenders for backlog', () => {
    // The bug's main vector: archives are never opened individually,
    // so their entire message_count piles up in the global counter.
    // The button must surface even when ALL unread sits in archives.
    const discs = [mkDisc('d1', 0), mkDisc('archived', 559, true)];
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discs}
        lastSeenMsgCount={{ d1: 0 }}
        onMarkAllRead={vi.fn()}
      />
    );
    const btn = screen.getByLabelText('disc.markAllRead');
    expect(btn.getAttribute('title')).toContain('559');
  });

  it('counts the active discussion too — backlog clearing is unconditional', () => {
    // Unlike `unseenByGroup` (which skips the active disc since the
    // user is currently reading it), `totalUnseenAll` includes it.
    // Otherwise opening one disc would hide the bulk-clear button
    // even though every OTHER disc is still unread, which is wrong.
    const discs = [mkDisc('active', 200), mkDisc('other', 100)];
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discs}
        activeId="active"
        lastSeenMsgCount={{}}
        onMarkAllRead={vi.fn()}
      />
    );
    const btn = screen.getByLabelText('disc.markAllRead');
    expect(btn.getAttribute('title')).toContain('300');
  });

  it('uses Math.max(messages.length, message_count) for the count', () => {
    // 0.8.3 guard: a disc whose list-endpoint shape says
    // `message_count = 75` but whose local `messages` array is empty
    // (the normal list shape) MUST count for 75, not 0. Without
    // Math.max, the unread total for fresh list-state discs would
    // be undercounted and the button would disappear prematurely.
    const disc: Discussion = {
      ...mkDisc('d1', 0),
      messages: [],
      message_count: 75,
    };
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={[disc]}
        lastSeenMsgCount={{}}
        onMarkAllRead={vi.fn()}
      />
    );
    const btn = screen.getByLabelText('disc.markAllRead');
    expect(btn.getAttribute('title')).toContain('75');
  });
});
