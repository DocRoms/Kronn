// WorktreesSection (KT-1438) — the three global lane-seed switches.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const { configApi } = vi.hoisted(() => ({
  configApi: { getServerConfig: vi.fn(), setServerConfig: vi.fn() },
}));

vi.mock('../../../lib/api', () => ({ config: configApi }));

import { WorktreesSection } from '../WorktreesSection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join('|')})` : key;

const serverConfig = (over: Record<string, unknown> = {}) => ({
  lane_seeds_copy_enabled: true,
  lane_seeds_adopt_suggested: false,
  lane_seeds_strong_confirm: true,
  ...over,
});

beforeEach(() => {
  configApi.getServerConfig.mockResolvedValue(serverConfig());
  configApi.setServerConfig.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('WorktreesSection (KT-1438)', () => {
  it('shows the defaults: copy on, adoption off, strong confirmation on', async () => {
    render(<WorktreesSection toast={vi.fn()} t={t} />);
    expect(await screen.findByTestId('worktrees-copy')).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByTestId('worktrees-adopt')).toHaveAttribute('aria-checked', 'false');
    expect(screen.getByTestId('worktrees-confirm')).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByText('settings.worktrees.copy')).toBeInTheDocument();
    expect(screen.getByText('settings.worktrees.adopt')).toBeInTheDocument();
    expect(screen.getByText('settings.worktrees.confirm')).toBeInTheDocument();
  });

  it('saves each switch on its own field and shows the new state', async () => {
    const toast = vi.fn();
    render(<WorktreesSection toast={toast} t={t} />);

    fireEvent.click(await screen.findByTestId('worktrees-adopt'));
    await waitFor(() => expect(screen.getByTestId('worktrees-adopt')).toHaveAttribute('aria-checked', 'true'));
    expect(configApi.setServerConfig).toHaveBeenLastCalledWith({ lane_seeds_adopt_suggested: true });

    fireEvent.click(screen.getByTestId('worktrees-copy'));
    await waitFor(() => expect(screen.getByTestId('worktrees-copy')).toHaveAttribute('aria-checked', 'false'));
    expect(configApi.setServerConfig).toHaveBeenLastCalledWith({ lane_seeds_copy_enabled: false });

    fireEvent.click(screen.getByTestId('worktrees-confirm'));
    await waitFor(() => expect(screen.getByTestId('worktrees-confirm')).toHaveAttribute('aria-checked', 'false'));
    expect(configApi.setServerConfig).toHaveBeenLastCalledWith({ lane_seeds_strong_confirm: false });
    expect(toast).toHaveBeenCalledWith('settings.worktrees.saved', 'success');
  });

  it('keeps the switch where it was when the change is refused (a cancelled confirmation)', async () => {
    configApi.setServerConfig.mockRejectedValue(new Error('strong confirmation cancelled'));
    render(<WorktreesSection toast={vi.fn()} t={t} />);

    fireEvent.click(await screen.findByTestId('worktrees-confirm'));
    expect(await screen.findByTestId('worktrees-error')).toBeInTheDocument();
    expect(screen.getByTestId('worktrees-confirm')).toHaveAttribute('aria-checked', 'true');
  });
});
