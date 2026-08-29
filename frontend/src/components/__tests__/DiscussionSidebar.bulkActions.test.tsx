import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());

import { projects as projectsApi } from '../../lib/api';
import { DiscussionSidebar } from '../DiscussionSidebar';
import type { Discussion } from '../../types/generated';

const makeDiscussion = (id: string): Discussion => ({
  id,
  project_id: null,
  title: `Discussion ${id}`,
  agent: 'Codex',
  language: 'fr',
  participants: ['Codex'],
  messages: [],
  message_count: 0,
  non_system_message_count: 0,
  tier: 'default',
  summary_strategy: 'Auto',
  introspection_call_count: 0,
  archived: false,
  pinned: false,
  pin_first_message: false,
  workspace_mode: 'Direct',
  created_at: '2026-07-25T10:00:00Z',
  updated_at: '2026-07-25T10:00:00Z',
  awaiting_agent: false,
});

const makeProps = () => ({
  discussions: [makeDiscussion('disc-a'), makeDiscussion('disc-b')],
  projects: [],
  activeId: null,
  sendingMap: {},
  lastSeenMsgCount: {},
  contacts: [],
  contactsOnline: {},
  wsConnected: true,
  isMobile: false,
  onSelect: vi.fn(),
  onArchive: vi.fn(),
  onUnarchive: vi.fn(),
  onDelete: vi.fn(),
  onBulkArchive: vi.fn().mockResolvedValue(undefined),
  onBulkDelete: vi.fn().mockResolvedValue(undefined),
  onCompareSelected: vi.fn().mockResolvedValue(undefined),
  onTogglePin: vi.fn(),
  onNewDiscussion: vi.fn(),
  onClose: vi.fn(),
  onContactAdd: vi.fn().mockResolvedValue(undefined),
  onContactDelete: vi.fn().mockResolvedValue(undefined),
  toast: vi.fn(),
  t: (key: string, ...args: (string | number)[]) =>
    args.length > 0 ? `${key}:${args.join(',')}` : key,
  collapsedGroups: new Set<string>(),
  onToggleGroup: vi.fn(),
});

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(projectsApi.discSources).mockResolvedValue([]);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('DiscussionSidebar — bulk selection', () => {
  it('opens a free comparison only after at least two discussions are selected', async () => {
    const props = makeProps();
    render(<DiscussionSidebar {...props} />);

    expect(screen.getByRole('complementary', { name: 'Discussions' })).toHaveClass('collection-shell-sidebar');
    fireEvent.click(screen.getByRole('button', { name: 'disc.sidebar.moreActions' }));
    fireEvent.click(screen.getByRole('button', { name: 'disc.bulk.start' }));
    const compare = screen.getByRole('button', { name: 'disc.bulk.compare' });
    expect(compare).toBeDisabled();

    fireEvent.click(screen.getByRole('checkbox', { name: /Discussion disc-a/ }));
    expect(compare).toBeDisabled();
    fireEvent.click(screen.getByRole('checkbox', { name: /Discussion disc-b/ }));
    expect(compare).toBeEnabled();
    fireEvent.click(compare);

    await waitFor(() => {
      expect(props.onCompareSelected).toHaveBeenCalledWith(['disc-a', 'disc-b']);
    });
    expect(screen.queryByText('disc.bulk.selected:2')).not.toBeInTheDocument();
  });

  it('selects rows without opening them and archives the selection after one confirmation', async () => {
    const props = makeProps();
    const confirmStub = vi.fn(() => true);
    vi.stubGlobal('confirm', confirmStub);
    render(<DiscussionSidebar {...props} />);

    fireEvent.click(screen.getByRole('button', { name: 'disc.sidebar.moreActions' }));
    fireEvent.click(screen.getByRole('button', { name: 'disc.bulk.start' }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Discussion disc-a/ }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Discussion disc-b/ }));

    expect(props.onSelect).not.toHaveBeenCalled();
    expect(screen.getByText('disc.bulk.selected:2')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'disc.bulk.archive' }));

    await waitFor(() => {
      expect(props.onBulkArchive).toHaveBeenCalledWith(['disc-a', 'disc-b']);
    });
    expect(confirmStub).toHaveBeenCalledTimes(1);
    expect(confirmStub).toHaveBeenCalledWith('disc.bulk.confirmArchive:2');
    expect(screen.queryByText('disc.bulk.selected:2')).not.toBeInTheDocument();
  });

  it('keeps the selection when bulk deletion is cancelled', () => {
    const props = makeProps();
    const confirmStub = vi.fn(() => false);
    vi.stubGlobal('confirm', confirmStub);
    render(<DiscussionSidebar {...props} />);

    fireEvent.click(screen.getByRole('button', { name: 'disc.sidebar.moreActions' }));
    fireEvent.click(screen.getByRole('button', { name: 'disc.bulk.start' }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Discussion disc-a/ }));
    fireEvent.click(screen.getByRole('button', { name: 'disc.bulk.delete' }));

    expect(confirmStub).toHaveBeenCalledTimes(1);
    expect(confirmStub).toHaveBeenCalledWith('disc.bulk.confirmDelete:1');
    expect(props.onBulkDelete).not.toHaveBeenCalled();
    expect(screen.getByText('disc.bulk.selected:1')).toBeInTheDocument();
  });

  it('prunes a deleted discussion from the live bulk selection after a refresh', async () => {
    const props = makeProps();
    const confirmStub = vi.fn(() => true);
    vi.stubGlobal('confirm', confirmStub);
    const { rerender } = render(<DiscussionSidebar {...props} />);

    fireEvent.click(screen.getByRole('button', { name: 'disc.sidebar.moreActions' }));
    fireEvent.click(screen.getByRole('button', { name: 'disc.bulk.start' }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Discussion disc-b/ }));
    expect(screen.getByText('disc.bulk.selected:1')).toBeInTheDocument();

    await act(async () => {
      rerender(<DiscussionSidebar {...props} discussions={[makeDiscussion('disc-a')]} />);
    });

    await waitFor(() => expect(screen.getByText('disc.bulk.selected:0')).toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'disc.bulk.archive' })).toBeDisabled();

    fireEvent.click(screen.getByRole('checkbox', { name: /Discussion disc-a/ }));
    fireEvent.click(screen.getByRole('button', { name: 'disc.bulk.archive' }));
    await waitFor(() => expect(props.onBulkArchive).toHaveBeenCalledWith(['disc-a']));
    expect(confirmStub).toHaveBeenCalledWith('disc.bulk.confirmArchive:1');
  });

  it('moves focus into the header disclosure and restores it on Escape', async () => {
    const props = makeProps();
    render(<DiscussionSidebar {...props} />);

    const trigger = screen.getByRole('button', { name: 'disc.sidebar.moreActions' });
    fireEvent.click(trigger);

    const action = screen.getByRole('button', { name: 'disc.bulk.start' });
    await waitFor(() => expect(action).toHaveFocus());
    expect(screen.getByRole('group', { name: 'disc.sidebar.moreActions' })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.queryByRole('group', { name: 'disc.sidebar.moreActions' })).toBeNull();
  });
});
