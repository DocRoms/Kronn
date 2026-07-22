import { describe, it, expect, vi } from 'vitest';
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
    }),
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

import { ProjectCard } from '../ProjectCard';
import { projects as projectsApi } from '../../lib/api';
import type { Project } from '../../types/generated';

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
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

describe('ProjectCard — repository overview', () => {
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
  });
});
