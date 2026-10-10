// KT-1103 — Kronn's global timezone in Settings.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const { configApi } = vi.hoisted(() => ({
  configApi: { getServerConfig: vi.fn(), setServerConfig: vi.fn() },
}));

vi.mock('../../../lib/api', () => ({ config: configApi }));

import { TimezoneSetting } from '../TimezoneSetting';

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join(',')}` : key);

beforeEach(() => {
  configApi.getServerConfig.mockReset();
  configApi.setServerConfig.mockReset();
  configApi.getServerConfig.mockResolvedValue({ timezone: null, timezone_effective: 'Europe/Paris', timezone_detected: 'Europe/Paris' });
  configApi.setServerConfig.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe('TimezoneSetting', () => {
  it('shows the zone in use and the machine zone as the default', async () => {
    render(<TimezoneSetting toast={vi.fn()} t={t} />);
    expect(await screen.findByText('config.timezoneEffective:Europe/Paris')).toBeInTheDocument();
    expect(screen.getByLabelText('config.timezone')).toHaveAttribute('placeholder', 'Europe/Paris');
    expect(screen.getByText('config.timezoneHint:Europe/Paris')).toBeInTheDocument();
  });

  it('saves a zone, and an empty value goes back to the machine zone', async () => {
    const toast = vi.fn();
    render(<TimezoneSetting toast={toast} t={t} />);
    await screen.findByText('config.timezoneEffective:Europe/Paris');
    fireEvent.change(screen.getByLabelText('config.timezone'), { target: { value: ' Asia/Tokyo ' } });
    fireEvent.click(screen.getByLabelText('config.timezoneSave'));
    await waitFor(() => expect(configApi.setServerConfig).toHaveBeenCalledWith({ timezone: 'Asia/Tokyo' }));
    expect(await screen.findByText('config.timezoneEffective:Asia/Tokyo')).toBeInTheDocument();
    expect(toast).toHaveBeenCalledWith('config.timezoneSaved', 'success');

    fireEvent.change(screen.getByLabelText('config.timezone'), { target: { value: '' } });
    fireEvent.click(screen.getByLabelText('config.timezoneSave'));
    await waitFor(() => expect(configApi.setServerConfig).toHaveBeenLastCalledWith({ timezone: '' }));
    expect(await screen.findByText('config.timezoneEffective:Europe/Paris')).toBeInTheDocument();
  });

  it('shows the server refusal of an unknown zone', async () => {
    configApi.setServerConfig.mockRejectedValue(new Error('Unknown timezone `Mars/Olympus`'));
    render(<TimezoneSetting toast={vi.fn()} t={t} />);
    await screen.findByText('config.timezoneEffective:Europe/Paris');
    fireEvent.change(screen.getByLabelText('config.timezone'), { target: { value: 'Mars/Olympus' } });
    fireEvent.click(screen.getByLabelText('config.timezoneSave'));
    expect(await screen.findByRole('alert')).toHaveTextContent('Mars/Olympus');
  });

  it('sends one save while the previous one is still in flight', async () => {
    let finish: () => void = () => {};
    configApi.setServerConfig.mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
    render(<TimezoneSetting toast={vi.fn()} t={t} />);
    await screen.findByText('config.timezoneEffective:Europe/Paris');
    fireEvent.change(screen.getByLabelText('config.timezone'), { target: { value: 'Asia/Tokyo' } });
    const button = screen.getByLabelText('config.timezoneSave');
    fireEvent.click(button);
    fireEvent.click(button);
    fireEvent.keyDown(screen.getByLabelText('config.timezone'), { key: 'Enter' });
    expect(configApi.setServerConfig).toHaveBeenCalledTimes(1);
    finish();
    expect(await screen.findByText('config.timezoneEffective:Asia/Tokyo')).toBeInTheDocument();
  });

  it('keeps what the user typed when the first load answers late', async () => {
    let answer: (cfg: unknown) => void = () => {};
    configApi.getServerConfig.mockImplementation(() => new Promise(resolve => { answer = resolve; }));
    render(<TimezoneSetting toast={vi.fn()} t={t} />);
    const input = screen.getByLabelText('config.timezone');
    fireEvent.change(input, { target: { value: 'Asia/Tokyo' } });
    answer({ timezone: 'UTC', timezone_effective: 'UTC', timezone_detected: 'Europe/Paris' });
    expect(await screen.findByText('config.timezoneEffective:UTC')).toBeInTheDocument();
    expect(input).toHaveValue('Asia/Tokyo');
  });

  it('ignores a first load that answers after a save', async () => {
    let answer: (cfg: unknown) => void = () => {};
    configApi.getServerConfig.mockImplementation(() => new Promise(resolve => { answer = resolve; }));
    render(<TimezoneSetting toast={vi.fn()} t={t} />);
    const input = screen.getByLabelText('config.timezone');
    fireEvent.change(input, { target: { value: 'Asia/Tokyo' } });
    fireEvent.click(screen.getByLabelText('config.timezoneSave'));
    expect(await screen.findByText('config.timezoneEffective:Asia/Tokyo')).toBeInTheDocument();
    answer({ timezone: null, timezone_effective: 'UTC', timezone_detected: 'UTC' });
    // The stale answer still lands its machine zone in the hint.
    expect(await screen.findByText('config.timezoneHint:UTC')).toBeInTheDocument();
    expect(screen.getByText('config.timezoneEffective:Asia/Tokyo')).toBeInTheDocument();
    expect(screen.queryByText('config.timezoneEffective:UTC')).toBeNull();
    expect(input).toHaveValue('Asia/Tokyo');
  });
});
