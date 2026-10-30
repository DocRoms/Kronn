/**
 * KT-1490 — a Page action whose Quick Exec an agent saved inactive says so on
 * the Page's own card, with the human-only Activer button, instead of failing
 * on launch. The card is the one a rendered Page opens for a clicked row.
 */
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePageAction, QuickExec } from '../../types/generated';

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  pendingApprovals: vi.fn(),
  activate: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  discussionActions: { get: vi.fn(), cancel: vi.fn(), launch: vi.fn() },
  pages: { getAction: vi.fn(), cancelAction: vi.fn(), launchAction: vi.fn() },
  quickExecs: { list: mocks.list, pendingApprovals: mocks.pendingApprovals, activate: mocks.activate },
  workflows: { get: vi.fn(), readiness: vi.fn() },
  agents: { detect: vi.fn() },
}));
vi.mock('../AgentSwitchPicker', () => ({ AgentSwitchPicker: () => null }));
vi.mock('../ModelCatalogPicker', () => ({ ModelCatalogPicker: () => null }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) => args.length > 0 ? `${key}:${args.join('/')}` : key,
  }),
}));
vi.mock('../RunStatusCard', () => ({ RunStatusCard: () => null }));

import { LivePageActionCard } from '../LivePageActionCard';

const purge = {
  id: 'qe-1', name: 'Purge CDN', icon: '⌘', description: '', project_id: null,
  command: 'fastly', args: ['purge', '--key', '{{surrogate_key}}'], timeout_secs: 60,
  output_format: 'text', variables: [], pinned: false, agent_written: true,
  created_at: '2026-10-28T10:00:00Z', updated_at: '2026-10-28T10:00:00Z',
} as unknown as QuickExec;

const pageAction = {
  id: 'page-action:page-1:purge', live_page_id: 'page-1', live_page_revision_id: 'rev-1',
  action_ref: 'purge', kind: 'quick_exec', target_id: 'qe-1', target_name: 'Purge CDN',
  project_id: null, project_name: null, state: 'proposed', values: [],
  shared_run_id: null, result_discussion_id: null, deep_link: null, diagnostic: null,
  launched_at: null, finished_at: null,
  created_at: '2026-10-28T10:00:00Z', updated_at: '2026-10-28T10:00:00Z',
  stale_source: false, binding_key: null,
} as unknown as LivePageAction;

beforeEach(() => {
  Object.values(mocks).forEach(mock => mock.mockReset());
  mocks.list.mockResolvedValue([purge]);
  mocks.activate.mockResolvedValue({ ...purge, unmodelled_args_approved: true });
});

describe('a Page action on an inactive Quick Exec', () => {
  it('shows « en attente d\'activation » with the activate button, and activates the line read', async () => {
    mocks.pendingApprovals.mockResolvedValue([{ id: 'qe-1', program: 'fastly', message: '', refused: false }]);
    render(<LivePageActionCard action={pageAction} onChanged={vi.fn()} onOpenDiscussion={vi.fn()} />);

    expect(await screen.findByText('qeActivation.pending:Purge CDN')).toBeDefined();
    mocks.pendingApprovals.mockResolvedValue([]);
    fireEvent.click(screen.getByText('qeActivation.activateNamed:Purge CDN'));
    await waitFor(() => expect(mocks.activate).toHaveBeenCalledWith('qe-1', {
      command: 'fastly',
      args: ['purge', '--key', '{{surrogate_key}}'],
    }));
    // Activated: the card asks again and, nothing waiting any more, drops the notice.
    await waitFor(() => expect(mocks.pendingApprovals).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByTestId('qe-pending-activation')).toBeNull());
  });

  it('shows no notice for an active Quick Exec', async () => {
    mocks.pendingApprovals.mockResolvedValue([]);
    render(<LivePageActionCard action={pageAction} onChanged={vi.fn()} onOpenDiscussion={vi.fn()} />);
    await waitFor(() => expect(mocks.pendingApprovals).toHaveBeenCalled());
    expect(screen.queryByTestId('qe-pending-activation')).toBeNull();
  });
});
