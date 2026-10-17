// KT-1335 — step 4 of the audit timeline opens the same tech-debt review
// screen as the project overview; the raw file stays a secondary link.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, screen, waitFor, within, act } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
    locale: 'fr',
  }),
}));

import { TechDebtBlock, TechDebtTimelineEntry } from '../TechDebtBlock';
import { projects as projectsApi } from '../../lib/api';
import { tdItem, tdList } from '../../test/tdFixtures';
import type { Project } from '../../types/generated';

const PROJECT = { id: 'p1', name: 'Kronn', path: '/repo', tech_debt_count: 3 } as Project;
const mocked = (fn: unknown) => fn as ReturnType<typeof vi.fn>;
const LIST = () => tdList([tdItem('TD-1', 'High'), tdItem('TD-2', 'Low', 'rejected')]);

function baseProps(project = PROJECT) {
  return {
    project, auditActive: false, agents: [], toast: vi.fn(),
    onOpenDiscussion: vi.fn(), onNavigate: vi.fn(), onRefetch: vi.fn(),
  };
}

beforeEach(() => vi.clearAllMocks());

describe('TechDebtTimelineEntry', () => {
  it('shows the overview control, with its label and count, and opens the same review screen', async () => {
    // The review is a lazy chunk: load it before the clicks.
    await import('../TechDebtReview');
    mocked(projectsApi.auditTds).mockResolvedValue(LIST());

    const overview = render(<TechDebtBlock {...baseProps()} />);
    const overviewButton = await screen.findByTestId('td-open-review');
    const overviewLabel = overviewButton.textContent;
    fireEvent.click(overviewButton);
    expect(await screen.findByTestId('td-review', undefined, { timeout: 10000 })).toBeInTheDocument();
    overview.unmount();

    render(<TechDebtTimelineEntry {...baseProps()} onOpenFile={vi.fn()} />);
    const stepButton = await screen.findByTestId('audit-timeline-td-open-review');
    expect(stepButton.textContent).toBe(overviewLabel);
    expect(stepButton).toHaveTextContent('techDebt.open 1');
    fireEvent.click(stepButton);
    expect(await screen.findByTestId('td-review', undefined, { timeout: 10000 })).toBeInTheDocument();
  }, 30000);

  it('keeps the raw file as a secondary link', async () => {
    mocked(projectsApi.auditTds).mockResolvedValue(LIST());
    const onOpenFile = vi.fn();
    render(<TechDebtTimelineEntry {...baseProps()} onOpenFile={onOpenFile} />);
    await screen.findByTestId('audit-timeline-td-open-review');
    const link = screen.getByTestId('audit-timeline-td-open-file');
    expect(link).toHaveClass('td-link-btn');
    expect(screen.getByTestId('audit-timeline-td-open-review')).toHaveClass('td-primary-btn');
    fireEvent.click(link);
    expect(onOpenFile).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('td-review')).toBeNull();
  });

  it('shows an explicit empty state when the audit produced no debt', () => {
    render(<TechDebtTimelineEntry {...baseProps({ ...PROJECT, tech_debt_count: 0 })} onOpenFile={vi.fn()} />);
    expect(screen.getByTestId('audit-timeline-td-empty')).toHaveTextContent('auditTimeline.validated.noDebt');
    expect(projectsApi.auditTds).not.toHaveBeenCalled();
    expect(screen.queryByTestId('audit-timeline-td-open-review')).toBeNull();
    expect(screen.queryByTestId('audit-timeline-td-open-file')).toBeNull();
  });

  it('shows the empty state when the debt index is empty', async () => {
    mocked(projectsApi.auditTds).mockResolvedValue(tdList([]));
    render(<TechDebtTimelineEntry {...baseProps()} onOpenFile={vi.fn()} />);
    expect(await screen.findByTestId('audit-timeline-td-empty')).toBeInTheDocument();
    expect(screen.queryByTestId('audit-timeline-td-open-review')).toBeNull();
  });

  it('says so when the debt cannot be read, and still offers the raw file', async () => {
    mocked(projectsApi.auditTds).mockRejectedValue(new Error('boom'));
    render(<TechDebtTimelineEntry {...baseProps()} onOpenFile={vi.fn()} />);
    expect(await screen.findByText('techDebt.loadError boom')).toBeInTheDocument();
    expect(screen.getByTestId('audit-timeline-td-open-file')).toBeInTheDocument();
  });

  it('keeps both entries in step after a decision taken from either one', async () => {
    await import('../TechDebtReview');
    const pending = tdList([tdItem('TD-1', 'High'), tdItem('TD-2', 'Medium')]);
    const decided = tdList([tdItem('TD-1', 'High', 'confirmed'), tdItem('TD-2', 'Medium')]);
    mocked(projectsApi.auditTds).mockResolvedValue(pending);
    mocked(projectsApi.decideTds).mockResolvedValue({
      results: [{ td_id: 'TD-1', state: 'confirmed' }],
      gate: { blocking_total: 1, blocking_settled: 1, met: true, open_disputes: 0 },
    });
    render(
      <>
        <TechDebtBlock {...baseProps({ ...PROJECT, tech_debt_count: 2 })} />
        <TechDebtTimelineEntry {...baseProps({ ...PROJECT, tech_debt_count: 2 })} onOpenFile={vi.fn()} />
      </>,
    );
    const overviewButton = await screen.findByTestId('td-open-review');
    const stepButton = await screen.findByTestId('audit-timeline-td-open-review');
    expect(overviewButton).toHaveTextContent('techDebt.open 2');
    expect(stepButton).toHaveTextContent('techDebt.open 2');

    // Decide from step 4: the next reload brings the list with TD-1 confirmed.
    mocked(projectsApi.auditTds).mockResolvedValue(decided);
    fireEvent.click(stepButton);
    const stepReview = await screen.findByTestId('td-review', undefined, { timeout: 10000 });
    fireEvent.click(within(stepReview).getByTestId('td-decide-confirm'));
    await waitFor(() => expect(projectsApi.decideTds).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByTestId('audit-timeline-td-open-review')).toHaveTextContent('techDebt.open 1'));
    expect(screen.getByTestId('td-open-review')).toHaveTextContent('techDebt.open 1');

    // The overview's own dialog no longer lists TD-1 as pending.
    fireEvent.click(within(stepReview).getByLabelText('techDebt.review.close'));
    fireEvent.click(screen.getByTestId('td-open-review'));
    const overviewReview = await screen.findByTestId('td-review', undefined, { timeout: 10000 });
    expect(within(overviewReview).queryByTestId('td-row-TD-1')).toBeNull();
    expect(within(overviewReview).getByTestId('td-row-TD-2')).toBeInTheDocument();
  }, 30000);

  it('never lets a read started before a decision overwrite the reload that followed it', async () => {
    await import('../TechDebtReview');
    const pending = tdList([tdItem('TD-1', 'High'), tdItem('TD-2', 'Medium')]);
    const decided = tdList([tdItem('TD-1', 'High', 'confirmed'), tdItem('TD-2', 'Medium')]);
    let answerFirstRead: (list: typeof pending) => void = () => {};
    // 1st read (overview mount) stays in flight; 2nd (step 4 mount) answers; then the decision's reload.
    mocked(projectsApi.auditTds)
      .mockImplementationOnce(() => new Promise(resolve => { answerFirstRead = resolve; }))
      .mockResolvedValueOnce(pending)
      .mockResolvedValue(decided);
    mocked(projectsApi.decideTds).mockResolvedValue({
      results: [{ td_id: 'TD-1', state: 'confirmed' }],
      gate: { blocking_total: 1, blocking_settled: 1, met: true, open_disputes: 0 },
    });
    const props = baseProps({ ...PROJECT, tech_debt_count: 2 });
    const overviewOnly = <TechDebtBlock {...props} />;
    const { rerender } = render(overviewOnly);
    await waitFor(() => expect(projectsApi.auditTds).toHaveBeenCalledTimes(1));

    // The user switches to the Audit tab while the overview's read is still in flight.
    rerender(
      <>
        {overviewOnly}
        <TechDebtTimelineEntry {...props} onOpenFile={vi.fn()} />
      </>,
    );
    const stepButton = await screen.findByTestId('audit-timeline-td-open-review');
    expect(stepButton).toHaveTextContent('techDebt.open 2');

    fireEvent.click(stepButton);
    const review = await screen.findByTestId('td-review', undefined, { timeout: 10000 });
    fireEvent.click(within(review).getByTestId('td-decide-confirm'));
    await waitFor(() => expect(screen.getByTestId('audit-timeline-td-open-review')).toHaveTextContent('techDebt.open 1'));
    expect(screen.getByTestId('td-open-review')).toHaveTextContent('techDebt.open 1');

    // The stale first read finally answers with the pre-decision state: it must be dropped.
    await act(async () => { answerFirstRead(pending); });
    expect(screen.getByTestId('audit-timeline-td-open-review')).toHaveTextContent('techDebt.open 1');
    expect(screen.getByTestId('td-open-review')).toHaveTextContent('techDebt.open 1');
  }, 30000);
});
