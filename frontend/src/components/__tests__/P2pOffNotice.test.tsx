import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act, cleanup, waitFor } from '@testing-library/react';

const { config } = vi.hoisted(() => ({
  config: { getServerConfig: vi.fn(), setServerConfig: vi.fn() },
}));
vi.mock('../../lib/api', () => ({ config }));

import { P2pOffNotice } from '../P2pOffNotice';

const t = (key: string) => key;

beforeEach(() => {
  config.getServerConfig.mockResolvedValue({ p2p_enabled: false });
  config.setServerConfig.mockResolvedValue(undefined);
});
afterEach(() => { cleanup(); vi.clearAllMocks(); });

describe('P2pOffNotice', () => {
  it('explains why contacts are offline and enables P2P in one click', async () => {
    render(<P2pOffNotice contactCount={2} t={t} />);
    expect(await screen.findByTestId('p2p-off-notice')).toBeInTheDocument();
    expect(screen.getByText('contacts.p2pOff')).toBeInTheDocument();
    await act(async () => { fireEvent.click(screen.getByText('contacts.p2pEnable')); });
    expect(config.setServerConfig).toHaveBeenCalledWith({ p2p_enabled: true });
    await waitFor(() => expect(screen.queryByTestId('p2p-off-notice')).toBeNull());
  });

  it('stays hidden without contacts or when P2P is on', async () => {
    const { unmount } = render(<P2pOffNotice contactCount={0} t={t} />);
    await act(async () => {});
    expect(screen.queryByTestId('p2p-off-notice')).toBeNull();
    unmount();
    config.getServerConfig.mockResolvedValue({ p2p_enabled: true });
    render(<P2pOffNotice contactCount={3} t={t} />);
    await act(async () => {});
    expect(screen.queryByTestId('p2p-off-notice')).toBeNull();
  });
});
