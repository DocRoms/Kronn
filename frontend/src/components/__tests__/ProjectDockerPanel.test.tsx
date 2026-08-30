import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { projects } from '../../lib/api';
import type { ProjectDockerStatus } from '../../types/generated';
import { ProjectDockerPanel } from '../ProjectDockerPanel';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    locale: 'fr',
    t: (key: string, ...args: (string | number)[]) =>
      args.reduce<string>(
        (label, arg, index) => `${label.replace(`{${index}}`, String(arg))} ${arg}`,
        key,
      ),
  }),
}));

vi.mock('../../lib/api', () => ({
  projects: {
    dockerStatus: vi.fn(),
    dockerAction: vi.fn(),
    dockerLogs: vi.fn(),
  },
}));

const emptyStatus: ProjectDockerStatus = {
  compose_present: false,
  compose_file: null,
  docker_available: false,
  daemon_available: false,
  services: [],
  checked_at: '2026-08-30T08:00:00Z',
  error: null,
};

const composeStatus: ProjectDockerStatus = {
  compose_present: true,
  compose_file: 'compose.yaml',
  docker_available: true,
  daemon_available: true,
  services: [
    {
      service: 'web',
      container_name: 'demo-web-1',
      image: 'nginx:alpine',
      state: 'running',
      status: 'Up 2 minutes',
      health: 'healthy',
      ports: ['0.0.0.0:8080 → 80/tcp'],
      endpoints: [
        {
          url: 'http://demo.local:8080',
          host: 'demo.local',
          host_status: 'missing',
        },
      ],
      running: true,
    },
    {
      service: 'worker',
      container_name: null,
      image: null,
      state: 'not_created',
      status: null,
      health: null,
      ports: [],
      endpoints: [],
      running: false,
    },
  ],
  checked_at: '2026-08-30T08:00:00Z',
  error: null,
};

describe('ProjectDockerPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(projects.dockerStatus).mockResolvedValue(composeStatus);
    vi.mocked(projects.dockerAction).mockResolvedValue(composeStatus);
    vi.mocked(projects.dockerLogs).mockResolvedValue({
      service: 'web',
      output: '2026-08-30T08:00:00Z web ready',
      fetched_at: '2026-08-30T08:00:01Z',
    });
  });

  it('explains how to enable the tab when the project has no Compose file', async () => {
    vi.mocked(projects.dockerStatus).mockResolvedValueOnce(emptyStatus);

    render(<ProjectDockerPanel projectId="project-1" toast={vi.fn()} onOpenConfig={vi.fn()} />);

    expect(await screen.findByText('projects.docker.noComposeTitle')).toBeInTheDocument();
    expect(projects.dockerStatus).toHaveBeenCalledWith('project-1');
  });

  it('lists running and uncreated services with their useful runtime details', async () => {
    const onRunningChange = vi.fn();
    render(
      <ProjectDockerPanel
        projectId="project-1"
        toast={vi.fn()}
        onOpenConfig={vi.fn()}
        onRunningChange={onRunningChange}
      />,
    );

    expect(await screen.findByText('web')).toBeInTheDocument();
    expect(screen.getByText('worker')).toBeInTheDocument();
    expect(screen.getByText('demo-web-1')).toBeInTheDocument();
    expect(screen.getByText('0.0.0.0:8080 → 80/tcp')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /http:\/\/demo\.local:8080/ })).toHaveAttribute(
      'href',
      'http://demo.local:8080',
    );
    expect(screen.getByLabelText('projects.docker.hostMissing')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'projects.docker.startAll' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'projects.docker.stopAll' })).toBeEnabled();
    expect(onRunningChange).toHaveBeenCalledWith(true);
  });

  it('opens recent logs for a running service', async () => {
    render(<ProjectDockerPanel projectId="project-1" toast={vi.fn()} onOpenConfig={vi.fn()} />);

    await screen.findByText('web');
    fireEvent.click(screen.getByRole('button', { name: 'projects.docker.logs' }));

    expect(await screen.findByRole('dialog')).toBeInTheDocument();
    expect(projects.dockerLogs).toHaveBeenCalledWith('project-1', 'web');
    expect(screen.getByText(/web ready/)).toBeInTheDocument();
  });

  it('opens the detected Compose file through the project Code view', async () => {
    const onOpenConfig = vi.fn();
    render(
      <ProjectDockerPanel
        projectId="project-1"
        toast={vi.fn()}
        onOpenConfig={onOpenConfig}
      />,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'projects.docker.openConfig' }));

    expect(onOpenConfig).toHaveBeenCalledWith('compose.yaml');
  });

  it('stops one service and replaces the displayed snapshot', async () => {
    const toast = vi.fn();
    vi.mocked(projects.dockerAction).mockResolvedValueOnce({
      ...composeStatus,
      services: composeStatus.services.map(service => service.service === 'web'
        ? { ...service, running: false, state: 'exited', status: 'Exited (0)' }
        : service),
    });
    render(<ProjectDockerPanel projectId="project-1" toast={toast} onOpenConfig={vi.fn()} />);

    await screen.findByText('web');
    fireEvent.click(screen.getByRole('button', { name: 'projects.docker.stop' }));

    await waitFor(() => {
      expect(projects.dockerAction).toHaveBeenCalledWith('project-1', 'stop', 'web');
      expect(screen.getByText('Exited (0)')).toBeInTheDocument();
      expect(toast).toHaveBeenCalledWith(expect.stringContaining('projects.docker.actionSuccess'), 'success');
    });
  });

  it('keeps Docker action errors visible and reports them through the page toast', async () => {
    const toast = vi.fn();
    vi.mocked(projects.dockerAction).mockRejectedValueOnce(new Error('daemon offline'));
    render(<ProjectDockerPanel projectId="project-1" toast={toast} onOpenConfig={vi.fn()} />);

    await screen.findByText('web');
    fireEvent.click(screen.getByRole('button', { name: 'projects.docker.stop' }));

    expect(await screen.findByText('daemon offline')).toBeInTheDocument();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining('daemon offline'), 'error');
  });
});
