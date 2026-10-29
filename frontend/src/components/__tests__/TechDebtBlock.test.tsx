// KT-982 — the project card block: progress, remaining per severity, the
// Validated rule and the review entry point.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
    locale: 'fr',
  }),
}));

import { TechDebtBlock } from '../TechDebtBlock';
import { projects as projectsApi } from '../../lib/api';
import { tdItem, tdList } from '../../test/tdFixtures';
import type { Project } from '../../types/generated';

const PROJECT = { id: 'p1', name: 'Kronn', path: '/repo', tech_debt_count: 3 } as Project;
const mocked = (fn: unknown) => fn as ReturnType<typeof vi.fn>;

function renderBlock(project = PROJECT) {
  const props = {
    project, auditActive: false, agents: [], toast: vi.fn(),
    onOpenDiscussion: vi.fn(), onNavigate: vi.fn(), onRefetch: vi.fn(),
  };
  render(<TechDebtBlock {...props} />);
  return props;
}

beforeEach(() => vi.clearAllMocks());

describe('TechDebtBlock', () => {
  it('shows progress, remaining per severity and the rule', async () => {
    mocked(projectsApi.auditTds).mockResolvedValueOnce(tdList([
      tdItem('TD-1', 'Critical', 'confirmed'),
      tdItem('TD-2', 'Critical', 'deferred', { task: { id: 't', reference: 'KT-1' } }),
      tdItem('TD-3', 'Low', 'rejected'),
    ]));
    const props = renderBlock();
    expect(await screen.findByTestId('td-block')).toBeInTheDocument();
    expect(screen.getByText('techDebt.remaining 1')).toBeInTheDocument();
    expect(screen.getByText('1 / 2')).toBeInTheDocument();
    expect(screen.getByTestId('td-rule')).toHaveTextContent('techDebt.rule 2 1');
    expect(screen.getByTestId('td-rule')).toHaveAttribute('data-met', 'false');
    expect(screen.getByTestId('td-open-review')).toHaveTextContent('techDebt.open 1');
    expect(screen.queryByTestId('td-validate')).toBeNull();
    fireEvent.click(screen.getByText('techDebt.validationLink'));
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('disc-validation');
    fireEvent.click(screen.getByText('techDebt.tasksLink 1'));
    expect(props.onNavigate).toHaveBeenCalledWith('planning');
  });

  it('validates the project once every Critical and High item is settled', async () => {
    mocked(projectsApi.auditTds).mockResolvedValue(tdList([
      tdItem('TD-1', 'High', 'accepted_decision'),
      tdItem('TD-2', 'Low'),
    ]));
    const props = renderBlock();
    fireEvent.click(await screen.findByTestId('td-validate'));
    await waitFor(() => expect(projectsApi.validateAudit).toHaveBeenCalledWith('p1'));
    await waitFor(() => expect(props.onRefetch).toHaveBeenCalled());
    expect(screen.getByTestId('td-rule')).toHaveAttribute('data-met', 'true');
  });

  it('flags open cross-agent disputes and links to their discussion', async () => {
    const list = tdList([tdItem('TD-1', 'High', 'confirmed')]);
    mocked(projectsApi.auditTds).mockResolvedValueOnce({
      ...list,
      gate: { ...list.gate, met: false, open_disputes: 2 },
      dispute_discussion_id: 'disc-disputes',
    });
    const props = renderBlock();
    expect(await screen.findByTestId('td-disputes')).toHaveTextContent('techDebt.disputes 2');
    expect(screen.queryByTestId('td-validate')).toBeNull();
    fireEvent.click(screen.getByTestId('td-disputes-link'));
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('disc-disputes');
    // onOpenDiscussion navigates by itself: a trailing navigate to the bare
    // page dropped the id and reopened the last discussion viewed (#421).
    expect(props.onNavigate).not.toHaveBeenCalledWith('discussions');
  });

  it('shows no dispute line when none is open', async () => {
    mocked(projectsApi.auditTds).mockResolvedValueOnce(tdList([tdItem('TD-1', 'High')]));
    renderBlock();
    expect(await screen.findByTestId('td-block')).toBeInTheDocument();
    expect(screen.queryByTestId('td-disputes')).toBeNull();
  });

  it('shows the active debt count the project card shows', async () => {
    // Pending, confirmed and deferred are active debt; rejected and
    // kept-on-purpose are settled and the card does not count them.
    mocked(projectsApi.auditTds).mockResolvedValueOnce(tdList([
      tdItem('TD-1', 'High'),
      tdItem('TD-2', 'Low', 'confirmed'),
      tdItem('TD-3', 'Low', 'deferred'),
      tdItem('TD-4', 'Low', 'rejected'),
      tdItem('TD-5', 'Low', 'accepted_decision'),
    ]));
    renderBlock({ ...PROJECT, tech_debt_count: 3 });
    expect(await screen.findByTestId('td-active-count')).toHaveTextContent('techDebt.active 3');
  });

  it('opens the review screen', async () => {
    // The review is a lazy chunk (markdown renderer included): load it before
    // the click so the test measures the screen, not the first import.
    await import('../TechDebtReview');
    mocked(projectsApi.auditTds).mockResolvedValueOnce(tdList([tdItem('TD-1', 'High')]));
    renderBlock();
    fireEvent.click(await screen.findByTestId('td-open-review'));
    expect(await screen.findByTestId('td-review', {}, { timeout: 10000 })).toBeInTheDocument();
  }, 20000);

  it('stays hidden without tech debt', () => {
    renderBlock({ ...PROJECT, tech_debt_count: 0 });
    expect(projectsApi.auditTds).not.toHaveBeenCalled();
    expect(screen.queryByTestId('td-block')).toBeNull();
  });
});
