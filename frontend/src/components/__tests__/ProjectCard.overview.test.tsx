import { beforeEach, describe, it, expect, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock({
  projects: {
    gitStatus: vi.fn().mockResolvedValue({
      branch: 'main',
      default_branch: 'main',
      is_default_branch: true,
      files: [{ path: 'src/wip.ts', status: 'modified', staged: false }],
      committed_files: [],
      ahead: 0,
      behind: 0,
      has_upstream: true,
      upstream: 'origin/main',
      provider: 'github',
      remote_url: 'https://github.com/team/demo',
      pull_requests_url: 'https://github.com/team/demo/pulls',
      last_tag: 'v0.9.0',
      pr_url: null,
      languages: [
        { language: 'Rust', bytes: 700 },
        { language: 'TypeScript', bytes: 300 },
      ],
      languages_checked_at: '2026-07-24T20:15:00Z',
      languages_cached: true,
    }),
    dependencyUpdates: vi.fn().mockResolvedValue({
      managers: [
        {
          manager: 'JS / TS',
          manifest: 'package.json',
          status: 'UpdatesAvailable',
          outdated: 2,
          major: 1,
          packages: [
            { name: 'react', current: '18.3.1', latest: '19.1.0', major: true },
            { name: 'vite', current: '6.0.0', latest: '6.1.0', major: false },
          ],
        },
        {
          manager: 'Composer',
          manifest: 'application/composer.json',
          status: 'Unavailable',
          outdated: 0,
          major: 0,
          packages: [],
        },
        {
          manager: 'Gradle',
          manifest: 'settings.gradle',
          status: 'Unsupported',
          outdated: 0,
          major: 0,
          packages: [],
        },
      ],
      total_outdated: 2,
      total_major: 1,
      checked_at: '2026-07-24T10:00:00Z',
      cached: false,
      monitoring_interval_days: 7,
      next_check_at: '2026-07-31T10:00:00Z',
    }),
    setDependencyMonitoring: vi.fn().mockResolvedValue({ interval_days: 30 }),
  },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    locale: 'fr',
    t: (key: string, ...args: (string | number)[]) =>
      args.reduce<string>(
        (label, arg, index) => label.replace(`{${index}}`, String(arg)),
        key,
      ),
  }),
}));
vi.mock('../../hooks/useMediaQuery', () => ({ useIsMobile: () => false }));
vi.mock('../ProjectCodePanel', () => ({
  ProjectCodePanel: ({ projectId, initialPath }: { projectId: string; initialPath?: string | null }) => (
    <div data-testid="project-code-panel">{projectId}:{initialPath ?? 'root'}</div>
  ),
}));
vi.mock('../ProjectDockerPanel', () => ({
  ProjectDockerPanel: ({ projectId, onOpenConfig }: {
    projectId: string;
    onOpenConfig: (path: string) => void;
  }) => (
    <div data-testid="project-docker-panel">
      {projectId}
      <button type="button" onClick={() => onOpenConfig('compose.yaml')}>open compose</button>
    </div>
  ),
}));

import { ProjectCard } from '../ProjectCard';
import { planning as planningApi, projects as projectsApi } from '../../lib/api';
import type { Discussion, Project } from '../../types/generated';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-overview',
  name: 'Demo',
  path: '/repos/demo',
  repo_url: 'git@github.com:team/demo.git',
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0,
  tech_debt_count: 0,
  needs_docs_migration: false,
  path_exists: true,
  write_access: { status: 'Writable' },
  mcp_sync_report: null,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

describe('ProjectCard — repository overview', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.removeItem('kronn:projectDetailView');
    sessionStorage.clear();
  });

  it('separates audit and docs, keeps the active tab, and consumes an audit deep-link', async () => {
    const renderCard = (project: Project) => render(
      <ProjectCard
        project={project}
        dockerRunning
        detailMode
        isOpen
        onToggleOpen={noop}
        discussions={[]}
        driftStatus={undefined}
        agents={[]}
        allSkills={[]}
        mcpConfigs={[]}
        workflows={[]}
        configLanguage="fr"
        toast={vi.fn()}
        onNavigate={noop}
        onSetDiscPrefill={noop}
        onAutoRunDiscussion={noop}
        onOpenDiscussion={noop}
        onRefetch={noop}
        onRefetchDiscussions={noop}
        onRefetchSkills={noop}
        onRefetchDrift={noop}
      />,
    );

    const first = renderCard(PROJECT);
    expect(document.querySelector('.project-detail-header')).toHaveClass('collection-detail-header');
    expect(screen.getByText('projects.master.dockerUp')).toBeInTheDocument();
    const auditTab = screen.getByRole('button', { name: 'projects.master.tab.audit' });
    const docsTab = screen.getByRole('button', { name: 'projects.master.tab.docs' });
    const detailBody = document.querySelector('.dash-card-body');

    fireEvent.click(auditTab);
    expect(auditTab).toHaveAttribute('data-active', 'true');
    expect(detailBody).toHaveAttribute('data-detail-view', 'audit');
    const auditSection = document.querySelector('[data-project-view="audit"]');
    expect(auditSection).toBeInTheDocument();
    expect(auditSection?.querySelector('.dash-collapsible-header')).not.toBeInTheDocument();
    expect(auditSection?.querySelector('.dash-audit-pad')).toBeInTheDocument();

    fireEvent.click(docsTab);
    expect(docsTab).toHaveAttribute('data-active', 'true');
    expect(detailBody).toHaveAttribute('data-detail-view', 'docs');
    const docsSection = document.querySelector('[data-project-view="docs"]');
    expect(docsSection).toBeInTheDocument();
    expect(docsSection?.querySelector('.dash-collapsible-header')).not.toBeInTheDocument();
    expect(docsSection?.querySelector('.aidoc-loading, .aidoc-root, .aidoc-empty'))
      .toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'projects.master.tab.code' }));
    expect(screen.getByRole('button', { name: 'projects.master.tab.code' }))
      .toHaveAttribute('data-active', 'true');

    fireEvent.click(screen.getByRole('button', { name: 'projects.master.tab.docker' }));
    expect(detailBody).toHaveAttribute('data-detail-view', 'docker');
    expect(screen.getByTestId('project-docker-panel')).toHaveTextContent(PROJECT.id);

    fireEvent.click(screen.getByRole('button', { name: 'open compose' }));
    expect(detailBody).toHaveAttribute('data-detail-view', 'code');
    expect(screen.getByTestId('project-code-panel')).toHaveTextContent('compose.yaml');
    first.unmount();

    const second = renderCard({ ...PROJECT, id: 'p-second', name: 'Second project' });
    expect(screen.getByRole('button', { name: 'projects.master.tab.code' }))
      .toHaveAttribute('data-active', 'true');
    second.unmount();

    localStorage.setItem('kronn:projectDetailView', 'removed-tab');
    const third = renderCard({ ...PROJECT, id: 'p-third', name: 'Third project' });
    expect(screen.getByRole('button', { name: 'projects.master.tab.overview' }))
      .toHaveAttribute('data-active', 'true');
    third.unmount();

    sessionStorage.setItem('kronn:projectView:p-deep', 'audit');
    renderCard({ ...PROJECT, id: 'p-deep', name: 'Deep-linked project' });
    await waitFor(() => expect(screen.getByRole('button', { name: 'projects.master.tab.audit' }))
      .toHaveAttribute('data-active', 'true'));
    expect(sessionStorage.getItem('kronn:projectView:p-deep')).toBeNull();
  });

  it('makes a project outside the write perimeter explicit', () => {
    render(
      <ProjectCard
        project={{
          ...PROJECT,
          write_access: {
            status: 'ReadOnly',
            reason: 'outside_rw_perimeter',
            writable_roots: ['/repos'],
          },
        }}
        detailMode
        isOpen
        onToggleOpen={noop}
        discussions={[]}
        driftStatus={undefined}
        agents={[]}
        allSkills={[]}
        mcpConfigs={[]}
        workflows={[]}
        configLanguage="fr"
        toast={vi.fn()}
        onNavigate={noop}
        onSetDiscPrefill={noop}
        onAutoRunDiscussion={noop}
        onOpenDiscussion={noop}
        onRefetch={noop}
        onRefetchDiscussions={noop}
        onRefetchSkills={noop}
        onRefetchDrift={noop}
      />,
    );
    expect(screen.getByTestId(`write-access-banner-${PROJECT.id}`)).toBeInTheDocument();
    expect(screen.getByText('projects.writeAccess.readOnlyTitle')).toBeInTheDocument();
    expect(screen.getByText('projects.writeAccess.copyMove')).toBeInTheDocument();
    expect(screen.getByText('projects.writeAccess.copyExtra')).toBeInTheDocument();
  });

  it('summarizes context drift and reveals technical audit details on demand', async () => {
    vi.mocked(projectsApi.auditEvidence).mockResolvedValueOnce({
      project_id: PROJECT.id,
      status: 'TemplateInstalled',
      kind: 'missing_evidence',
      state_file: 'docs/.kronn.json',
      runtime_workspace: '.kronn/',
      audit_runs: 2,
      interrupted_runs: 1,
      interruption_rate_percent: 50,
      resumable_after_step: 4,
    });
    vi.mocked(projectsApi.contextAudit).mockResolvedValueOnce({
      project_id: PROJECT.id,
      audit: { files: [], per_agent: [], proposal: [], findings: [], no_convention_found: false },
      rendered: '',
      drift: {
        grown: [['AGENTS.md', 420]],
        new_files: [],
        newly_broken_routes: ['docs/missing.md'],
        unused_files: ['frontend/AGENTS.md'],
        paid_agent_growth: [{
          agent: 'Codex', previous_bytes: 1000, current_bytes: 1420, delta_bytes: 420,
        }],
      },
    });
    vi.mocked(projectsApi.attestDocumentation).mockResolvedValueOnce({
      project_id: PROJECT.id,
      status: 'Audited',
      kind: 'human_attestation',
      state_file: 'docs/.kronn.json',
      runtime_workspace: '.kronn/',
      audit_runs: 2,
      interrupted_runs: 1,
      interruption_rate_percent: 50,
      resumable_after_step: 4,
    });
    vi.mocked(projectsApi.acceptContextBaseline).mockResolvedValueOnce({
      project_id: PROJECT.id,
      audit: { files: [], per_agent: [], proposal: [], findings: [], no_convention_found: false },
      rendered: '',
      drift: {
        grown: [], new_files: [], newly_broken_routes: [], unused_files: [], paid_agent_growth: [],
      },
    });
    const confirm = vi.fn().mockReturnValue(true);
    vi.stubGlobal('confirm', confirm);
    const toast = vi.fn();
    const onRefetch = vi.fn();

    render(
      <ProjectCard
        project={PROJECT}
        detailMode
        isOpen
        onToggleOpen={noop}
        discussions={[]}
        driftStatus={undefined}
        agents={[]}
        allSkills={[]}
        mcpConfigs={[]}
        workflows={[]}
        configLanguage="fr"
        toast={toast}
        onNavigate={noop}
        onSetDiscPrefill={noop}
        onAutoRunDiscussion={noop}
        onOpenDiscussion={noop}
        onRefetch={onRefetch}
        onRefetchDiscussions={noop}
        onRefetchSkills={noop}
        onRefetchDrift={noop}
      />,
    );

    expect(await screen.findByText('projects.contextAudit.evidence.missing_evidence'))
      .toBeInTheDocument();
    expect(screen.getByText('projects.contextAudit.growthSummary')).toBeInTheDocument();
    expect(screen.getByText('projects.contextAudit.growthHelp')).toBeInTheDocument();
    expect(screen.queryByText('projects.contextAudit.whyHelp')).not.toBeInTheDocument();
    const contextHelp = screen.getByRole('button', { name: 'projects.contextAudit.whyTitle' });
    expect(contextHelp).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(contextHelp);
    expect(contextHelp).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('projects.contextAudit.whyHelp')).toBeInTheDocument();
    expect(screen.getByText('projects.contextAudit.brokenSummary')).toBeInTheDocument();
    expect(screen.getByText('projects.contextAudit.orphanSummary')).toBeInTheDocument();
    expect(screen.queryByText('projects.contextAudit.paidGrowth')).not.toBeInTheDocument();

    const details = screen.getByRole('button', { name: 'projects.contextAudit.showDetails' });
    expect(details).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(details);
    expect(screen.getByRole('button', { name: 'projects.contextAudit.hideDetails' }))
      .toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('projects.contextAudit.paidGrowth')).toBeInTheDocument();
    expect(screen.getByText('projects.contextAudit.brokenRoute')).toBeInTheDocument();
    expect(screen.getByText('projects.contextAudit.orphanFile')).toBeInTheDocument();
    expect(screen.getByText('projects.contextAudit.resume')).toBeInTheDocument();

    const acceptBaseline = screen.getByRole('button', { name: 'projects.contextAudit.acceptBaseline' });
    expect(acceptBaseline).toHaveClass('project-context-audit-accept');
    expect(acceptBaseline).toHaveAttribute('title', 'projects.contextAudit.acceptBaselineHelp');
    fireEvent.click(acceptBaseline);
    await waitFor(() => expect(projectsApi.acceptContextBaseline).toHaveBeenCalledWith(PROJECT.id));
    expect(await screen.findByText('projects.contextAudit.stable')).toBeInTheDocument();
    expect(toast).toHaveBeenCalledWith('projects.contextAudit.baselineAccepted', 'success');

    fireEvent.click(screen.getByRole('button', { name: 'projects.contextAudit.attest' }));
    await waitFor(() => expect(projectsApi.attestDocumentation).toHaveBeenCalledWith(PROJECT.id));
    expect(confirm).toHaveBeenCalledWith('projects.contextAudit.attestConfirm');
    expect(await screen.findByText('projects.contextAudit.evidence.human_attestation'))
      .toBeInTheDocument();
    expect(onRefetch).toHaveBeenCalled();
    expect(toast).toHaveBeenCalledWith('projects.contextAudit.attested', 'success');
    vi.unstubAllGlobals();
  });

  it('shows repository and dependency health, then allows a forced refresh', async () => {
    render(
      <ProjectCard
        project={PROJECT}
        detailMode
        isOpen
        onToggleOpen={noop}
        discussions={[]}
        driftStatus={undefined}
        agents={[]}
        allSkills={[]}
        mcpConfigs={[]}
        workflows={[]}
        configLanguage="fr"
        toast={vi.fn()}
        onNavigate={noop}
        onSetDiscPrefill={noop}
        onAutoRunDiscussion={noop}
        onOpenDiscussion={noop}
        onRefetch={noop}
        onRefetchDiscussions={noop}
        onRefetchSkills={noop}
        onRefetchDrift={noop}
      />,
    );

    await waitFor(() => expect(projectsApi.gitStatus).toHaveBeenCalledWith('p-overview'));
    await waitFor(() => expect(projectsApi.dependencyUpdates).toHaveBeenCalledWith('p-overview'));
    expect(await screen.findByText('v0.9.0')).toBeInTheDocument();
    expect(screen.getByText('projects.master.overview.upToDate')).toBeInTheDocument();
    expect(screen.getByText('projects.master.overview.localChanges')).toBeInTheDocument();
    expect(screen.getByText('Rust')).toBeInTheDocument();
    expect(screen.getByText('70.0 %')).toBeInTheDocument();
    expect(screen.getByText('projects.master.overview.languagesCachedShort')).toBeInTheDocument();
    expect(screen.getByText('JS / TS')).toBeInTheDocument();
    expect(screen.getByText('Composer')).toBeInTheDocument();
    expect(screen.getByText('Gradle')).toBeInTheDocument();
    expect(screen.getByText(/react 18\.3\.1 → 19\.1\.0/)).toBeInTheDocument();
    expect(screen.getByText('projects.master.overview.dependencyToolUnavailable')).toBeInTheDocument();
    expect(screen.getByText('projects.master.overview.dependencyUnsupported')).toBeInTheDocument();
    expect(document.querySelectorAll('.project-overview-dependency-major').length).toBeGreaterThan(0);
    expect(document.querySelector('.project-overview-dependency-package-major'))
      .toHaveTextContent(/react 18\.3\.1 → 19\.1\.0/);
    expect(screen.getByText(/projects\.master\.overview\.dependenciesCheckedAt/))
      .toBeInTheDocument();
    expect(screen.getByText(/projects\.master\.overview\.dependenciesNextCheckAt/))
      .toBeInTheDocument();

    expect(screen.getByRole('link', { name: /projects\.master\.overview\.openRepository/ }))
      .toHaveAttribute('href', 'https://github.com/team/demo');
    expect(screen.getByRole('link', { name: /projects\.master\.overview\.pullRequests/ }))
      .toHaveAttribute('href', 'https://github.com/team/demo/pulls');

    const languageRefresh = screen.getByRole('button', {
      name: 'projects.master.overview.languagesRefresh',
    });
    fireEvent.click(languageRefresh);
    fireEvent.click(languageRefresh);
    await waitFor(() => expect(projectsApi.gitStatus).toHaveBeenCalledWith('p-overview', true));
    expect(projectsApi.gitStatus).toHaveBeenCalledTimes(2);

    const refreshButton = screen.getByRole('button', {
      name: 'projects.master.overview.dependenciesRefresh',
    });
    fireEvent.click(refreshButton);
    fireEvent.click(refreshButton);
    await waitFor(() =>
      expect(projectsApi.dependencyUpdates).toHaveBeenCalledWith('p-overview', true),
    );
    expect(projectsApi.dependencyUpdates).toHaveBeenCalledTimes(2);

    fireEvent.change(
      screen.getByLabelText('projects.master.overview.dependenciesSchedule'),
      { target: { value: '30' } },
    );
    await waitFor(() =>
      expect(projectsApi.setDependencyMonitoring).toHaveBeenCalledWith('p-overview', 30),
    );
    await waitFor(() => expect(projectsApi.dependencyUpdates).toHaveBeenCalledTimes(3));
  });

  it('puts discussion/task counts in tabs and paginates recent discussions', async () => {
    const discussions = Array.from({ length: 65 }, (_, index) => ({
      id: `disc-${index}`,
      project_id: PROJECT.id,
      title: `Discussion ${index}`,
      agent: 'Codex',
      messages: [],
      message_count: index,
      non_system_message_count: index,
      archived: false,
      updated_at: new Date(2026, 0, index + 1).toISOString(),
    })) as unknown as Discussion[];
    vi.mocked(planningApi.list).mockResolvedValueOnce({
      items: Array.from({ length: 4 }, () => ({})),
      next_cursor: null,
    } as never);

    render(
      <ProjectCard
        project={PROJECT}
        detailMode
        isOpen
        onToggleOpen={noop}
        discussions={discussions}
        driftStatus={undefined}
        agents={[]}
        allSkills={[]}
        mcpConfigs={[]}
        workflows={[]}
        configLanguage="fr"
        toast={vi.fn()}
        onNavigate={noop}
        onSetDiscPrefill={noop}
        onAutoRunDiscussion={noop}
        onOpenDiscussion={noop}
        onRefetch={noop}
        onRefetchDiscussions={noop}
        onRefetchSkills={noop}
        onRefetchDrift={noop}
      />,
    );

    const discussionsTab = screen.getByRole('button', {
      name: /projects\.master\.tab\.discussions 65/,
    });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /projects\.master\.tab\.tasks 4/ }))
        .toBeInTheDocument();
    });

    fireEvent.click(discussionsTab);
    expect(screen.getByText('Discussion 64')).toBeInTheDocument();
    expect(screen.getByText('Discussion 55')).toBeInTheDocument();
    expect(screen.queryByText('Discussion 54')).not.toBeInTheDocument();

    const amount = screen.getByLabelText('projects.master.discussions.loadAmountLabel');
    expect(screen.getByRole('option', { name: '10' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: '50' })).toBeInTheDocument();
    expect(screen.getByRole('option', {
      name: 'projects.master.discussions.loadAll',
    })).toBeInTheDocument();

    fireEvent.change(amount, { target: { value: '50' } });
    fireEvent.click(screen.getByText('projects.master.discussions.loadPrefix'));
    expect(screen.getByText('Discussion 5')).toBeInTheDocument();
    expect(screen.queryByText('Discussion 4')).not.toBeInTheDocument();

    fireEvent.click(amount);
    expect(screen.queryByText('Discussion 4')).not.toBeInTheDocument();

    fireEvent.change(amount, { target: { value: 'all' } });
    fireEvent.click(screen.getByText('projects.master.discussions.loadSuffix'));
    expect(screen.getByText('Discussion 0')).toBeInTheDocument();
    expect(screen.queryByText('projects.master.tab.discussions', { selector: '.dash-section-title' }))
      .not.toBeInTheDocument();
  });

  it('opens a project task directly in global planning', async () => {
    vi.mocked(planningApi.list).mockResolvedValue({
      items: [{
        id: 'task-42',
        reference: 'KT-42',
        parent_id: null,
        parent_reference: null,
        parent_title: null,
        title: 'Review release',
        status: 'todo',
        priority: 'high',
        rank: 1024,
        completed_subtasks: 0,
        total_subtasks: 0,
        project_ids: [PROJECT.id],
        discussion_ids: [],
        tags: [],
        blocker_count: 0,
        created_at: '2026-07-25T00:00:00Z',
        updated_at: '2026-07-25T00:00:00Z',
      }],
      next_cursor: null,
    });
    const onNavigate = vi.fn();

    render(
      <ProjectCard
        project={PROJECT}
        detailMode
        isOpen
        onToggleOpen={noop}
        discussions={[]}
        driftStatus={undefined}
        agents={[]}
        allSkills={[]}
        mcpConfigs={[]}
        workflows={[]}
        configLanguage="fr"
        toast={vi.fn()}
        onNavigate={onNavigate}
        onSetDiscPrefill={noop}
        onAutoRunDiscussion={noop}
        onOpenDiscussion={noop}
        onRefetch={noop}
        onRefetchDiscussions={noop}
        onRefetchSkills={noop}
        onRefetchDrift={noop}
      />,
    );

    fireEvent.click(await screen.findByRole('button', {
      name: /projects\.master\.tab\.tasks 1/,
    }));
    fireEvent.click(await screen.findByRole('button', {
      name: 'projects.tasks.openTask',
    }));

    expect(onNavigate).toHaveBeenCalledWith('planning:task-42');
  });
});
