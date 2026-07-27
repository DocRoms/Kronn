// DiscussionSidebar — 0.8.4 (#294) cross-agent source badge + filter.
//
// Validates:
//   1. Badge renders for discs whose id appears in `discSources()`
//   2. Badge omitted for un-bound discs
//   3. Diverged binding flips the badge to a warning style + tooltip
//   4. Filter dropdown is hidden when no source bindings exist
//   5. Filter dropdown lists each distinct source_agent
//   6. Selecting a filter narrows the sidebar to matching discs only
//
// The fetch goes through `projectsApi.discSources`, so we mock the
// shared api module via the apiMock helper.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import { DiscussionSidebar } from '../DiscussionSidebar';
import type { Discussion } from '../../types/generated';

const noop = () => {};

const baseProps = {
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
  t: (key: string, ...args: (string | number)[]) =>
    args.length > 0 ? `${key}:${args.join(',')}` : key,
  collapsedGroups: new Set<string>(),
  onToggleGroup: noop,
};

const mkDisc = (id: string, title: string): Discussion => ({
  id,
  project_id: null,
  title,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: 0, non_system_message_count: 0, tier: "default" as const, summary_strategy: "Auto" as const, introspection_call_count: 0,
  archived: false,
  pinned: false, pin_first_message: false,
  workspace_mode: 'Direct',
  created_at: '2026-05-15T10:00:00Z',
  updated_at: '2026-05-15T10:00:00Z',
  awaiting_agent: false,
});

describe('DiscussionSidebar — source badge (0.8.4 #294)', () => {
  it('renders the badge for bound discs and omits it for un-bound ones', async () => {
    (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      { disc_id: 'd-bound',  source_agent: 'ClaudeCode', source_session_id: 'sess-1' },
    ]);

    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={[mkDisc('d-bound', 'Imported thread'), mkDisc('d-free', 'Native thread')]}
      />
    );

    await waitFor(() => {
      const badges = screen.queryAllByTestId('disc-source-badge');
      expect(badges).toHaveLength(1);
      expect(badges[0].textContent).toContain('ClaudeCode');
    });
  });

  it('hides the filter dropdown when there are no bindings', async () => {
    (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]);
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={[mkDisc('d-free', 'Native')]}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());
    expect(screen.queryByTestId('disc-source-filter')).toBeNull();
  });

  it('shows the filter dropdown with one option per distinct agent', async () => {
    (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      { disc_id: 'd-cc',  source_agent: 'ClaudeCode', source_session_id: 's1' },
      { disc_id: 'd-cur', source_agent: 'Cursor',     source_session_id: 's2' },
      // Same agent → still one option (deduped).
      { disc_id: 'd-cc2', source_agent: 'ClaudeCode', source_session_id: 's3' },
    ]);
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={[mkDisc('d-cc', 'a'), mkDisc('d-cur', 'b'), mkDisc('d-cc2', 'c')]}
      />
    );

    const select = await screen.findByTestId('disc-source-filter') as HTMLSelectElement;
    const options = Array.from(select.options).map(o => o.value);
    // Default "" option + 2 agents (deduped).
    expect(options).toEqual(['', 'ClaudeCode', 'Cursor']);
  });

  it('selecting a filter narrows the sidebar to matching discs', async () => {
    (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      { disc_id: 'd-cc',  source_agent: 'ClaudeCode', source_session_id: 's1' },
      { disc_id: 'd-cur', source_agent: 'Cursor',     source_session_id: 's2' },
    ]);

    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={[
          mkDisc('d-cc',   'Imported from CC'),
          mkDisc('d-cur',  'Imported from Cursor'),
          mkDisc('d-free', 'Native thread (no binding)'),
        ]}
      />
    );

    // All three render initially (one filter = all sources).
    await waitFor(() => {
      expect(screen.getByText('Imported from CC')).toBeInTheDocument();
      expect(screen.getByText('Imported from Cursor')).toBeInTheDocument();
      expect(screen.getByText('Native thread (no binding)')).toBeInTheDocument();
    });

    const select = screen.getByTestId('disc-source-filter') as HTMLSelectElement;
    fireEvent.change(select, { target: { value: 'ClaudeCode' } });

    // After filtering: only CC-bound disc remains, the others are filtered out.
    expect(screen.getByText('Imported from CC')).toBeInTheDocument();
    expect(screen.queryByText('Imported from Cursor')).toBeNull();
    expect(screen.queryByText('Native thread (no binding)')).toBeNull();
  });

  // KT-74 — a session binding is not an import. The badge must say "bound to",
  // because "imported from" told the user a file had been ingested when Kronn
  // was merely following a live CLI session.
  it('labels a plain binding as bound, never as imported', async () => {
    (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      { disc_id: 'd-bound', source_agent: 'Codex', source_session_id: 's1' },
    ]);
    render(
      <DiscussionSidebar {...baseProps} discussions={[mkDisc('d-bound', 'bound thread')]} />
    );
    const badge = await screen.findByTestId('disc-source-badge');
    expect(badge.getAttribute('title')).toMatch(/disc\.source\.boundHint/);
    expect(badge.getAttribute('title')).not.toMatch(/imported/i);
    expect(badge.textContent).toContain('disc.source.boundBadge');
  });

  // KT-74 — the two provenances are independent. A bound disc is not an
  // import, an import is not a binding, and a disc can carry both.
  describe('portable import provenance', () => {
    it('names the exporter when the bundle carried an identity', async () => {
      (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]);
      (projectsApi.discImports as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
        {
          disc_id: 'd-imported',
          provenance_kind: 'portable_bundle',
          imported_by_pseudo: 'Romu',
          imported_by_avatar_email: 'romu@example.test',
          imported_at: '2026-07-28T10:00:00Z',
        },
      ]);
      render(
        <DiscussionSidebar {...baseProps} discussions={[mkDisc('d-imported', 'from a colleague')]} />
      );
      const badge = await screen.findByTestId('disc-import-badge');
      expect(badge.textContent).toContain('disc.import.byBadge');
      expect(badge.textContent).toContain('Romu');
      // No source binding → no bound badge on the same row.
      expect(screen.queryByTestId('disc-source-badge')).toBeNull();
    });

    it('renders an authorless import rather than inventing a name', async () => {
      (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]);
      (projectsApi.discImports as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
        {
          disc_id: 'd-legacy',
          provenance_kind: 'portable_bundle',
          imported_by_pseudo: null,
          imported_by_avatar_email: null,
          imported_at: '2026-07-28T10:00:00Z',
        },
      ]);
      render(
        <DiscussionSidebar {...baseProps} discussions={[mkDisc('d-legacy', 'legacy bundle')]} />
      );
      const badge = await screen.findByTestId('disc-import-badge');
      expect(badge.textContent).toContain('disc.import.anonymousBadge');
      // Neither a leaked placeholder nor the "imported BY someone" branch.
      expect(badge.textContent).not.toMatch(/undefined|null/i);
      expect(badge.textContent).not.toContain('disc.import.byBadge');
    });

    it('ignores the reserved agent_transcript kind, which is not implemented', async () => {
      (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]);
      (projectsApi.discImports as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
        {
          disc_id: 'd-future',
          provenance_kind: 'agent_transcript',
          imported_by_pseudo: 'Romu',
          imported_by_avatar_email: null,
          imported_at: '2026-07-28T10:00:00Z',
        },
      ]);
      render(
        <DiscussionSidebar {...baseProps} discussions={[mkDisc('d-future', 'future route')]} />
      );
      await waitFor(() => expect(projectsApi.discImports).toHaveBeenCalled());
      expect(screen.queryByTestId('disc-import-badge')).toBeNull();
    });

    it('shows both badges when a disc is bound AND imported', async () => {
      (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
        { disc_id: 'd-both', source_agent: 'Codex', source_session_id: 's1' },
      ]);
      (projectsApi.discImports as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
        {
          disc_id: 'd-both',
          provenance_kind: 'portable_bundle',
          imported_by_pseudo: 'Romu',
          imported_by_avatar_email: null,
          imported_at: '2026-07-28T10:00:00Z',
        },
      ]);
      render(<DiscussionSidebar {...baseProps} discussions={[mkDisc('d-both', 'both')]} />);
      expect(await screen.findByTestId('disc-import-badge')).toBeInTheDocument();
      expect(await screen.findByTestId('disc-source-badge')).toBeInTheDocument();
    });
  });

  // KT-85 — a cross-agent room carries one binding per joined session. The
  // sidebar used to key a single binding per disc, so only one chip appeared.
  it('renders one chip per agent bound to the same discussion', async () => {
    (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      { disc_id: 'd-room', source_agent: 'Codex', source_session_id: 'cli-codex-1' },
      { disc_id: 'd-room', source_agent: 'ClaudeCode', source_session_id: 'cli-claude-1' },
      // A second session of an agent already shown must not double its chip.
      { disc_id: 'd-room', source_agent: 'Codex', source_session_id: 'cli-codex-2' },
    ]);
    render(<DiscussionSidebar {...baseProps} discussions={[mkDisc('d-room', 'cross-agent room')]} />);

    await waitFor(() => {
      expect(screen.queryAllByTestId('disc-source-badge')).toHaveLength(2);
    });
    const labels = screen.getAllByTestId('disc-source-badge').map(b => b.textContent ?? '');
    expect(labels.some(l => l.includes('Codex'))).toBe(true);
    expect(labels.some(l => l.includes('ClaudeCode'))).toBe(true);
  });

  it('diverged binding renders the warning variant', async () => {
    (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      {
        disc_id: 'd-diverged',
        source_agent: 'ClaudeCode',
        source_session_id: 's1',
        diverged_at: '2026-05-15T11:00:00Z',
      },
    ]);
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={[mkDisc('d-diverged', 'edited locally')]}
      />
    );
    const badge = await screen.findByTestId('disc-source-badge');
    // Diverged uses the `divergedHint` i18n key (carries the agent).
    expect(badge.getAttribute('title')).toMatch(/disc\.source\.divergedHint/);
  });
});
