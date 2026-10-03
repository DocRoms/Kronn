import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

const { agentFilesMock, setAgentFilesMock } = vi.hoisted(() => ({
  agentFilesMock: vi.fn(),
  setAgentFilesMock: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  projects: { agentFiles: agentFilesMock as never, setAgentFiles: setAgentFilesMock as never },
}));

import { ProjectAgentFilesSetting } from '../ProjectAgentFilesSetting';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ProjectAgentFilesSetting (KT-971)', () => {
  it('shows where the files go and moves them outside the repository on demand', async () => {
    agentFilesMock.mockResolvedValue({ policy: 'repo' });
    setAgentFilesMock.mockResolvedValue({
      policy: 'outside',
      outside_dir: '/data/project-agent-files/3f2a',
      cleaned: ['.mcp.json', '.gemini/settings.json'],
    });
    render(<ProjectAgentFilesSetting projectId="p1" t={t} />);

    await waitFor(() => expect(screen.getByTestId('project-agent-files-repo')).toBeChecked());
    expect(screen.queryByTestId('project-agent-files-where')).toBeNull();

    fireEvent.click(screen.getByTestId('project-agent-files-outside'));
    await waitFor(() => expect(screen.getByTestId('project-agent-files-outside')).toBeChecked());
    expect(setAgentFilesMock).toHaveBeenCalledWith('p1', 'outside');
    expect(screen.getByTestId('project-agent-files-where')).toHaveTextContent('/data/project-agent-files/3f2a');
    expect(screen.getByTestId('project-agent-files-cleaned'))
      .toHaveTextContent('projects.agentFiles.cleaned:.mcp.json, .gemini/settings.json');
  });

  it('keeps the current choice and says why when the change fails', async () => {
    agentFilesMock.mockResolvedValue({ policy: 'repo' });
    setAgentFilesMock.mockRejectedValue(new Error('Project not found'));
    render(<ProjectAgentFilesSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-agent-files-repo')).toBeChecked());

    fireEvent.click(screen.getByTestId('project-agent-files-outside'));
    await waitFor(() => expect(screen.getByRole('alert')).toBeInTheDocument());
    expect(screen.getByTestId('project-agent-files-repo')).toBeChecked();
  });

  it('fails alone, never taking the project card down with it', async () => {
    agentFilesMock.mockImplementation(() => { throw new Error('unavailable'); });
    render(<ProjectAgentFilesSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByRole('alert')).toBeInTheDocument());
    expect(screen.getByTestId('project-agent-files')).toBeInTheDocument();
  });
});
