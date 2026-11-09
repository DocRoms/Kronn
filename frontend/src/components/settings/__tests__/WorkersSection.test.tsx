import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import { ApiRequestError } from '../../../lib/apiRequestError';

const { getMock, saveMock } = vi.hoisted(() => ({ getMock: vi.fn(), saveMock: vi.fn() }));

vi.mock('../../../lib/api', () => buildApiMock({
  config: { getWorkers: getMock as never, saveWorkers: saveMock as never },
}));

import { WorkersSection } from '../WorkersSection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join(',')})` : key;

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('WorkersSection (KT-1345)', () => {
  it('shows the saved value and where it applies, without a warning for a number', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2 });
    render(<WorkersSection t={t} />);
    const input = await screen.findByTestId('workers-rounds-input');
    await waitFor(() => expect(input).toHaveValue(5));
    expect(screen.getByTestId('workers-current')).toHaveTextContent('config.workers.currentRounds(5)');
    expect(screen.getByTestId('workers-applies')).toHaveTextContent('config.workers.appliesTo');
    expect(screen.queryByTestId('workers-unlimited-warning')).not.toBeInTheDocument();
    expect(screen.getByTestId('workers-save')).toBeDisabled();
  });

  it('saves a new integer', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2 });
    saveMock.mockResolvedValue({ default_max_review_rounds: 8, review_rounds_unlimited: false, max_concurrent_task_workers: 2 });
    render(<WorkersSection t={t} />);
    const input = await screen.findByTestId('workers-rounds-input');
    await waitFor(() => expect(input).toHaveValue(5));

    fireEvent.change(input, { target: { value: '8' } });
    fireEvent.click(screen.getByTestId('workers-save'));
    await waitFor(() => expect(saveMock).toHaveBeenCalledWith({
      default_max_review_rounds: 8,
      review_rounds_unlimited: false,
      max_concurrent_task_workers: 2,
    }));
    await waitFor(() => expect(screen.getByTestId('workers-current'))
      .toHaveTextContent('config.workers.currentRounds(8)'));
  });

  it('warns about token burn as soon as Unlimited is ticked, and saves it keeping the integer', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 6, review_rounds_unlimited: false, max_concurrent_task_workers: 2 });
    saveMock.mockResolvedValue({ default_max_review_rounds: 6, review_rounds_unlimited: true, max_concurrent_task_workers: 2 });
    render(<WorkersSection t={t} />);
    const input = await screen.findByTestId('workers-rounds-input');
    await waitFor(() => expect(input).toHaveValue(6));

    fireEvent.click(screen.getByTestId('workers-unlimited'));
    expect(screen.getByTestId('workers-unlimited-warning'))
      .toHaveTextContent('config.workers.unlimitedWarning');
    expect(input).toBeDisabled();

    fireEvent.click(screen.getByTestId('workers-save'));
    await waitFor(() => expect(saveMock).toHaveBeenCalledWith({
      default_max_review_rounds: 6,
      review_rounds_unlimited: true,
      max_concurrent_task_workers: 2,
    }));
    await waitFor(() => expect(screen.getByTestId('workers-current'))
      .toHaveTextContent('config.workers.currentUnlimited'));
    // Still unlimited after the save: the warning stays.
    expect(screen.getByTestId('workers-unlimited-warning')).toBeInTheDocument();
  });

  it('opens on an unlimited setting with the warning visible', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: true, max_concurrent_task_workers: 2 });
    render(<WorkersSection t={t} />);
    expect(await screen.findByTestId('workers-unlimited-warning')).toBeInTheDocument();
    expect(screen.getByTestId('workers-current')).toHaveTextContent('config.workers.currentUnlimited');
  });

  it('refuses 0 and out-of-range values before calling the API', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2 });
    render(<WorkersSection t={t} />);
    const input = await screen.findByTestId('workers-rounds-input');
    await waitFor(() => expect(input).toHaveValue(5));

    for (const bad of ['0', '', '1001', '2.5']) {
      fireEvent.change(input, { target: { value: bad } });
      expect(screen.getByTestId('workers-invalid')).toBeInTheDocument();
      expect(screen.getByTestId('workers-save')).toBeDisabled();
    }
    expect(saveMock).not.toHaveBeenCalled();
  });

  it('KT-1632 — shows the task-worker cap, saves a new one and refuses 0 or too many', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2 });
    saveMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 3 });
    render(<WorkersSection t={t} />);
    const cap = await screen.findByTestId('workers-cap-input');
    await waitFor(() => expect(cap).toHaveValue(2));
    expect(screen.getByTestId('workers-cap-current')).toHaveTextContent('config.workers.maxConcurrentCurrent(2)');
    expect(screen.getByTestId('workers-cap-hint')).toHaveTextContent('config.workers.maxConcurrentHint');
    expect(screen.getByTestId('workers-save')).toBeDisabled();

    for (const bad of ['0', '', '65', '1.5']) {
      fireEvent.change(cap, { target: { value: bad } });
      expect(screen.getByTestId('workers-cap-invalid')).toBeInTheDocument();
      expect(screen.getByTestId('workers-save')).toBeDisabled();
    }
    expect(saveMock).not.toHaveBeenCalled();

    fireEvent.change(cap, { target: { value: '3' } });
    expect(screen.queryByTestId('workers-cap-invalid')).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId('workers-save'));
    await waitFor(() => expect(saveMock).toHaveBeenCalledWith({
      default_max_review_rounds: 5,
      review_rounds_unlimited: false,
      max_concurrent_task_workers: 3,
    }));
    await waitFor(() => expect(screen.getByTestId('workers-cap-current'))
      .toHaveTextContent('config.workers.maxConcurrentCurrent(3)'));
  });

  it('shows the server refusal and keeps the field editable', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2 });
    saveMock.mockRejectedValue(new ApiRequestError('default_max_review_rounds must be between 1 and 1000', 'validation'));
    render(<WorkersSection t={t} />);
    const input = await screen.findByTestId('workers-rounds-input');
    await waitFor(() => expect(input).toHaveValue(5));

    fireEvent.change(input, { target: { value: '9' } });
    fireEvent.click(screen.getByTestId('workers-save'));
    await waitFor(() => expect(screen.getByTestId('workers-error')).toHaveTextContent('between 1 and 1000'));
    expect(input).not.toBeDisabled();
  });
});
