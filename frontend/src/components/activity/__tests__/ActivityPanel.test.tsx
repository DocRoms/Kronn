import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../../lib/I18nContext';
import type { ActivityOverview, ActivityRow } from '../../../types/generated';
import { ActivityPanel, type ActivityPanelProps } from '../ActivityPanel';

vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock();
});

const NOW = Date.parse('2026-11-10T12:00:00Z');
const ago = (minutes: number) => new Date(NOW - minutes * 60_000).toISOString();

function row(overrides: Partial<ActivityRow> & Pick<ActivityRow, 'id' | 'type'>): ActivityRow {
  return {
    compartment: 'running',
    state: 'running',
    source: 'test',
    title: overrides.id,
    started_at: ago(4),
    stalled: false,
    actions: [],
    ...overrides,
  };
}

const overview = (rows: ActivityRow[], extra: Partial<ActivityOverview> = {}): ActivityOverview => ({
  rows, stall_after_secs: 900, generated_at: new Date(NOW).toISOString(), unavailable_sources: [], ...extra,
});

function renderPanel(rows: ActivityRow[] | null, props: Partial<ActivityPanelProps> = {}, extra: Partial<ActivityOverview> = {}) {
  const onOpenLink = vi.fn();
  const onClose = vi.fn();
  render(
    <I18nProvider>
      <ActivityPanel
        overview={rows ? overview(rows, extra) : null}
        onClose={onClose}
        onOpenLink={onOpenLink}
        {...props}
      />
    </I18nProvider>,
  );
  return { onOpenLink, onClose };
}

const FOUR_KINDS: ActivityRow[] = [
  row({
    id: 'agent_dispatch_jobs:j1', type: 'agent_turn', title: 'Fix the login', project_name: 'Kronn',
    link: { target: 'discussion', id: 'd1' },
    actions: [{ id: 'stop_dispatch', kind: 'stop', args: { discussion_id: 'd1', dispatch_id: 'j1' } }],
  }),
  row({
    id: 'workflow_runs:r1', type: 'workflow_run', title: 'Nightly review',
    link: { target: 'workflow', id: 'wf-1', run_id: 'r1' },
    actions: [{ id: 'cancel_workflow_run', kind: 'stop', args: { workflow_id: 'wf-1', run_id: 'r1' } }],
  }),
  row({
    id: 'audit_tracker:p1', type: 'audit', title: 'Kronn', detail: '3/10 · repo-map.md',
    link: { target: 'project', id: 'p1' },
    actions: [{ id: 'cancel_audit', kind: 'stop', args: { project_id: 'p1' } }],
  }),
  row({
    id: 'task_executions:e1', type: 'task_worker', title: 'KT-12 · Ship the bell', state: 'queued',
    link: { target: 'planning_task', id: 't1' },
    actions: [{ id: 'cancel_task_execution', kind: 'cancel', args: { execution_id: 'e1' } }],
  }),
];

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('ActivityPanel — the "En cours" compartment', () => {
  it('lists discussions, workflow runs, audits and task workers with their state, project and elapsed time', () => {
    vi.useFakeTimers({ now: NOW, toFake: ['Date', 'setInterval', 'clearInterval'] });
    renderPanel(FOUR_KINDS);

    const section = screen.getByRole('region', { name: /En cours/ });
    expect(section).toHaveAttribute('data-compartment', 'running');
    const rows = within(section).getAllByTestId('activity-row');
    expect(rows.map(r => r.getAttribute('data-row-type'))).toEqual(['agent_turn', 'workflow_run', 'audit', 'task_worker']);

    const turn = rows[0];
    expect(turn).toHaveTextContent('Fix the login');
    expect(turn).toHaveTextContent('Réponse d\'agent');
    expect(turn).toHaveTextContent('Kronn');
    expect(within(turn).getByTestId('activity-row-elapsed')).toHaveTextContent('depuis 4m 00s');
    expect(within(turn).getByTestId('activity-row-elapsed')).toHaveAccessibleName('Temps écoulé : 4m 00s');
    expect(rows[2]).toHaveTextContent('3/10 · repo-map.md');
    expect(within(rows[3]).getByTestId('activity-row-state')).toHaveTextContent('En file');
  });

  it('counts elapsed time live', () => {
    vi.useFakeTimers({ now: NOW, toFake: ['Date', 'setInterval', 'clearInterval'] });
    renderPanel(FOUR_KINDS.slice(0, 1));
    expect(screen.getByTestId('activity-row-elapsed')).toHaveTextContent('depuis 4m 00s');
    act(() => { vi.advanceTimersByTime(5_000); });
    expect(screen.getByTestId('activity-row-elapsed')).toHaveTextContent('depuis 4m 05s');
  });

  it('shows a paused workflow run as paused with its wake time and a Resume now, never as running', () => {
    const handler = vi.fn().mockResolvedValue(undefined);
    renderPanel([
      row({
        id: 'workflow_runs:paused', type: 'workflow_run', title: 'PR Review v3.4', state: 'paused',
        detail: 'Agent', wake_at: '2026-11-11T02:02:00Z',
        link: { target: 'workflow', id: 'wf-review', run_id: 'paused' },
        actions: [{ id: 'resume_workflow_run', kind: 'resume_now', args: { run_id: 'paused' } }],
      }),
    ], { handlers: { resume_workflow_run: handler } });

    const item = screen.getByTestId('activity-row');
    expect(item).toHaveAttribute('data-state', 'paused');
    expect(within(item).getByTestId('activity-row-state')).toHaveTextContent('En pause');
    expect(within(item).getByTestId('activity-wake')).toHaveTextContent('Reprise automatique à');
    fireEvent.click(within(item).getByRole('button', { name: /Reprendre maintenant/ }));
    expect(handler).toHaveBeenCalledWith({ run_id: 'paused' });
  });

  it('says a parked run waits for a person when no wake time is scheduled', () => {
    renderPanel([row({ id: 'workflow_runs:parked', type: 'workflow_run', state: 'paused' })]);
    expect(screen.getByTestId('activity-wake')).toHaveTextContent('En attente de votre reprise');
  });

  it('flags a stalled row with how long it has been silent, and not a fresh one', () => {
    vi.useFakeTimers({ now: NOW, toFake: ['Date', 'setInterval', 'clearInterval'] });
    renderPanel([
      row({ id: 'a', type: 'agent_turn', title: 'Quiet one', stalled: true, last_activity_at: ago(21) }),
      row({ id: 'b', type: 'agent_turn', title: 'Fresh one', last_activity_at: ago(1) }),
    ]);
    const [quiet, fresh] = screen.getAllByTestId('activity-row');
    expect(quiet).toHaveAttribute('data-stalled', 'true');
    expect(within(quiet).getByTestId('activity-stalled')).toHaveTextContent('Aucune activité depuis 21m 00s');
    expect(fresh).not.toHaveAttribute('data-stalled');
    expect(within(fresh).queryByTestId('activity-stalled')).toBeNull();
  });

  it('opens the deep link of a row, and leaves a modified click to the browser (new tab)', () => {
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    const { onOpenLink } = renderPanel(FOUR_KINDS);
    const link = screen.getByRole('button', { name: 'Ouvrir Nightly review' });
    fireEvent.click(link);
    expect(onOpenLink).toHaveBeenCalledWith({ target: 'workflow', id: 'wf-1', run_id: 'r1' });

    fireEvent.click(link, { ctrlKey: true });
    expect(onOpenLink).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith(`${window.location.origin}/workflows/wf-1/runs/r1`, '_blank', 'noopener,noreferrer');
  });

  it('runs the existing action of each kind with its own arguments', async () => {
    const handlers = {
      stop_dispatch: vi.fn().mockResolvedValue(undefined),
      cancel_workflow_run: vi.fn().mockResolvedValue(undefined),
      cancel_audit: vi.fn().mockResolvedValue(undefined),
      cancel_task_execution: vi.fn().mockResolvedValue(undefined),
    };
    const onActionSettled = vi.fn();
    renderPanel(FOUR_KINDS, { handlers, onActionSettled });

    for (const name of [/Arrêter : Fix the login/, /Arrêter : Nightly review/, /Arrêter : Kronn/, /Annuler : KT-12/]) {
      await act(async () => { fireEvent.click(screen.getByRole('button', { name })); });
    }
    expect(handlers.stop_dispatch).toHaveBeenCalledWith({ discussion_id: 'd1', dispatch_id: 'j1' });
    expect(handlers.cancel_workflow_run).toHaveBeenCalledWith({ workflow_id: 'wf-1', run_id: 'r1' });
    expect(handlers.cancel_audit).toHaveBeenCalledWith({ project_id: 'p1' });
    expect(handlers.cancel_task_execution).toHaveBeenCalledWith({ execution_id: 'e1' });
    expect(onActionSettled).toHaveBeenCalledTimes(4);
  });

  it('never fires an action twice for a double click, and shows a failure with an explicit retry', async () => {
    let fail = true;
    let release: () => void = () => {};
    const handler = vi.fn(() => new Promise<void>((resolve, reject) => {
      release = () => (fail ? reject(new Error('backend down')) : resolve());
    }));
    renderPanel(FOUR_KINDS.slice(0, 1), { handlers: { stop_dispatch: handler } });
    const stop = screen.getByRole('button', { name: /Arrêter : Fix the login/ });

    fireEvent.click(stop);
    fireEvent.click(stop);
    expect(handler).toHaveBeenCalledTimes(1);
    expect(stop).toBeDisabled();

    await act(async () => { release(); });
    expect(await screen.findByRole('alert')).toHaveTextContent('L\'action a échoué : backend down');
    expect(stop).toBeEnabled();

    fail = false;
    fireEvent.click(stop);
    expect(handler).toHaveBeenCalledTimes(2);
    await act(async () => { release(); });
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('shows an empty "En cours" with nothing running, and a loading hint before the first answer', () => {
    renderPanel([]);
    expect(screen.getByRole('region', { name: /En cours/ })).toHaveTextContent('Rien ne tourne pour le moment.');
    cleanup();
    renderPanel(null);
    expect(screen.getByRole('region', { name: /En cours/ })).toHaveTextContent('Chargement…');
  });

  it('says when the list may be incomplete or out of date', () => {
    renderPanel([], { failed: true }, { unavailable_sources: ['task_executions'] });
    expect(screen.getByText(/Lecture impossible : task_executions/)).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('Ce qui s\'affiche peut être périmé');
  });

  it('is a labelled, non-modal dialog whose rows are reachable by keyboard', () => {
    renderPanel(FOUR_KINDS);
    const dialog = screen.getByRole('dialog', { name: 'Activité' });
    expect(dialog).toHaveAttribute('aria-modal', 'false');
    for (const control of within(dialog).getAllByRole('button')) {
      expect(control).not.toHaveAttribute('tabindex', '-1');
      expect(control).toHaveAccessibleName();
    }
  });
});

describe('ActivityPanel — a row it has never seen (DoD 1)', () => {
  it('renders an injected row type with its own title, state and elapsed time, without any change to the panel', () => {
    renderPanel([
      row({
        id: 'media_jobs:42', type: 'media_job', title: 'Render the clip', source: 'media_jobs', detail: '2/5 frames',
        link: { target: 'project', id: 'p9' },
      }),
    ]);
    const item = screen.getByTestId('activity-row');
    expect(item).toHaveAttribute('data-row-type', 'media_job');
    expect(item).toHaveTextContent('Render the clip');
    expect(item).toHaveTextContent('Media job');
    expect(item).toHaveTextContent('2/5 frames');
    expect(within(item).getByTestId('activity-row-elapsed')).toBeInTheDocument();
    expect(within(item).getByRole('button', { name: 'Ouvrir Render the clip' })).toBeInTheDocument();
  });

  it('hides an action whose handler this build does not know, instead of showing a dead button', () => {
    renderPanel([
      row({
        id: 'x:1', type: 'media_job', title: 'Render',
        actions: [{ id: 'cancel_media_job', kind: 'cancel', args: { job: '1' } }],
      }),
    ]);
    expect(screen.queryByRole('button', { name: /Annuler/ })).toBeNull();
  });
});

describe('ActivityPanel — the KT-334 compartments are reserved', () => {
  it('shows only "En cours" until another compartment has a row', () => {
    renderPanel(FOUR_KINDS);
    const sections = document.querySelectorAll('[data-compartment]');
    expect(Array.from(sections, s => s.getAttribute('data-compartment'))).toEqual(['running']);
  });

  it('places a row of a later compartment under its own heading, in reading order, with no panel change', () => {
    renderPanel([
      row({ id: 'done:1', type: 'workflow_run', title: 'Finished earlier', compartment: 'unseen_done', state: 'running' }),
      row({ id: 'run:1', type: 'agent_turn', title: 'Working now' }),
      row({ id: 'ask:1', type: 'agent_turn', title: 'Needs you', compartment: 'needs_decision' }),
    ]);
    const sections = Array.from(document.querySelectorAll('[data-compartment]'));
    expect(sections.map(s => s.getAttribute('data-compartment'))).toEqual(['needs_decision', 'running', 'unseen_done']);
    expect(screen.getByRole('region', { name: /Attend une décision/ })).toHaveTextContent('Needs you');
    expect(screen.getByRole('region', { name: /Terminé non-vu/ })).toHaveTextContent('Finished earlier');
    expect(screen.getByRole('region', { name: /En cours/ })).toHaveTextContent('Working now');
  });

  it('orders a compartment working first, then queued, then paused, oldest first', () => {
    renderPanel([
      row({ id: 'p', type: 'workflow_run', title: 'Paused', state: 'paused', started_at: ago(50) }),
      row({ id: 'q', type: 'task_worker', title: 'Queued', state: 'queued', started_at: ago(40) }),
      row({ id: 'r2', type: 'agent_turn', title: 'Newer', started_at: ago(3) }),
      row({ id: 'r1', type: 'agent_turn', title: 'Older', started_at: ago(30) }),
    ]);
    const titles = screen.getAllByTestId('activity-row').map(r => within(r).getByText(/Paused|Queued|Newer|Older/).textContent);
    expect(titles).toEqual(['Older', 'Newer', 'Queued', 'Paused']);
  });
});
