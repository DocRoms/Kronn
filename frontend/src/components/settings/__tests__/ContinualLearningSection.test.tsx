import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';

const { configApi } = vi.hoisted(() => ({
  configApi: {
    getContinualLearningEnabled: vi.fn(),
    saveContinualLearningEnabled: vi.fn(),
  },
}));

vi.mock('../../../lib/api', () => ({ config: configApi }));

import { ContinualLearningSection } from '../ContinualLearningSection';

const t = (key: string) => key;

describe('ContinualLearningSection', () => {
  beforeEach(() => {
    cleanup();
    vi.clearAllMocks();
    configApi.getContinualLearningEnabled.mockResolvedValue(false);
    configApi.saveContinualLearningEnabled.mockResolvedValue(undefined);
  });

  it('loads the current toggle state (OFF) on mount', async () => {
    render(<ContinualLearningSection toast={vi.fn()} t={t} />);
    await waitFor(() => expect(configApi.getContinualLearningEnabled).toHaveBeenCalled());
    const cb = screen.getByRole('checkbox') as HTMLInputElement;
    expect(cb.checked).toBe(false);
    // beta badge present (it's opt-in / beta)
    expect(screen.getByText('settings.betaBadge')).toBeInTheDocument();
  });

  it('reflects ON when the backend reports enabled', async () => {
    configApi.getContinualLearningEnabled.mockResolvedValue(true);
    render(<ContinualLearningSection toast={vi.fn()} t={t} />);
    await waitFor(() => {
      expect((screen.getByRole('checkbox') as HTMLInputElement).checked).toBe(true);
    });
  });

  it('saves on toggle + toasts success', async () => {
    const toast = vi.fn();
    render(<ContinualLearningSection toast={toast} t={t} />);
    await waitFor(() => expect(configApi.getContinualLearningEnabled).toHaveBeenCalled());
    fireEvent.click(screen.getByRole('checkbox'));
    expect(configApi.saveContinualLearningEnabled).toHaveBeenCalledWith(true);
    await waitFor(() => expect(toast).toHaveBeenCalledWith('settings.clSaved', 'success'));
  });

  it('reverts the toggle + toasts error when save fails', async () => {
    const toast = vi.fn();
    configApi.saveContinualLearningEnabled.mockRejectedValue(new Error('boom'));
    render(<ContinualLearningSection toast={toast} t={t} />);
    await waitFor(() => expect(configApi.getContinualLearningEnabled).toHaveBeenCalled());
    const cb = screen.getByRole('checkbox') as HTMLInputElement;
    fireEvent.click(cb);
    await waitFor(() => expect(toast).toHaveBeenCalledWith('settings.clSaveError', 'error'));
    expect(cb.checked).toBe(false); // reverted
  });
});
