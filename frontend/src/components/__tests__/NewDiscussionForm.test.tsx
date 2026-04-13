import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, act, waitFor } from '@testing-library/react';
import { NewDiscussionForm } from '../NewDiscussionForm';
import type { Project, AgentDetection } from '../../types/generated';

vi.mock('../../lib/api', () => ({
  skills: { list: vi.fn().mockResolvedValue([]) },
  profiles: { list: vi.fn().mockResolvedValue([]) },
  directives: { list: vi.fn().mockResolvedValue([]) },
}));

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

const PROJECT_WITH_REPO: Project = {
  id: 'proj-git', name: 'front_euronews',
  path: '/repos/front_euronews',
  repo_url: 'git@github.com:Euronews-tech/front_euronews.git',
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const PROJECT_WITHOUT_REPO: Project = {
  id: 'proj-local', name: 'local-notes',
  path: '/repos/local-notes',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const AGENT: AgentDetection = {
  name: 'Claude Code',
  agent_type: 'ClaudeCode',
  installed: true,
  enabled: true,
  path: '/usr/bin/claude',
  version: '1.0.0',
  latest_version: null,
  origin: 'host',
  install_command: null,
  host_managed: false,
  host_label: null,
  runtime_available: false,
};

const mount = (projects: Project[]) => {
  const onSubmit = vi.fn();
  return render(
    <NewDiscussionForm
      projects={projects}
      agents={[AGENT]}
      configLanguage="fr"
      agentAccess={null}
      onSubmit={onSubmit}
      onClose={vi.fn()}
      onNavigate={vi.fn()}
      t={(key: string) => key}
    />
  );
};

describe('NewDiscussionForm — workspace toggle', () => {
  it('shows the workspace toggle (Direct / Isolated) when the selected project has a repo_url', async () => {
    // Regression guard for 2026-04-13: user reported the Isolated option
    // disappeared. Root cause would be either the `selectedProj?.repo_url`
    // check or the condition wrapping the whole block — both are tested here.
    mount([PROJECT_WITH_REPO]);

    // Project dropdown shows our repo-backed project — select it
    const projectSelect = screen.getAllByRole('combobox')[0];
    await act(async () => {
      fireEvent.change(projectSelect, { target: { value: PROJECT_WITH_REPO.id } });
    });

    // The workspace-toggle container renders with both buttons (data-mode)
    await waitFor(() => {
      expect(document.querySelector('.disc-workspace-toggle')).not.toBeNull();
      expect(document.querySelector('.disc-workspace-btn[data-mode="direct"]')).not.toBeNull();
      expect(document.querySelector('.disc-workspace-btn[data-mode="isolated"]')).not.toBeNull();
    });
  });

  it('shows the toggle but disables Isolated for projects without a repo_url', async () => {
    // Non-regression: for non-git projects we still display the toggle
    // so users see the option exists — but Isolated is disabled with a
    // tooltip explaining why. Hiding it silently was the bug that made
    // Marie think the feature vanished.
    mount([PROJECT_WITHOUT_REPO]);
    const projectSelect = screen.getAllByRole('combobox')[0];
    await act(async () => {
      fireEvent.change(projectSelect, { target: { value: PROJECT_WITHOUT_REPO.id } });
    });
    expect(document.querySelector('.disc-workspace-toggle')).not.toBeNull();
    const isolatedBtn = document.querySelector('.disc-workspace-btn[data-mode="isolated"]') as HTMLButtonElement;
    expect(isolatedBtn).not.toBeNull();
    expect(isolatedBtn.disabled).toBe(true);
  });

  it('reveals the branch-name / base-branch inputs when the user picks Isolated', async () => {
    mount([PROJECT_WITH_REPO]);
    const projectSelect = screen.getAllByRole('combobox')[0];
    await act(async () => {
      fireEvent.change(projectSelect, { target: { value: PROJECT_WITH_REPO.id } });
    });
    await waitFor(() => {
      expect(document.querySelector('.disc-workspace-btn[data-mode="isolated"]')).not.toBeNull();
    });
    const isolatedBtn = document.querySelector('.disc-workspace-btn[data-mode="isolated"]') as HTMLButtonElement;
    await act(async () => {
      fireEvent.click(isolatedBtn);
    });
    await waitFor(() => {
      expect(document.querySelector('.disc-workspace-branch-grid')).not.toBeNull();
    });
  });
});
