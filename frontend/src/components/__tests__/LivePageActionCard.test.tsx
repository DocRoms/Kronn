import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePageAction } from '../../types/generated';

const mocks = vi.hoisted(() => ({
  getAction: vi.fn(),
  cancelAction: vi.fn(),
  launchAction: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  discussionActions: { get: vi.fn(), cancel: vi.fn(), launch: vi.fn() },
  pages: mocks,
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) => args.length > 0 ? `${key}:${args.join('/')}` : key,
  }),
}));
vi.mock('../RunStatusCard', () => ({
  RunStatusCard: ({ runId }: { runId?: string }) => <div data-testid="run-card">{runId}</div>,
}));

import { LivePageActionCard } from '../LivePageActionCard';

function action(overrides: Partial<LivePageAction> = {}): LivePageAction {
  return {
    id: 'page-action:page-1:ticket',
    live_page_id: 'page-1',
    live_page_revision_id: 'rev-1',
    action_ref: 'ticket',
    kind: 'quick_exec',
    target_id: 'qe-1',
    target_name: 'Open ticket',
    project_id: 'project-1',
    state: 'proposed',
    values: [{
      name: 'ticket', label: 'Ticket', placeholder: '', description: null,
      required: true, allow_manual_override: false, provenance: 'dynamic_binding',
      source_ref: '<page.dataset.tickets.find(key).id>',
    }],
    shared_run_id: null,
    result_discussion_id: null,
    deep_link: null,
    diagnostic: null,
    launched_at: null,
    finished_at: null,
    created_at: '2026-09-01T08:00:00Z',
    updated_at: '2026-09-01T08:00:00Z',
    stale_source: false,
    ...overrides,
  };
}

describe('LivePageActionCard', () => {
  beforeEach(() => vi.clearAllMocks());

  it('uses the shared native form and forwards only the dataset row selector', async () => {
    mocks.launchAction.mockResolvedValue(action({ state: 'launching' }));
    render(
      <LivePageActionCard
        action={action()}
        bindings={{ ticket: 'KT-538' }}
        onChanged={vi.fn()}
        onOpenDiscussion={vi.fn()}
      />,
    );

    expect(screen.getByDisplayValue('disc.action.resolvedAtLaunch')).toBeDisabled();
    expect(screen.getByText(/KT-538/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /disc\.action\.launch/ }));
    await waitFor(() => expect(mocks.launchAction).toHaveBeenCalledWith(
      'page-action:page-1:ticket',
      { variables: {}, bindings: { ticket: 'KT-538' } },
    ));
  });

  it('keeps a terminal action anchored and explains a stale Page revision', () => {
    render(
      <LivePageActionCard
        action={action({ state: 'succeeded', stale_source: true, shared_run_id: 'run-1' })}
        onChanged={vi.fn()}
        onOpenDiscussion={vi.fn()}
      />,
    );
    expect(screen.getByText('disc.action.stalePageSource')).toBeInTheDocument();
    expect(screen.getByTestId('run-card')).toHaveTextContent('run-1');
  });

  it('refuses launch when the proposal was removed from the current Page revision', () => {
    render(
      <LivePageActionCard
        action={action({ state: 'proposed', stale_source: true })}
        onChanged={vi.fn()}
        onOpenDiscussion={vi.fn()}
      />,
    );
    expect(screen.getByText('disc.action.unavailablePageAction')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /disc\.action\.launch/ })).toBeDisabled();
  });
});
