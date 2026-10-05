// The desktop port field validates 1024-65535, saves, and offers a restart.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const { configApi } = vi.hoisted(() => ({
  configApi: { getDesktopPort: vi.fn(), setDesktopPort: vi.fn() },
}));

vi.mock('../../../lib/api', () => ({ config: configApi }));
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: unknown[]) => (args.length ? `${key} ${args.join(' | ')}` : key),
    locale: 'en',
  }),
}));

import { DesktopPortField } from '../DesktopPortField';

const info = (over: Record<string, unknown> = {}) => ({ saved: 47315, current: 47315, min: 1024, max: 65535, ...over });

function setup() {
  const toast = vi.fn();
  const restart = vi.fn();
  render(<DesktopPortField toast={toast} onRestart={restart} />);
  return { toast, restart };
}

beforeEach(() => {
  configApi.getDesktopPort.mockReset().mockResolvedValue(info());
  configApi.setDesktopPort.mockReset();
});
afterEach(cleanup);

describe('DesktopPortField', () => {
  it('shows the saved port and the launch-time explanation', async () => {
    setup();
    const input = (await screen.findByLabelText('settings.desktopPort')) as HTMLInputElement;
    expect(input.value).toBe('47315');
    expect(screen.getByText(/settings.desktopPortHint 47315/)).toBeInTheDocument();
  });

  it('refuses out-of-range and non-numeric values', async () => {
    setup();
    const input = await screen.findByLabelText('settings.desktopPort');
    const save = screen.getByText('settings.desktopPortSave');
    for (const bad of ['80', '1023', '65536', 'abc', '', '4.5']) {
      fireEvent.change(input, { target: { value: bad } });
      expect(save).toBeDisabled();
      expect(screen.getByText(/settings.desktopPortInvalid 1024 \| 65535/)).toBeInTheDocument();
    }
    for (const good of ['1024', '65535']) {
      fireEvent.change(input, { target: { value: good } });
      expect(save).not.toBeDisabled();
    }
  });

  it('saves the port, then offers to restart', async () => {
    configApi.setDesktopPort.mockResolvedValue(info({ saved: 50000 }));
    const { restart } = setup();
    const input = await screen.findByLabelText('settings.desktopPort');
    expect(screen.queryByText('settings.desktopPortRestartBtn')).toBeNull();
    fireEvent.change(input, { target: { value: '50000' } });
    fireEvent.click(screen.getByText('settings.desktopPortSave'));
    await waitFor(() => expect(configApi.setDesktopPort).toHaveBeenCalledWith(50000));
    fireEvent.click(await screen.findByText('settings.desktopPortRestartBtn'));
    expect(restart).toHaveBeenCalledTimes(1);
  });

  it('reports a failed save', async () => {
    configApi.setDesktopPort.mockRejectedValue(new Error('disk full'));
    const { toast } = setup();
    const input = await screen.findByLabelText('settings.desktopPort');
    fireEvent.change(input, { target: { value: '50000' } });
    fireEvent.click(screen.getByText('settings.desktopPortSave'));
    await waitFor(() => expect(toast).toHaveBeenCalledWith(expect.stringContaining('common.actionFailed'), 'error'));
  });
});
