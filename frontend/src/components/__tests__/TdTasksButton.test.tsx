// « En faire des tâches Kronn » (KT-1167): the TD sheets become tasks; a
// double click fires one request, the counts reach the toast.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../hooks/useMediaQuery', () => ({ useIsMobile: () => false }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));

import { TdTasksButton } from '../TdTasksButton';
import { ProjectCard } from '../ProjectCard';
import { projects as projectsApi } from '../../lib/api';
import type { AgentDetection, Project, TdTasksOutcome } from '../../types/generated';

const PROJECT: Project = {
  id: 'p-td',
  name: 'td-target',
  path: '/repos/td-target',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'Validated',
  ai_todo_count: 0, tech_debt_count: 3, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const OUTCOME: TdTasksOutcome = {
  created: 2, already_linked: 1, skipped: 1,
  themes: [{ theme: 'security', label: 'security', task_reference: 'KT-10', created: true }],
  items: [],
};

const convert = () => projectsApi.convertTdTasks as ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.clearAllMocks();
});

describe('TdTasksButton', () => {
  it('converts once and reports the counts', async () => {
    let release: (v: TdTasksOutcome) => void = () => {};
    convert().mockReturnValueOnce(new Promise<TdTasksOutcome>(r => { release = r; }));
    const toast = vi.fn();
    render(<TdTasksButton project={PROJECT} auditActive={false} toast={toast} />);

    const button = screen.getByTestId('project-td-tasks');
    fireEvent.click(button);
    fireEvent.click(button);
    expect(convert()).toHaveBeenCalledTimes(1);
    expect(convert()).toHaveBeenCalledWith('p-td');

    release(OUTCOME);
    await waitFor(() => expect(toast).toHaveBeenCalledWith('audit.tdTasks.done 2 1 1', 'success'));
  });

  it('reports a re-click that created nothing as info', async () => {
    convert().mockResolvedValueOnce({ ...OUTCOME, created: 0, already_linked: 3 });
    const toast = vi.fn();
    render(<TdTasksButton project={PROJECT} auditActive={false} toast={toast} />);
    fireEvent.click(screen.getByTestId('project-td-tasks'));
    await waitFor(() => expect(toast).toHaveBeenCalledWith('audit.tdTasks.done 0 3 1', 'info'));
  });

  it('surfaces a failure', async () => {
    convert().mockRejectedValueOnce(new Error('boom'));
    const toast = vi.fn();
    render(<TdTasksButton project={PROJECT} auditActive={false} toast={toast} />);
    fireEvent.click(screen.getByTestId('project-td-tasks'));
    await waitFor(() => expect(toast).toHaveBeenCalledWith('audit.tdTasks.error boom', 'error'));
  });

  it('is hidden without tech debt or while an audit runs', () => {
    const { rerender } = render(
      <TdTasksButton project={{ ...PROJECT, tech_debt_count: 0 }} auditActive={false} toast={vi.fn()} />,
    );
    expect(screen.queryByTestId('project-td-tasks')).toBeNull();
    rerender(<TdTasksButton project={PROJECT} auditActive toast={vi.fn()} />);
    expect(screen.queryByTestId('project-td-tasks')).toBeNull();
  });
});

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true,
  path: '/usr/bin/claude', version: '1.0.0', latest_version: null, origin: 'host',
  install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

describe('ProjectCard — En faire des tâches Kronn', () => {
  it('offers the action on a card with tech debt', async () => {
    vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(null);
    render(<ProjectCard {...({
      project: PROJECT, isOpen: true, onToggleOpen: () => {}, discussions: [],
      driftStatus: undefined, agents: [AGENT], allSkills: [], mcpConfigs: [],
      workflows: [], configLanguage: 'fr',
      toast: vi.fn(), onNavigate: vi.fn(), onSetDiscPrefill: () => {},
      onAutoRunDiscussion: vi.fn(), onOpenDiscussion: vi.fn(),
      onRefetch: vi.fn(), onRefetchDiscussions: vi.fn(), onRefetchSkills: () => {},
      onRefetchDrift: vi.fn(),
    } as unknown as React.ComponentProps<typeof ProjectCard>)} />);
    expect(await screen.findByTestId('project-td-tasks')).toBeInTheDocument();
  });
});
