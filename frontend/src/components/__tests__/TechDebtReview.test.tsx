// KT-982 — the tech-debt review: grouped list, filters, atomic bulk decisions,
// single decisions with a reason, task links and placeholders for trackers.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { useState } from 'react';
import userEvent from '@testing-library/user-event';
import { buildApiMock } from '../../test/apiMock';
import { expectFocusKeptIn } from '../../test/dialogFocus';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
    locale: 'fr',
  }),
}));

import { TechDebtReview } from '../TechDebtReview';
import { projects as projectsApi } from '../../lib/api';
import { tdItem, tdList } from '../../test/tdFixtures';
import type { Project, TdReviewList } from '../../types/generated';

const PROJECT = { id: 'p1', name: 'Kronn', path: '/repo', tech_debt_count: 4 } as Project;
const mocked = (fn: unknown) => fn as ReturnType<typeof vi.fn>;

function list(): TdReviewList {
  return tdList([
    tdItem('TD-20261001-crit', 'Critical', 'pending', { talk_count: 2 }),
    tdItem('TD-20261001-high', 'High', 'deferred'),
    tdItem('TD-20261001-med', 'Medium', 'pending', { task: { id: 'task-1', reference: 'KT-42' } }),
    tdItem('TD-20261001-low', 'Low', 'confirmed'),
  ]);
}

function setup(data = list()) {
  const props = {
    project: PROJECT,
    list: data,
    agents: [],
    toast: vi.fn(),
    onReload: vi.fn().mockResolvedValue(undefined),
    onValidate: vi.fn(),
    onClose: vi.fn(),
    onOpenDiscussion: vi.fn(),
    onNavigate: vi.fn(),
  };
  render(<TechDebtReview {...props} />);
  return props;
}

beforeEach(() => vi.clearAllMocks());

describe('TechDebtReview', () => {
  it('opens on the items to settle, deferred included, grouped by severity', () => {
    setup();
    expect(screen.getByTestId('td-row-TD-20261001-crit')).toBeInTheDocument();
    expect(screen.getByTestId('td-row-TD-20261001-high')).toBeInTheDocument();
    expect(screen.queryByTestId('td-row-TD-20261001-low')).toBeNull();
    expect(screen.getByTestId('td-filter-todo')).toHaveTextContent('techDebt.filter.todo 3');
    fireEvent.click(screen.getByTestId('td-filter-done'));
    expect(screen.getByTestId('td-row-TD-20261001-low')).toBeInTheDocument();
    expect(screen.queryByTestId('td-row-TD-20261001-crit')).toBeNull();
    // The first item to settle is the current one; the talk icon shows the exchange.
    fireEvent.click(screen.getByTestId('td-filter-all'));
    expect(screen.getByTestId('td-row-TD-20261001-crit')).toHaveAttribute('data-current', 'true');
    expect(within(screen.getByTestId('td-row-TD-20261001-crit')).getByTitle('techDebt.talkCount 2')).toBeInTheDocument();
  });

  it('never shows the word ADR', () => {
    setup();
    expect(document.body.textContent ?? '').not.toMatch(/\bADR\b/);
  });

  it('settles a whole selection in one request', async () => {
    mocked(projectsApi.decideTds).mockResolvedValueOnce({
      results: [
        { td_id: 'TD-20261001-crit', state: 'rejected' },
        { td_id: 'TD-20261001-high', state: 'rejected' },
      ],
      gate: { blocking_total: 2, blocking_settled: 2, met: true, open_disputes: 0 },
    });
    const props = setup();
    fireEvent.click(screen.getByTestId('td-group-toggle-Critical'));
    fireEvent.click(within(screen.getByTestId('td-row-TD-20261001-high')).getByRole('checkbox'));
    const bulk = screen.getByTestId('td-bulk');
    expect(bulk).toHaveTextContent('techDebt.bulk.selected 2');
    fireEvent.click(within(bulk).getByText('techDebt.decision.reject'));
    await waitFor(() => expect(props.onReload).toHaveBeenCalled());
    expect(projectsApi.decideTds).toHaveBeenCalledTimes(1);
    expect(projectsApi.decideTds).toHaveBeenCalledWith('p1', [
      { td_id: 'TD-20261001-crit', decision: 'reject' },
      { td_id: 'TD-20261001-high', decision: 'reject' },
    ]);
    await waitFor(() => expect(screen.queryByTestId('td-bulk')).toBeNull());
    expect(props.toast).toHaveBeenCalledWith('techDebt.bulk.done 2', 'success');
  });

  it('keeps an item on purpose with its reason, then moves to the next one', async () => {
    mocked(projectsApi.decideTds).mockResolvedValueOnce({
      results: [{ td_id: 'TD-20261001-crit', state: 'accepted_decision' }],
      gate: { blocking_total: 2, blocking_settled: 1, met: false, open_disputes: 0 },
    });
    setup();
    fireEvent.change(screen.getByPlaceholderText('techDebt.detail.reasonPlaceholder'), {
      target: { value: '  Choix voulu  ' },
    });
    fireEvent.click(screen.getByTestId('td-decide-accept_decision'));
    await waitFor(() => expect(projectsApi.decideTds).toHaveBeenCalledWith('p1', [
      { td_id: 'TD-20261001-crit', decision: 'accept_decision', reason: 'Choix voulu' },
    ]));
    await waitFor(() =>
      expect(screen.getByTestId('td-row-TD-20261001-high')).toHaveAttribute('data-current', 'true'));
  });

  it('sends the typed reason with every kind of decision, and none when it is empty', async () => {
    mocked(projectsApi.decideTds).mockResolvedValue({
      results: [], gate: { blocking_total: 2, blocking_settled: 0, met: false, open_disputes: 0 },
    });
    setup();
    for (const decision of ['confirm', 'defer', 'reject'] as const) {
      fireEvent.change(screen.getByPlaceholderText('techDebt.detail.reasonPlaceholder'), {
        target: { value: `because ${decision}` },
      });
      fireEvent.click(screen.getByTestId(`td-decide-${decision}`));
      await waitFor(() => expect(projectsApi.decideTds).toHaveBeenLastCalledWith('p1', [
        { td_id: 'TD-20261001-crit', decision, reason: `because ${decision}` },
      ]));
      // A deferral keeps the item current, the others move on: come back to it.
      fireEvent.click(screen.getByTestId('td-row-TD-20261001-crit').querySelector('.td-row-main') as HTMLElement);
    }
    fireEvent.change(screen.getByPlaceholderText('techDebt.detail.reasonPlaceholder'), { target: { value: '   ' } });
    fireEvent.click(screen.getByTestId('td-decide-confirm'));
    await waitFor(() => expect(projectsApi.decideTds).toHaveBeenLastCalledWith('p1', [
      { td_id: 'TD-20261001-crit', decision: 'confirm' },
    ]));
    // The label says the reason is kept for every decision, not only "keep".
    expect(screen.getByText('techDebt.detail.reason')).toBeInTheDocument();
  });

  it('renders the impact as Markdown, without raw HTML', () => {
    setup(tdList([
      tdItem('TD-20261001-crit', 'Critical', 'pending', {
        impact: '**Critical** — a key is public.\n\n- anyone can read it\n- rotate it <script>alert(1)</script>',
      }),
    ]));
    const impact = screen.getByTestId('td-markdown');
    expect(within(impact).getByText('Critical').tagName).toBe('STRONG');
    expect(within(impact).getAllByRole('listitem')).toHaveLength(2);
    expect(impact.textContent).not.toContain('**');
    expect(impact.querySelector('script')).toBeNull();
  });

  it('shows an explicit state when a sheet has no verification criterion', () => {
    setup(tdList([
      tdItem('TD-20261001-crit', 'Critical', 'pending', { verification: null }),
      tdItem('TD-20261001-high', 'High', 'pending'),
    ]));
    expect(screen.getByTestId('td-verification')).toHaveTextContent('techDebt.detail.verification');
    expect(screen.getByTestId('td-no-verification')).toHaveTextContent('techDebt.detail.noVerification');
    fireEvent.click(within(screen.getByTestId('td-row-TD-20261001-high')).getByRole('button'));
    expect(screen.queryByTestId('td-no-verification')).toBeNull();
    expect(screen.getByTestId('td-verification')).toHaveTextContent('It no longer breaks.');
  });

  it('lists a detail file without front matter and offers no task for it', () => {
    setup(tdList([
      tdItem('CQ-001', 'High', 'pending', { status: 'Unknown' }),
      tdItem('PERF-20261016-deep-clone', 'Unknown', 'pending'),
    ]));
    expect(screen.getByTestId('td-row-CQ-001')).toBeInTheDocument();
    expect(screen.getByTestId('td-row-PERF-20261016-deep-clone')).toBeInTheDocument();
    expect(screen.getByTestId('td-filter-todo')).toHaveTextContent('techDebt.filter.todo 2');
    expect(screen.queryByTestId('td-task-create')).toBeNull();
    fireEvent.click(within(screen.getByTestId('td-row-PERF-20261016-deep-clone')).getByRole('button'));
    expect(screen.getByTestId('td-detail')).toHaveTextContent('techDebt.detail.severity : Unknown');
  });

  it('offers no task for a debt only the index lists, until a sheet exists', () => {
    setup(tdList([
      tdItem('TD-20261016-rowonly', 'High', 'pending', { source_path: 'docs/inconsistencies-tech-debt.md' }),
    ]));
    expect(screen.getByTestId('td-row-TD-20261016-rowonly')).toBeInTheDocument();
    expect(screen.queryByTestId('td-task-create')).toBeNull();
  });

  it('links an existing task, creates a missing one, and keeps trackers as placeholders', async () => {
    mocked(projectsApi.createTdTask).mockResolvedValueOnce({
      created: 1, already_linked: 0, skipped: 0, themes: [],
      items: [{ td_id: 'TD-20261001-crit', theme: 'backend', state: 'created', task_reference: 'KT-50', reason: null }],
    });
    const props = setup();
    fireEvent.click(screen.getByTestId('td-task-create'));
    await waitFor(() => expect(projectsApi.createTdTask).toHaveBeenCalledWith('p1', 'TD-20261001-crit'));
    await waitFor(() => expect(props.toast).toHaveBeenCalledWith('techDebt.taskCreated KT-50 TD-20261001-crit', 'success'));
    expect(screen.getByText('techDebt.link.github')).toBeDisabled();
    expect(screen.getByText('techDebt.link.jira')).toBeDisabled();

    fireEvent.click(within(screen.getByTestId('td-row-TD-20261001-med')).getByRole('button'));
    fireEvent.click(screen.getByTestId('td-task-link'));
    expect(props.onNavigate).toHaveBeenCalledWith('planning:task-1');
  });

  it('jumps to the next item to settle and filters by search', () => {
    setup();
    fireEvent.click(screen.getByTestId('td-next'));
    expect(screen.getByTestId('td-row-TD-20261001-high')).toHaveAttribute('data-current', 'true');
    fireEvent.change(screen.getByLabelText('techDebt.search'), { target: { value: 'med' } });
    expect(screen.getByTestId('td-row-TD-20261001-med')).toBeInTheDocument();
    expect(screen.queryByTestId('td-row-TD-20261001-crit')).toBeNull();
  });

  it('offers validation once the rule is met and closes on Escape', () => {
    const met = tdList([tdItem('TD-1', 'High', 'confirmed'), tdItem('TD-2', 'Low')]);
    const props = setup(met);
    fireEvent.click(screen.getAllByText('techDebt.validate')[0]);
    expect(props.onValidate).toHaveBeenCalled();
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(props.onClose).toHaveBeenCalled();
  });

  it('leaves Escape to an IME composition or a control that consumed it (KT-1072)', () => {
    const props = setup();
    const search = screen.getByLabelText('techDebt.search');
    fireEvent.keyDown(search, { key: 'Escape', isComposing: true });
    search.addEventListener('keydown', event => event.preventDefault(), { once: true });
    fireEvent.keyDown(search, { key: 'Escape' });
    expect(props.onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(search, { key: 'Escape' });
    expect(props.onClose).toHaveBeenCalledTimes(1);
  });

  it('stays usable with 150 items', () => {
    const many = Array.from({ length: 150 }, (_, i) =>
      tdItem(`TD-20261001-n${i}`, ['Critical', 'High', 'Medium', 'Low'][i % 4]));
    setup(tdList(many));
    expect(screen.getAllByRole('checkbox')).toHaveLength(150);
    fireEvent.click(screen.getByTestId('td-group-toggle-Low'));
    expect(screen.getByTestId('td-bulk')).toHaveTextContent('techDebt.bulk.selected 37');
  });

  describe('dialog focus (KT-1072)', () => {
    function Opener() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>open-review</button>
          {open && (
            <TechDebtReview
              project={PROJECT} list={list()} agents={[]} toast={vi.fn()}
              onReload={vi.fn().mockResolvedValue(undefined)} onValidate={vi.fn()}
              onClose={() => setOpen(false)} onOpenDiscussion={vi.fn()} onNavigate={vi.fn()}
            />
          )}
        </>
      );
    }

    it('takes focus, keeps it, closes on Escape and returns focus to its trigger', async () => {
      render(<Opener />);
      const trigger = screen.getByText('open-review');
      await userEvent.click(trigger);
      const dialog = screen.getByTestId('td-review');
      await expectFocusKeptIn(dialog, 6);
      await userEvent.keyboard('{Escape}');
      expect(screen.queryByTestId('td-review')).toBeNull();
      expect(trigger).toHaveFocus();
    });

    it('the source viewer takes focus, keeps it, and Escape returns to the pointer that opened it', async () => {
      render(<Opener />);
      await userEvent.click(screen.getByText('open-review'));
      const pointer = within(screen.getByTestId('td-review')).getAllByRole('button', { name: /backend\/src\/lib\.rs/ })[0];
      await userEvent.click(pointer);
      const viewer = screen.getByRole('dialog', { name: 'backend/src/lib.rs' });
      expect(viewer).toHaveAttribute('aria-modal', 'true');
      await expectFocusKeptIn(viewer, 3);
      await userEvent.keyboard('{Escape}');
      expect(screen.queryByRole('dialog', { name: 'backend/src/lib.rs' })).toBeNull();
      expect(screen.getByTestId('td-review')).toBeInTheDocument();
      expect(pointer).toHaveFocus();
    });
  });
});
