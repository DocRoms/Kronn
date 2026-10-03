// KT-983 — selecting a discussion must re-render only the two cards whose
// active state changed. Every card re-rendered per click once a callback or
// badge prop was recreated, which cost ~200 ms per click on a large sidebar.

import { describe, it, expect, vi } from 'vitest';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});
const relativeTimeCalls = vi.hoisted(() => ({ ids: [] as string[] }));
vi.mock('../../lib/relativeTime', () => ({
  formatRelativeTime: (iso: string) => {
    relativeTimeCalls.ids.push(iso);
    return iso;
  },
}));
import { act, render } from '@testing-library/react';
import { DiscussionSidebar } from '../DiscussionSidebar';
import type { Discussion } from '../../types/generated';

const noop = () => {};

const baseProps = {
  projects: [],
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
  lastSeenMsgCount: {},
};

const mkDisc = (index: number): Discussion => ({
  id: `d${index}`,
  project_id: null,
  title: `Discussion ${index}`,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: 3, non_system_message_count: 3, tier: 'default' as const, summary_strategy: 'OnDemand' as const, introspection_call_count: 0,
  archived: false,
  pinned: false, pin_first_message: false,
  workspace_mode: 'Direct',
  created_at: '2026-01-01T00:00:00Z',
  // One distinct timestamp per card so each render is attributable.
  updated_at: `2026-01-${String(index + 1).padStart(2, '0')}T00:00:00Z`,
  awaiting_agent: false,
});

describe('DiscussionSidebar — render cost of a selection (KT-983)', () => {
  it('re-renders only the previously and newly active cards', async () => {
    // Under the smart-section threshold, so each discussion has one card.
    const discussions = Array.from({ length: 12 }, (_, index) => mkDisc(index));
    const { rerender } = render(
      <DiscussionSidebar {...baseProps} discussions={discussions} activeId="d1" />,
    );
    // Let the mount-time fetches settle so their updates are not counted.
    await act(async () => {});
    expect(relativeTimeCalls.ids.length).toBeGreaterThanOrEqual(12);

    relativeTimeCalls.ids = [];
    rerender(<DiscussionSidebar {...baseProps} discussions={discussions} activeId="d2" />);

    expect([...new Set(relativeTimeCalls.ids)].sort()).toEqual([
      discussions[1].updated_at,
      discussions[2].updated_at,
    ]);
  });
});
