import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { Discussion } from '../../types/generated';

const mocks = vi.hoisted(() => ({
  exportDiscussion: vi.fn(),
  triggerDownload: vi.fn(),
}));

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock({
    discussions: {
      exportDiscussion: mocks.exportDiscussion,
    },
  });
});
vi.mock('../../lib/downloadBlob', () => ({
  triggerDownload: mocks.triggerDownload,
}));

import { ChatHeader } from '../ChatHeader';
import { DiscussionSidebar } from '../DiscussionSidebar';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;
const noop = () => {};

const discussion: Discussion = {
  id: 'disc-portable',
  project_id: null,
  title: 'Portable discussion',
  agent: 'Codex',
  language: 'fr',
  participants: ['Codex'],
  messages: [],
  message_count: 0,
  non_system_message_count: 0,
  tier: 'default',
  summary_strategy: 'Off',
  introspection_call_count: 0,
  archived: false,
  pinned: false,
  pin_first_message: false,
  workspace_mode: 'Direct',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  awaiting_agent: false,
};

describe('discussion portability UI', () => {
  it('downloads the complete discussion bundle from the header', async () => {
    const blob = new Blob(['{}'], { type: 'application/json' });
    mocks.exportDiscussion.mockResolvedValue({
      filename: 'portable.kronn-discussion.json',
      blob,
    });
    const toast = vi.fn();
    render(
      <ChatHeader
        discussion={discussion}
        projects={[]}
        agents={[]}
        showGitPanel={false}
        isMobile={false}
        sending={false}
        pendingFilesCount={0}
        onRequestTestMode={noop}
        onToggleGitPanel={noop}
        onToggleSettingsPanel={noop}
        onToggleSidebar={noop}
        onDelete={noop}
        onDiscussionUpdated={noop}
        onAgentSwitch={noop}
        toast={toast}
        t={t}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'disc.portability.export' }));
    await waitFor(() =>
      expect(mocks.exportDiscussion).toHaveBeenCalledWith('disc-portable'),
    );
    expect(mocks.triggerDownload).toHaveBeenCalledWith(
      'portable.kronn-discussion.json',
      blob,
    );
    expect(toast).toHaveBeenCalledWith('disc.portability.exportDone', 'success');
  });

  it('passes the selected JSON bundle to the sidebar import action once', async () => {
    const onImportDiscussion = vi.fn().mockResolvedValue(undefined);
    const { container } = render(
      <DiscussionSidebar
        discussions={[discussion]}
        projects={[]}
        activeId={null}
        sendingMap={{}}
        lastSeenMsgCount={{}}
        contacts={[]}
        contactsOnline={{}}
        wsConnected
        isMobile={false}
        onSelect={noop}
        onArchive={noop}
        onUnarchive={noop}
        onDelete={noop}
        onTogglePin={noop}
        onNewDiscussion={noop}
        onImportDiscussion={onImportDiscussion}
        onClose={noop}
        onContactAdd={vi.fn().mockResolvedValue(undefined)}
        onContactDelete={vi.fn().mockResolvedValue(undefined)}
        toast={vi.fn()}
        t={t}
        collapsedGroups={new Set()}
        onToggleGroup={noop}
      />,
    );
    const file = new File(['{"kind":"kronn.discussion"}'], 'disc.json', {
      type: 'application/json',
    });
    const input = container.querySelector<HTMLInputElement>('input[type="file"]');
    fireEvent.change(input!, { target: { files: [file] } });

    await waitFor(() => expect(onImportDiscussion).toHaveBeenCalledWith(file));
    expect(onImportDiscussion).toHaveBeenCalledTimes(1);
  });
});
