// ProjectCard — the "AI & automation" tab carries the number of items waiting
// for a decision and hands the panel the links it needs (Git tab, keys page).

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';
import { listing, resource } from './repositoryResourceFixtures';

const repositoryResources = vi.hoisted(() => vi.fn());

vi.mock('../../lib/api', () => buildApiMock({ projects: { repositoryResources } }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) => (args.length ? `${key} ${args.map(String).join(' ')}` : key),
  }),
}));
vi.mock('../../hooks/useMediaQuery', () => ({ useIsMobile: () => false }));

import { ProjectCard } from '../ProjectCard';
import type { Project } from '../../types/generated';

const noop = () => {};
const PROJECT: Project = {
  id: 'p-1',
  name: 'demo',
  path: '/repos/demo',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

function renderCard(onNavigate = noop) {
  return render(
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
}

describe('ProjectCard — AI & automation tab', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.setItem('kronn:projectDetailView', 'resources');
  });

  it('shows how many items are waiting for a decision once the tab has loaded', async () => {
    repositoryResources.mockResolvedValue(listing({
      resources: [
        resource({ id: 'a', name: 'Clash', kind: 'workflow', status: 'conflict' }),
        resource({ id: 'b', name: 'Fresh', kind: 'quick_exec', status: 'repository_only' }),
        resource({ id: 'c', name: 'Same', kind: 'quick_prompt', status: 'up_to_date' }),
      ],
    }));
    renderCard();

    const tab = screen.getByRole('button', { name: /projects\.master\.tab\.resources/ });
    await waitFor(() => expect(within(tab).getByText('2')).toHaveClass('project-detail-tab-count'));
  });

  it('shows no count when nothing needs a decision', async () => {
    repositoryResources.mockResolvedValue(listing({
      resources: [resource({ id: 'c', name: 'Same', kind: 'quick_prompt', status: 'up_to_date' })],
    }));
    renderCard();

    await screen.findByRole('tablist');
    const tab = screen.getByRole('button', { name: /projects\.master\.tab\.resources/ });
    expect(tab.querySelector('.project-detail-tab-count')).toBeNull();
  });

  it('opens the Git tab from the uncommitted-files banner and the keys page from the approval sheet', async () => {
    const onNavigate = vi.fn();
    repositoryResources.mockResolvedValue(listing({
      uncommitted_managed_paths: ['kronn/a.md'],
      resources: [resource({
        id: 'qe-1', name: 'Lint', kind: 'quick_exec', status: 'approval_required',
        required_secrets: [{ name: 'GITHUB_TOKEN', configured: false }],
      })],
    }));
    renderCard(onNavigate);

    // "To handle" is per sub-tab: the automation lives under its own tab.
    fireEvent.click(await screen.findByRole('tab', { name: /tab\.automation/ }));
    const toHandle = within(await screen.findByTestId('repository-attention'));
    fireEvent.click(toHandle.getByRole('button', { name: 'projects.repositoryResources.action.approve' }));
    fireEvent.click(await screen.findByRole('button', { name: 'projects.repositoryResources.approve.addKey' }));
    expect(onNavigate).toHaveBeenCalledWith('mcps');
    fireEvent.keyDown(document.body, { key: 'Escape' });

    fireEvent.click(screen.getByRole('button', { name: 'projects.repositoryResources.banner.openGit' }));
    expect(localStorage.getItem('kronn:projectDetailView')).toBe('git');
    expect(screen.queryByRole('tablist')).not.toBeInTheDocument();
  });
});
