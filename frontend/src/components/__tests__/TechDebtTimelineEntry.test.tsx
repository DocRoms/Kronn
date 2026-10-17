// KT-1335 — step 4 of the audit timeline opens the same tech-debt review
// screen as the project overview; the raw file stays a secondary link.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, screen } from '@testing-library/react';
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
});
