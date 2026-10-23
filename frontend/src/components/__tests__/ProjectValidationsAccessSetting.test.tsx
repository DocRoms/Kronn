import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

const { getMock, setMock } = vi.hoisted(() => ({
  getMock: vi.fn(),
  setMock: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  projects: { validationsAccess: getMock as never, setValidationsAccess: setMock as never },
}));

import { ProjectValidationsAccessSetting } from '../ProjectValidationsAccessSetting';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ProjectValidationsAccessSetting (KT-1433)', () => {
  it('opens Docker at once and names the socket it reaches', async () => {
    getMock.mockResolvedValue({ access: 'sandbox', docker_socket: '/Users/me/.orbstack/run/docker.sock' });
    setMock.mockResolvedValue({ access: 'docker', docker_socket: '/Users/me/.orbstack/run/docker.sock' });
    render(<ProjectValidationsAccessSetting projectId="p1" t={t} />);

    await waitFor(() => expect(screen.getByTestId('project-validations-access-sandbox')).toBeChecked());
    expect(screen.getByTestId('project-validations-access-socket')).toHaveTextContent('/Users/me/.orbstack/run/docker.sock');

    fireEvent.click(screen.getByTestId('project-validations-access-docker'));
    await waitFor(() => expect(screen.getByTestId('project-validations-access-docker')).toBeChecked());
    expect(setMock).toHaveBeenCalledWith('p1', 'docker');
  });

  it('applies full access only after the warning is confirmed', async () => {
    getMock.mockResolvedValue({ access: 'sandbox' });
    setMock.mockResolvedValue({ access: 'full' });
    render(<ProjectValidationsAccessSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-validations-access-sandbox')).toBeChecked());

    fireEvent.click(screen.getByTestId('project-validations-access-full'));
    expect(screen.getByTestId('project-validations-access-full-warning')).toBeInTheDocument();
    expect(setMock).not.toHaveBeenCalled();
    expect(screen.getByTestId('project-validations-access-sandbox')).toBeChecked();

    fireEvent.click(screen.getByText('projects.validationsAccess.cancel'));
    expect(screen.queryByTestId('project-validations-access-full-warning')).toBeNull();
    expect(setMock).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTestId('project-validations-access-full'));
    fireEvent.click(screen.getByTestId('project-validations-access-full-confirm'));
    await waitFor(() => expect(screen.getByTestId('project-validations-access-full')).toBeChecked());
    expect(setMock).toHaveBeenCalledWith('p1', 'full');
    expect(screen.queryByTestId('project-validations-access-full-warning')).toBeNull();
  });

  it('says why no Docker socket is usable', async () => {
    getMock.mockResolvedValue({ access: 'sandbox', docker_error: 'no Docker socket found on this host' });
    render(<ProjectValidationsAccessSetting projectId="p1" t={t} />);
    await waitFor(() =>
      expect(screen.getByTestId('project-validations-access-socket')).toHaveTextContent('no Docker socket found'),
    );
  });
});
