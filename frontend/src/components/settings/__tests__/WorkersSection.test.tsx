import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import { ApiRequestError } from '../../../lib/apiRequestError';

const { getMock, saveMock, getServerMock } = vi.hoisted(() => ({ getMock: vi.fn(), saveMock: vi.fn(), getServerMock: vi.fn() }));

vi.mock('../../../lib/api', () => buildApiMock({
  config: { getWorkers: getMock as never, saveWorkers: saveMock as never, getServerConfig: getServerMock as never },
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
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true });
    render(<WorkersSection t={t} />);
    const input = await screen.findByTestId('workers-rounds-input');
    await waitFor(() => expect(input).toHaveValue(5));
    expect(screen.getByTestId('workers-current')).toHaveTextContent('config.workers.currentRounds(5)');
    expect(screen.getByTestId('workers-applies')).toHaveTextContent('config.workers.appliesTo');
    expect(screen.queryByTestId('workers-unlimited-warning')).not.toBeInTheDocument();
    expect(screen.getByTestId('workers-save')).toBeDisabled();
  });

  it('saves a new integer', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true });
    saveMock.mockResolvedValue({ default_max_review_rounds: 8, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true });
    render(<WorkersSection t={t} />);
    const input = await screen.findByTestId('workers-rounds-input');
    await waitFor(() => expect(input).toHaveValue(5));

    fireEvent.change(input, { target: { value: '8' } });
    fireEvent.click(screen.getByTestId('workers-save'));
    await waitFor(() => expect(saveMock).toHaveBeenCalledWith({
      default_max_review_rounds: 8,
      review_rounds_unlimited: false,
      max_concurrent_task_workers: 2,
      background_priority: true,
      heavy_output_kb: 256,
      bash_default_timeout_min: 15,
      bash_max_timeout_min: 60,
      lane_warm: {},
    }));
    await waitFor(() => expect(screen.getByTestId('workers-current'))
      .toHaveTextContent('config.workers.currentRounds(8)'));
  });

  it('warns about token burn as soon as Unlimited is ticked, and saves it keeping the integer', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 6, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true });
    saveMock.mockResolvedValue({ default_max_review_rounds: 6, review_rounds_unlimited: true, max_concurrent_task_workers: 2, background_priority: true });
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
      background_priority: true,
      heavy_output_kb: 256,
      bash_default_timeout_min: 15,
      bash_max_timeout_min: 60,
      lane_warm: {},
    }));
    await waitFor(() => expect(screen.getByTestId('workers-current'))
      .toHaveTextContent('config.workers.currentUnlimited'));
    // Still unlimited after the save: the warning stays.
    expect(screen.getByTestId('workers-unlimited-warning')).toBeInTheDocument();
  });

  it('opens on an unlimited setting with the warning visible', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: true, max_concurrent_task_workers: 2, background_priority: true });
    render(<WorkersSection t={t} />);
    expect(await screen.findByTestId('workers-unlimited-warning')).toBeInTheDocument();
    expect(screen.getByTestId('workers-current')).toHaveTextContent('config.workers.currentUnlimited');
  });

  it('refuses 0 and out-of-range values before calling the API', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true });
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
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true });
    saveMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 3, background_priority: true });
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
      background_priority: true,
      heavy_output_kb: 256,
      bash_default_timeout_min: 15,
      bash_max_timeout_min: 60,
      lane_warm: {},
    }));
    await waitFor(() => expect(screen.getByTestId('workers-cap-current'))
      .toHaveTextContent('config.workers.maxConcurrentCurrent(3)'));
  });

  it('shows the server refusal and keeps the field editable', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true });
    saveMock.mockRejectedValue(new ApiRequestError('default_max_review_rounds must be between 1 and 1000', 'validation'));
    render(<WorkersSection t={t} />);
    const input = await screen.findByTestId('workers-rounds-input');
    await waitFor(() => expect(input).toHaveValue(5));

    fireEvent.change(input, { target: { value: '9' } });
    fireEvent.click(screen.getByTestId('workers-save'));
    await waitFor(() => expect(screen.getByTestId('workers-error')).toHaveTextContent('between 1 and 1000'));
    expect(input).not.toBeDisabled();
  });

  // KT-1633 — the background-priority switch shares the card and the save button.
  it('shows the background-priority switch on by default and explains it', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true });
    render(<WorkersSection t={t} />);
    const box = await screen.findByTestId('workers-background-priority');
    await waitFor(() => expect(box).toBeChecked());
    expect(screen.getByTestId('workers-background-hint')).toHaveTextContent('config.workers.backgroundPriorityHint');
    expect(screen.getByTestId('workers-save')).toBeDisabled();
  });

  it('saves the switch turned off, keeping the review-round values', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true });
    saveMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: false });
    render(<WorkersSection t={t} />);
    const box = await screen.findByTestId('workers-background-priority');
    await waitFor(() => expect(box).toBeChecked());

    fireEvent.click(box);
    expect(screen.getByTestId('workers-save')).toBeEnabled();
    fireEvent.click(screen.getByTestId('workers-save'));
    await waitFor(() => expect(saveMock).toHaveBeenCalledWith({
      default_max_review_rounds: 5,
      review_rounds_unlimited: false,
      max_concurrent_task_workers: 2,
      background_priority: false,
      heavy_output_kb: 256,
      bash_default_timeout_min: 15,
      bash_max_timeout_min: 60,
      lane_warm: {},
    }));
    await waitFor(() => expect(box).not.toBeChecked());
    expect(screen.getByTestId('workers-save')).toBeDisabled();
  });

  it('opens with the switch off when the server says so, and leaves it on for a server that predates it', async () => {
    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: false });
    const { unmount } = render(<WorkersSection t={t} />);
    const off = await screen.findByTestId('workers-background-priority');
    await waitFor(() => expect(screen.getByTestId('workers-rounds-input')).toHaveValue(5));
    expect(off).not.toBeChecked();
    unmount();

    getMock.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2 });
    render(<WorkersSection t={t} />);
    const legacy = await screen.findByTestId('workers-background-priority');
    await waitFor(() => expect(screen.getByTestId('workers-rounds-input')).toHaveValue(5));
    expect(legacy).toBeChecked();
  });

  it('KT-1525 — shows the heavy-output threshold, saves a new one and refuses 0 or too many', async () => {
    const saved = { default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true, heavy_output_kb: 256, bash_default_timeout_min: 15, bash_max_timeout_min: 60 };
    getMock.mockResolvedValue(saved);
    saveMock.mockResolvedValue({ ...saved, heavy_output_kb: 512 });
    render(<WorkersSection t={t} />);
    const input = await screen.findByTestId('workers-heavy-input');
    await waitFor(() => expect(input).toHaveValue(256));
    expect(screen.getByTestId('workers-heavy-current')).toHaveTextContent('config.workers.heavyOutputCurrent(256)');
    expect(screen.getByTestId('workers-save')).toBeDisabled();

    for (const bad of ['0', '102401']) {
      fireEvent.change(input, { target: { value: bad } });
      expect(screen.getByTestId('workers-heavy-invalid')).toHaveTextContent('config.workers.heavyOutputInvalid(1,102400)');
      expect(screen.getByTestId('workers-save')).toBeDisabled();
    }

    fireEvent.change(input, { target: { value: '512' } });
    expect(screen.queryByTestId('workers-heavy-invalid')).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId('workers-save'));
    await waitFor(() => expect(saveMock).toHaveBeenCalledWith({ ...saved, heavy_output_kb: 512, lane_warm: {} }));
    await waitFor(() => expect(screen.getByTestId('workers-heavy-current'))
      .toHaveTextContent('config.workers.heavyOutputCurrent(512)'));
  });

  it('KT-1718 — shows the per-command cap next to the generation limit, saves new limits and refuses a default above the maximum', async () => {
    const saved = { default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true, heavy_output_kb: 256, bash_default_timeout_min: 15, bash_max_timeout_min: 60 };
    getMock.mockResolvedValue(saved);
    saveMock.mockResolvedValue({ ...saved, bash_max_timeout_min: 90 });
    getServerMock.mockResolvedValue({ agent_global_timeout_min: 120 });
    render(<WorkersSection t={t} />);
    const max = await screen.findByTestId('workers-bash-max-input');
    await waitFor(() => expect(max).toHaveValue(60));
    expect(screen.getByTestId('workers-bash-default-input')).toHaveValue(15);
    await waitFor(() => expect(screen.getByTestId('workers-generation-limit')).toHaveTextContent('120'));
    expect(screen.getByTestId('workers-shell-current'))
      .toHaveTextContent('config.workers.shellTimeoutsSummary(15,60,120)');
    expect(screen.getByTestId('workers-save')).toBeDisabled();
    expect(screen.queryByTestId('workers-shell-outlive-warning')).not.toBeInTheDocument();

    for (const bad of ['0', '241', '1.5']) {
      fireEvent.change(max, { target: { value: bad } });
      expect(screen.getByTestId('workers-shell-invalid')).toHaveTextContent('config.workers.shellTimeoutsInvalid(1,240)');
      expect(screen.getByTestId('workers-save')).toBeDisabled();
    }
    fireEvent.change(max, { target: { value: '10' } });
    expect(screen.getByTestId('workers-shell-invalid')).toBeInTheDocument();
    expect(screen.getByTestId('workers-save')).toBeDisabled();

    // A command allowed as long as the generation would be cut with it.
    fireEvent.change(max, { target: { value: '120' } });
    expect(screen.queryByTestId('workers-shell-invalid')).not.toBeInTheDocument();
    expect(screen.getByTestId('workers-shell-outlive-warning')).toHaveTextContent('config.workers.shellTimeoutsOutlive');

    fireEvent.change(max, { target: { value: '90' } });
    expect(screen.queryByTestId('workers-shell-outlive-warning')).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId('workers-save'));
    await waitFor(() => expect(saveMock).toHaveBeenCalledWith({ ...saved, bash_max_timeout_min: 90, lane_warm: {} }));
    await waitFor(() => expect(screen.getByTestId('workers-shell-current'))
      .toHaveTextContent('config.workers.shellTimeoutsSummary(15,90,120)'));
  });
});

describe('WorkersSection lane warm-up (KT-1742)', () => {
  const base = { default_max_review_rounds: 5, review_rounds_unlimited: false, max_concurrent_task_workers: 2, background_priority: true, heavy_output_kb: 256, bash_default_timeout_min: 15, bash_max_timeout_min: 60 };

  it('shows what is saved and starts on the detected defaults', async () => {
    getMock.mockResolvedValue({ ...base, lane_warm: { seed_alignment: false, first_build_command: 'npm ci', first_build_timeout_min: 90 } });
    render(<WorkersSection t={t} />);
    await waitFor(() => expect(screen.getByTestId('workers-lane-warm-alignment')).toHaveValue('off'));
    expect(screen.getByTestId('workers-lane-warm-command')).toHaveValue('npm ci');
    expect(screen.getByTestId('workers-lane-warm-timeout')).toHaveValue(90);
    expect(screen.getByTestId('workers-lane-warm-env-mode')).toHaveValue('inherit');
  });

  it('saves the global entry: a custom environment, a command and a bound', async () => {
    getMock.mockResolvedValue({ ...base, lane_warm: {} });
    saveMock.mockImplementation(async (value: unknown) => value);
    render(<WorkersSection t={t} />);
    await waitFor(() => expect(screen.getByTestId('workers-lane-warm-env-mode')).not.toBeDisabled());

    fireEvent.change(screen.getByTestId('workers-lane-warm-env-mode'), { target: { value: 'custom' } });
    fireEvent.change(screen.getByTestId('workers-lane-warm-env-text'), { target: { value: 'CC=clang\nCFLAGS=-O1' } });
    fireEvent.change(screen.getByTestId('workers-lane-warm-command'), { target: { value: 'composer install' } });
    fireEvent.change(screen.getByTestId('workers-lane-warm-timeout'), { target: { value: '45' } });
    fireEvent.change(screen.getByTestId('workers-lane-warm-alignment'), { target: { value: 'on' } });
    fireEvent.click(screen.getByTestId('workers-save'));

    await waitFor(() => expect(saveMock).toHaveBeenCalledWith({
      ...base,
      lane_warm: {
        seed_alignment: true,
        compiler_env: ['CC=clang', 'CFLAGS=-O1'],
        first_build_command: 'composer install',
        first_build_timeout_min: 45,
      },
    }));
  });

  it('refuses a malformed environment line or an out-of-range bound before saving', async () => {
    getMock.mockResolvedValue({ ...base, lane_warm: {} });
    render(<WorkersSection t={t} />);
    await waitFor(() => expect(screen.getByTestId('workers-lane-warm-env-mode')).not.toBeDisabled());

    fireEvent.change(screen.getByTestId('workers-lane-warm-env-mode'), { target: { value: 'custom' } });
    fireEvent.change(screen.getByTestId('workers-lane-warm-env-text'), { target: { value: 'NOT_AN_ASSIGNMENT' } });
    expect(screen.getByTestId('workers-lane-warm-env-invalid')).toBeInTheDocument();
    expect(screen.getByTestId('workers-save')).toBeDisabled();

    fireEvent.change(screen.getByTestId('workers-lane-warm-env-mode'), { target: { value: 'none' } });
    fireEvent.change(screen.getByTestId('workers-lane-warm-timeout'), { target: { value: '0' } });
    expect(screen.getByTestId('workers-lane-warm-timeout-invalid')).toBeInTheDocument();
    expect(screen.getByTestId('workers-save')).toBeDisabled();
  });

  it('an empty list means no compiler environment at all, saved as such', async () => {
    getMock.mockResolvedValue({ ...base, lane_warm: {} });
    saveMock.mockImplementation(async (value: unknown) => value);
    render(<WorkersSection t={t} />);
    await waitFor(() => expect(screen.getByTestId('workers-lane-warm-env-mode')).not.toBeDisabled());

    fireEvent.change(screen.getByTestId('workers-lane-warm-env-mode'), { target: { value: 'none' } });
    fireEvent.click(screen.getByTestId('workers-save'));
    await waitFor(() => expect(saveMock).toHaveBeenCalledWith({ ...base, lane_warm: { compiler_env: [] } }));
  });
});
