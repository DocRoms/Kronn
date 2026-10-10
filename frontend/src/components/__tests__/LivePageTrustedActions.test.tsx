import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePageActionTrustState } from '../../types/generated';

vi.mock('../../lib/api', () => ({ pages: { trustAction: vi.fn(), revokeActionTrust: vi.fn() } }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: unknown[]) => [key, ...args].join('|') }),
}));

import { pages as pagesApi } from '../../lib/api';
import { LivePageTrustedActions, TRUSTED_ACTIONS_EXPANDED_KEY } from '../LivePageTrustedActions';

function state(overrides: Partial<LivePageActionTrustState> = {}): LivePageActionTrustState {
  return {
    action_id: 'page-action:p:todo-move', action_ref: 'todo-move', target_name: 'Move card',
    fingerprint: 'fp-1', refusal: null, trust: null, active: false, ...overrides,
  };
}

let counter = 0;
function many(count: number, overrides: Partial<LivePageActionTrustState>): LivePageActionTrustState[] {
  return Array.from({ length: count }, () => {
    counter += 1;
    return state({ action_id: `page-action:p:a${counter}`, action_ref: `a${counter}`, target_name: `Workflow ${counter}`, ...overrides });
  });
}

const expand = () => fireEvent.click(screen.getByTestId('page-trusted-actions-toggle'));

beforeEach(() => {
  localStorage.clear();
  vi.mocked(pagesApi.trustAction).mockReset().mockResolvedValue({} as never);
  vi.mocked(pagesApi.revokeActionTrust).mockReset().mockResolvedValue(true);
});

describe('LivePageTrustedActions', () => {
  it('renders nothing for a Page without actions', () => {
    const { container } = render(<LivePageTrustedActions trusts={[]} onChanged={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('starts as one collapsed summary line with the right counts', () => {
    const trusts = [
      ...many(4, {}),
      ...many(1, { active: true }),
      ...many(3, { fingerprint: null, refusal: 'cross_project' }),
      ...many(2, { fingerprint: null, refusal: 'agent_step' }),
    ];
    render(<LivePageTrustedActions trusts={trusts} onChanged={vi.fn()} />);
    expect(screen.getByTestId('page-trusted-actions')).toHaveAttribute('data-expanded', 'false');
    expect(screen.getByTestId('page-trusted-actions-counts')).toHaveTextContent(
      'pages.trust.summaryPending|4 · pages.trust.summaryApproved|1 · pages.trust.summaryIneligible|5',
    );
    expect(screen.getByTestId('page-trusted-actions-toggle')).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('listitem')).toBeNull();
    expect(screen.queryByText(/pages\.trust\.approve/)).toBeNull();
  });

  it('says briefly that nothing is eligible, without an empty list', () => {
    render(<LivePageTrustedActions trusts={many(3, { fingerprint: null, refusal: 'user_input' })} onChanged={vi.fn()} />);
    expect(screen.getByTestId('page-trusted-actions-counts')).toHaveTextContent('pages.trust.summaryNone|3');
    expect(screen.queryByText(/summaryPending/)).toBeNull();
    expand();
    expect(screen.queryByTestId('page-trusted-actions-eligible')).toBeNull();
    expect(screen.getByTestId('page-trust-group-user_input')).toBeInTheDocument();
  });

  it('expands on Détails and remembers it for this viewer', () => {
    const { unmount } = render(<LivePageTrustedActions trusts={[state()]} onChanged={vi.fn()} />);
    expand();
    expect(screen.getByTestId('page-trusted-actions-toggle')).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByTestId('page-trust-todo-move')).toBeInTheDocument();
    expect(localStorage.getItem(TRUSTED_ACTIONS_EXPANDED_KEY)).toBe('true');
    unmount();
    render(<LivePageTrustedActions trusts={[state()]} onChanged={vi.fn()} />);
    expect(screen.getByTestId('page-trusted-actions')).toHaveAttribute('data-expanded', 'true');
    expand();
    expect(screen.queryByTestId('page-trust-todo-move')).toBeNull();
    expect(localStorage.getItem(TRUSTED_ACTIONS_EXPANDED_KEY)).toBe('false');
  });

  it('lists eligible actions first and groups the others by reason, collapsed with a count', () => {
    const trusts = [
      ...many(2, { fingerprint: null, refusal: 'cross_project' }),
      state({ action_id: 'page-action:p:ok', action_ref: 'ok' }),
      ...many(3, { fingerprint: null, refusal: 'agent_step' }),
      ...many(1, { fingerprint: null, refusal: 'cross_project' }),
    ];
    render(<LivePageTrustedActions trusts={trusts} onChanged={vi.fn()} />);
    expand();
    const eligible = screen.getByTestId('page-trusted-actions-eligible');
    expect(within(eligible).getAllByRole('listitem').map(item => item.dataset.testid)).toEqual(['page-trust-ok']);

    const cross = screen.getByTestId('page-trust-group-cross_project');
    const agent = screen.getByTestId('page-trust-group-agent_step');
    expect(cross.tagName).toBe('DETAILS');
    expect(cross).not.toHaveAttribute('open');
    expect(agent).not.toHaveAttribute('open');
    expect(cross.querySelector('summary')).toHaveTextContent('pages.trust.ineligible|pages.trust.reason.cross_project3');
    expect(agent.querySelector('summary')).toHaveTextContent('pages.trust.ineligible|pages.trust.reason.agent_step3');
    expect(within(cross).getAllByRole('listitem', { hidden: true })).toHaveLength(3);
    // The eligible list comes before every group in the document.
    expect(eligible.compareDocumentPosition(cross) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('approves only after a confirmation, with the fingerprint the reader saw', async () => {
    const onChanged = vi.fn();
    render(<LivePageTrustedActions trusts={[state()]} onChanged={onChanged} />);
    expand();
    fireEvent.click(screen.getByText(/pages\.trust\.approve/));
    expect(pagesApi.trustAction).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText('pages.trust.confirmYes'));
    await waitFor(() => expect(pagesApi.trustAction).toHaveBeenCalledWith('page-action:p:todo-move', 'fp-1'));
    expect(onChanged).toHaveBeenCalled();
  });

  it('runs a double approval click once (useAsyncGuard)', async () => {
    let release: () => void = () => {};
    vi.mocked(pagesApi.trustAction).mockReturnValue(new Promise(resolve => { release = () => resolve({} as never); }));
    render(<LivePageTrustedActions trusts={[state()]} onChanged={vi.fn()} />);
    expand();
    fireEvent.click(screen.getByText(/pages\.trust\.approve/));
    const confirm = screen.getByText('pages.trust.confirmYes');
    fireEvent.click(confirm);
    fireEvent.click(confirm);
    release();
    await waitFor(() => expect(pagesApi.trustAction).toHaveBeenCalledTimes(1));
  });

  it('withdraws an active approval in one click', async () => {
    render(<LivePageTrustedActions trusts={[state({ active: true })]} onChanged={vi.fn()} />);
    expand();
    fireEvent.click(screen.getByText(/pages\.trust\.revoke/));
    await waitFor(() => expect(pagesApi.revokeActionTrust).toHaveBeenCalledWith('page-action:p:todo-move'));
  });

  it('says why an action is not eligible and offers no approval', () => {
    render(<LivePageTrustedActions trusts={[state({ fingerprint: null, refusal: 'agent_step' })]} onChanged={vi.fn()} />);
    expand();
    expect(screen.getByText('pages.trust.ineligible|pages.trust.reason.agent_step')).toBeInTheDocument();
    expect(screen.queryByText(/pages\.trust\.approve/)).toBeNull();
  });

  it('shows a visible notice for an invalidated approval and counts it in the summary', () => {
    render(<LivePageTrustedActions trusts={[state({
      trust: {
        action_id: 'page-action:p:todo-move', live_page_id: 'p', action_ref: 'todo-move', project_id: null,
        target_id: 'wf', fingerprint: 'fp-0', approval_id: 'a-0', approved_at: 'x', invalidated_at: 'y', invalidated_reason: 'changed',
      },
    })]} onChanged={vi.fn()} />);
    expect(screen.getByTestId('page-trusted-actions-counts')).toHaveTextContent('pages.trust.summaryWithdrawn|1');
    expand();
    expect(screen.getByRole('status')).toHaveTextContent('pages.trust.invalidated|pages.trust.reason.changed');
    expect(screen.getByText(/pages\.trust\.reapprove/)).toBeInTheDocument();
  });

  it('starts collapsed when storage is unavailable', () => {
    const getItem = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked'); });
    try {
      render(<LivePageTrustedActions trusts={[state()]} onChanged={vi.fn()} />);
      expect(screen.getByTestId('page-trusted-actions')).toHaveAttribute('data-expanded', 'false');
    } finally {
      getItem.mockRestore();
    }
  });
});
