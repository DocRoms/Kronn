import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { AutoDisabledWorkflow } from '../../../types/generated';

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: unknown[]) => [key, ...args].join('|') }),
}));

import { AutoDisabledReview } from '../AutoDisabledReview';

const manual: AutoDisabledWorkflow = {
  id: 'wf-manual',
  name: 'Weekly digest',
  project_id: null,
  trigger: { type: 'Manual' },
  reason: 'agent_edit',
  disabled_at: '2026-10-07T09:00:00Z',
  disabled_by: 'Codex',
  summary: 'steps changed by Codex',
};
const cron: AutoDisabledWorkflow = {
  ...manual,
  id: 'wf-cron',
  name: 'Nightly import',
  trigger: { type: 'Cron', schedule: '0 3 * * *' },
  reason: 'imported',
  disabled_by: 'import',
  summary: 'imported from the file of « Nightly import »',
};

describe('AutoDisabledReview', () => {
  const confirmSpy = vi.fn((_message?: string) => true);
  beforeEach(() => {
    confirmSpy.mockReset();
    confirmSpy.mockReturnValue(true);
    vi.stubGlobal('confirm', confirmSpy);
  });
  afterEach(() => vi.unstubAllGlobals());

  it('renders nothing when no workflow was disabled by Kronn', () => {
    const { container } = render(
      <AutoDisabledReview items={[]} onReenable={vi.fn()} onOpen={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it('shows the count and lists each workflow with its reason, summary, author and link', () => {
    const onOpen = vi.fn();
    render(<AutoDisabledReview items={[manual, cron]} onReenable={vi.fn()} onOpen={onOpen} />);
    expect(screen.getByTestId('auto-disabled-banner')).toHaveTextContent('wf.autoDisabled.banner|2');
    expect(screen.queryByTestId('auto-disabled-panel')).not.toBeInTheDocument();

    fireEvent.click(screen.getByText('wf.autoDisabled.details'));
    const rows = screen.getAllByTestId('auto-disabled-row');
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent('wf.autoDisabled.reason.agent_edit');
    expect(rows[0]).toHaveTextContent('steps changed by Codex');
    expect(rows[0]).toHaveTextContent('wf.autoDisabled.byWhen|Codex|');
    expect(rows[1]).toHaveTextContent('wf.autoDisabled.reason.imported');

    fireEvent.click(screen.getByText('Weekly digest'));
    expect(onOpen).toHaveBeenCalledWith('wf-manual');
  });

  it('re-enables a manual workflow without asking, and asks first for a Cron one', async () => {
    const onReenable = vi.fn().mockResolvedValue(undefined);
    render(<AutoDisabledReview items={[manual, cron]} onReenable={onReenable} onOpen={vi.fn()} />);
    fireEvent.click(screen.getByText('wf.autoDisabled.details'));
    const buttons = screen.getAllByText('wf.autoDisabled.reenable');

    fireEvent.click(buttons[0]);
    await waitFor(() => expect(onReenable).toHaveBeenCalledWith(['wf-manual']));
    expect(confirmSpy).not.toHaveBeenCalled();

    confirmSpy.mockReturnValueOnce(false);
    fireEvent.click(buttons[1]);
    await waitFor(() => expect(confirmSpy).toHaveBeenCalledWith('wf.autoDisabled.confirmScheduled|Nightly import'));
    expect(onReenable).toHaveBeenCalledTimes(1);

    fireEvent.click(buttons[1]);
    await waitFor(() => expect(onReenable).toHaveBeenLastCalledWith(['wf-cron']));
  });

  it('re-enables all after a confirmation that counts the scheduled ones', async () => {
    const onReenable = vi.fn().mockResolvedValue(undefined);
    render(<AutoDisabledReview items={[manual, cron]} onReenable={onReenable} onOpen={vi.fn()} />);
    fireEvent.click(screen.getByText('wf.autoDisabled.details'));
    fireEvent.click(screen.getByText('wf.autoDisabled.reenableAll'));
    await waitFor(() => expect(onReenable).toHaveBeenCalledWith(['wf-manual', 'wf-cron']));
    expect(confirmSpy).toHaveBeenCalledWith('wf.autoDisabled.confirmAllScheduled|2|1');
  });
});
