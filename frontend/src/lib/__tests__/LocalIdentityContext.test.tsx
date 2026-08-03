import { act, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { config as configApi } from '../api';
import { LocalIdentityProvider } from '../LocalIdentityContext';
import { useLocalIdentity } from '../localIdentity';

vi.mock('../api', () => ({
  config: {
    getServerConfig: vi.fn(),
    getAgentAccess: vi.fn().mockResolvedValue(null),
  },
}));

function IdentityProbe() {
  const { pseudo, avatarEmail } = useLocalIdentity();
  return <span>{pseudo ?? 'none'}|{avatarEmail ?? 'none'}</span>;
}

describe('LocalIdentityProvider', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it('reloads the human identity after the backend becomes reachable', async () => {
    vi.useFakeTimers();
    vi.mocked(configApi.getServerConfig)
      .mockRejectedValueOnce(new Error('backend restarting'))
      .mockResolvedValueOnce({
        pseudo: 'Romu - mac',
        avatar_email: 'romu@example.com',
      } as Awaited<ReturnType<typeof configApi.getServerConfig>>);

    render(
      <LocalIdentityProvider>
        <IdentityProbe />
      </LocalIdentityProvider>,
    );

    expect(screen.getByText('none|none')).toBeInTheDocument();
    await act(async () => {
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(5_000);
    });

    expect(screen.getByText('Romu - mac|romu@example.com')).toBeInTheDocument();
    expect(configApi.getServerConfig).toHaveBeenCalledTimes(2);
  });
});
