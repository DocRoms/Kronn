// DiscussionSidebar — re-pair a `needs_repair` contact (KT-1650).
//
// A contact that predates pairing secrets gets a "Re-pair" action: it asks the
// backend for a pairing code issued for THAT contact and shows it. Paired
// contacts do not get the action.

import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});
import { render, fireEvent, act } from '@testing-library/react';
import { DiscussionSidebar } from '../DiscussionSidebar';
import type { Contact } from '../../types/generated';

const noop = () => {};

const contact = (id: string, pseudo: string, status: string): Contact => ({
  id,
  pseudo,
  avatar_email: null,
  kronn_url: 'http://100.94.142.6:3140',
  invite_code: `kronn:${pseudo}@100.94.142.6:3140`,
  status,
  created_at: '2026-06-29T00:00:00Z',
  updated_at: '2026-06-29T00:00:00Z',
});

const baseProps = {
  discussions: [],
  projects: [],
  activeId: null,
  sendingMap: {},
  lastSeenMsgCount: {},
  contacts: [contact('c-old', 'Old', 'needs_repair'), contact('c-ok', 'Ok', 'accepted')],
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

describe('DiscussionSidebar — re-pair a contact', () => {
  beforeEach(() => vi.clearAllMocks());

  it('asks for a pairing code issued for that contact and shows it', async () => {
    const code = `kronn:me@10.0.0.5:3140#${'ab'.repeat(32)}`;
    const onContactRepair = vi.fn().mockResolvedValue(code);
    const { getByTestId, queryByTestId } = render(
      <DiscussionSidebar {...baseProps} onContactRepair={onContactRepair} />,
    );

    expect(queryByTestId('contact-repair-c-ok')).toBeNull();
    expect(queryByTestId('contact-repair-code-c-old')).toBeNull();
    await act(async () => {
      fireEvent.click(getByTestId('contact-repair-c-old'));
    });

    expect(onContactRepair).toHaveBeenCalledWith('c-old');
    expect(getByTestId('contact-repair-code-c-old')).toHaveTextContent(code);
  });

  it('reports a refusal and shows no code', async () => {
    const onContactRepair = vi.fn().mockRejectedValue(new Error('cannot be re-paired'));
    const { getByTestId, queryByTestId } = render(
      <DiscussionSidebar {...baseProps} onContactRepair={onContactRepair} />,
    );

    await act(async () => {
      fireEvent.click(getByTestId('contact-repair-c-old'));
    });

    expect(queryByTestId('contact-repair-code-c-old')).toBeNull();
    expect(baseProps.toast).toHaveBeenCalledWith(
      'contacts.repairError: cannot be re-paired',
      'error',
    );
  });

  it('hides the action when the page offers no re-pair', () => {
    const { queryByTestId } = render(<DiscussionSidebar {...baseProps} />);
    expect(queryByTestId('contact-repair-c-old')).toBeNull();
  });
});
