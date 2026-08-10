import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

const { configApi } = vi.hoisted(() => ({
  configApi: {
    getAntiHallucinationMode: vi.fn(),
    saveAntiHallucinationMode: vi.fn(),
  },
}));

vi.mock('../../../lib/api', () => ({ config: configApi }));

import { AntiHallucSection } from '../AntiHallucSection';

const t = (key: string) => key;

describe('AntiHallucSection', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    configApi.getAntiHallucinationMode.mockResolvedValue('warn');
    configApi.saveAntiHallucinationMode.mockResolvedValue(undefined);
  });

  it('explains the difference between Kronn-launched, MCP and full CLI agents', async () => {
    render(<AntiHallucSection toast={vi.fn()} t={t} />);
    await waitFor(() => expect(configApi.getAntiHallucinationMode).toHaveBeenCalled());

    expect(document.querySelectorAll('#settings-sourcing .set-beta-feature-panel')).toHaveLength(2);

    fireEvent.click(screen.getByRole('button', { name: 'settings.sourcingScopeTitle' }));

    expect(screen.getByText('settings.sourcingScopeLaunched')).toBeInTheDocument();
    expect(screen.getByText('settings.sourcingScopeExternal')).toBeInTheDocument();
  });
});
