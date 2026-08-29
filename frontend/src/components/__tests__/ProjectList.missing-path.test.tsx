// ProjectList — missing-path banner + filter (0.8.9).
//
// After a cross-OS import, projects whose directory doesn't resolve are flagged
// `path_exists === false`. The list surfaces a persistent banner (count, sing.
// vs plural) and a one-click toggle to filter down to just those projects.
// ProjectCard is stubbed — this exercises the list's own logic only.

import { beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import type { Project } from '../../types/generated';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));
vi.mock('../ProjectCard', () => ({
  ProjectCard: ({ project }: { project: Project }) => (
    <div data-testid={`card-${project.id}`}>{project.name}</div>
  ),
}));
vi.mock('../MatrixText', () => ({ MatrixText: ({ text }: { text: string }) => <span>{text}</span> }));
const deleteProject = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock('../../lib/api', () => ({ projects: { delete: deleteProject } }));

import { ProjectList } from '../ProjectList';

const noop = () => {};

function proj(id: string, name: string, path: string, path_exists?: boolean, updatedAt = '2026-01-01T00:00:00Z'): Project {
  return {
    id, name, path,
    repo_url: null, token_override: null,
    ai_config: { detected: false, configs: [] },
    audit_status: 'NoTemplate', ai_todo_count: 0, tech_debt_count: 0,
    needs_docs_migration: false, path_exists,
    created_at: '2026-01-01T00:00:00Z', updated_at: updatedAt,
  } as Project;
}

function renderList(projects: Project[]) {
  return render(
    <ProjectList
      projects={projects}
      discussions={[]}
      discussionsByProject={{}}
      driftByProject={{}}
      agents={[]}
      allSkills={[]}
      mcpConfigs={[]}
      workflows={[]}
      configLanguage="fr"
      toast={noop}
      onNavigate={noop}
      onSetDiscPrefill={noop}
      onAutoRunDiscussion={noop}
      onOpenDiscussion={noop}
      onRefetch={noop}
      onRefetchDiscussions={noop}
      onRefetchSkills={noop}
      onRefetchDrift={noop}
      expandedId={null}
      onSetExpandedId={noop}
    />
  );
}

describe('ProjectList — missing-path banner', () => {
  beforeEach(() => {
    localStorage.clear();
    deleteProject.mockClear();
    vi.stubGlobal('confirm', () => true);
  });
  it('mounts one detail card while keeping every project in the compact list', () => {
    renderList([
      proj('p1', 'Alpha', '/repos/alpha', true),
      proj('p2', 'Beta', '/repos/beta', true),
      proj('p3', 'Gamma', '/repos/gamma', true),
    ]);

    expect(within(screen.getByTestId('project-section-all')).getAllByTestId(/^project-list-item-/)).toHaveLength(3);
    expect(screen.getAllByTestId(/^card-/)).toHaveLength(1);
    expect(screen.getByRole('complementary', { name: 'projects.title' })).toHaveClass('collection-shell-sidebar');
  });

  it('selects a project through the shared flat-list row contract', () => {
    const onSetExpandedId = vi.fn();
    render(
      <ProjectList
        projects={[proj('p1', 'Alpha', '/repos/alpha', true), proj('p2', 'Beta', '/repos/beta', true)]}
        discussions={[]}
        discussionsByProject={{}}
        driftByProject={{}}
        agents={[]}
        allSkills={[]}
        mcpConfigs={[]}
        workflows={[]}
        configLanguage="fr"
        toast={noop}
        onNavigate={noop}
        onSetDiscPrefill={noop}
        onAutoRunDiscussion={noop}
        onOpenDiscussion={noop}
        onRefetch={noop}
        onRefetchDiscussions={noop}
        onRefetchSkills={noop}
        onRefetchDrift={noop}
        expandedId={null}
        onSetExpandedId={onSetExpandedId}
      />,
    );

    const alphaRow = screen.getByTestId('project-list-item-p1').querySelector('.disc-item-open');
    expect(alphaRow).toHaveAttribute('aria-current', 'true');
    fireEvent.click(screen.getByTestId('project-list-item-p2').querySelector('.disc-item-open')!);
    expect(onSetExpandedId).toHaveBeenCalledWith('p2');
  });

  it('hides the banner when every path resolves', () => {
    renderList([
      proj('p1', 'Alpha', '/repos/alpha', true),
      proj('p2', 'Beta', '/repos/beta'), // undefined = treated as present
    ]);
    expect(screen.queryByTestId('missing-path-banner')).not.toBeInTheDocument();
  });

  it('shows the singular banner for exactly one missing project', () => {
    renderList([
      proj('p1', 'Alpha', '/repos/alpha', true),
      proj('p2', 'Beta', '/repos/beta', false),
    ]);
    expect(screen.getByTestId('missing-path-banner')).toBeInTheDocument();
    expect(screen.getByText('projects.missingBanner.one')).toBeInTheDocument();
  });

  it('shows the plural banner with the count for several missing projects', () => {
    renderList([
      proj('p1', 'Alpha', '/repos/alpha', false),
      proj('p2', 'Beta', '/repos/beta', false),
      proj('p3', 'Gamma', '/repos/gamma', true),
    ]);
    expect(screen.getByText('projects.missingBanner.plural 2')).toBeInTheDocument();
  });

  it('filters down to only the missing projects when the toggle is clicked', () => {
    renderList([
      proj('p1', 'Alpha', '/repos/alpha', true),
      proj('p2', 'Beta', '/repos/beta', false),
    ]);
    // Both compact list entries are visible initially. Only one heavy detail
    // card is mounted at a time by the master/detail layout.
    expect(screen.getByTestId('project-list-item-p1')).toBeInTheDocument();
    expect(screen.getByTestId('project-list-item-p2')).toBeInTheDocument();

    fireEvent.click(screen.getByText('projects.missingBanner.showOnly'));

    // Now only the missing one remains; the toggle flips to "show all".
    expect(screen.queryByTestId('project-list-item-p1')).not.toBeInTheDocument();
    expect(screen.getByTestId('project-list-item-p2')).toBeInTheDocument();
    expect(screen.getByText('projects.missingBanner.showAll')).toBeInTheDocument();
  });

  it('uses the shared title, favorite, collapse, and bulk-delete interactions', async () => {
    const onRefetch = vi.fn();
    render(
      <ProjectList
        projects={[proj('p1', 'Alpha', '/repos/alpha', true), proj('p2', 'Beta', '/repos/beta', true)]}
        discussions={[]} discussionsByProject={{}} driftByProject={{}} agents={[]} allSkills={[]} mcpConfigs={[]} workflows={[]}
        configLanguage="fr" toast={vi.fn()} onNavigate={noop} onSetDiscPrefill={noop} onAutoRunDiscussion={noop}
        onOpenDiscussion={noop} onRefetch={onRefetch} onRefetchDiscussions={noop} onRefetchSkills={noop} onRefetchDrift={noop}
        expandedId={null} onSetExpandedId={noop}
      />,
    );
    expect(screen.getByText('projects.title').closest('.collection-shell-title')).toHaveTextContent('projects.title · 2');
    fireEvent.click(within(screen.getByTestId('project-section-all')).getByRole('button', { name: /wf\.pin · Alpha/ }));
    expect(localStorage.getItem('kronn:collection-favorites:projects')).toContain('p1');
    fireEvent.click(screen.getByRole('button', { name: 'projects.master.filter' }));
    fireEvent.click(screen.getByRole('button', { name: 'collection.favorites' }));
    expect(screen.getByTestId('project-list-item-p1')).toBeInTheDocument();
    expect(screen.queryByTestId('project-list-item-p2')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'collection.favorites' }));
    fireEvent.click(screen.getByRole('button', { name: 'collection.moreActions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'collection.selectMultiple' }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Alpha.*collection\.selectItem/ }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Beta.*collection\.selectItem/ }));
    fireEvent.click(screen.getByRole('button', { name: 'collection.deleteSelected' }));
    await waitFor(() => expect(deleteProject).toHaveBeenCalledTimes(2));
    expect(onRefetch).toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'collection.closeCollection' }));
    expect(screen.queryByRole('complementary', { name: 'projects.title' })).toBeNull();
    expect(screen.getByRole('button', { name: 'collection.openCollection' })).toHaveClass('collection-shell-sidebar-rail');
  });

  it('uses the shared row menu and keyboard footer', async () => {
    const onRefetch = vi.fn();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    const { container } = render(
      <ProjectList
        projects={[proj('p1', 'Alpha', '/repos/alpha', true)]}
        discussions={[]} discussionsByProject={{}} driftByProject={{}} agents={[]} allSkills={[]} mcpConfigs={[]} workflows={[]}
        configLanguage="fr" toast={vi.fn()} onNavigate={noop} onSetDiscPrefill={noop} onAutoRunDiscussion={noop}
        onOpenDiscussion={noop} onRefetch={onRefetch} onRefetchDiscussions={noop} onRefetchSkills={noop} onRefetchDrift={noop}
        expandedId={null} onSetExpandedId={noop}
      />,
    );

    const row = within(screen.getByTestId('project-section-all')).getByTestId('project-list-item-p1');
    fireEvent.click(within(row).getByRole('button', { name: 'collection.moreActions · Alpha' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'disc.copyId' }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('p1'));
    fireEvent.click(screen.getByRole('menuitem', { name: 'disc.delete' }));
    await waitFor(() => expect(deleteProject).toHaveBeenCalledWith('p1'));
    expect(onRefetch).toHaveBeenCalled();

    const footer = container.querySelector('.disc-sidebar-footer') as HTMLElement;
    expect(footer).toHaveTextContent('projects.sidebar.hint');
    expect(within(footer).getByText('↑↓')).toBeInTheDocument();
    expect(within(footer).getByText('/')).toBeInTheDocument();
  });

  it('renders persistent Favorites, Recent, and canonical All sections', async () => {
    localStorage.setItem('kronn:collection-favorites:projects', JSON.stringify(['p12']));
    const projects = Array.from({ length: 12 }, (_, index) => {
      const number = index + 1;
      const suffix = String(number).padStart(2, '0');
      return proj(
        `p${number}`,
        `Project ${suffix}`,
        `/repos/project-${suffix}`,
        true,
        `2026-01-${suffix}T00:00:00Z`,
      );
    });
    const view = renderList(projects);

    const favorites = screen.getByTestId('project-section-favorites');
    expect(within(favorites).getByRole('button', { name: 'disc.favorites 1' })).toHaveClass('collection-favorites-header');
    const recent = screen.getByTestId('project-section-recent');
    const all = screen.getByTestId('project-section-all');
    expect([...all.parentElement!.children].map(section => section.getAttribute('data-section')))
      .toEqual(['favorites', 'recent', 'all']);
    expect(within(favorites).getByText('Project 12')).toBeInTheDocument();
    expect(within(recent).getAllByText(/^Project /).map(node => node.textContent)).toEqual([
      'Project 11', 'Project 10', 'Project 09', 'Project 08', 'Project 07',
      'Project 06', 'Project 05', 'Project 04', 'Project 03', 'Project 02',
    ]);
    expect(within(recent).queryByText('Project 12')).toBeNull();
    expect(within(all).getAllByTestId(/^project-list-item-/)).toHaveLength(12);

    const favoriteCanonicalRow = within(all).getByTestId('project-list-item-p12');
    expect(favoriteCanonicalRow.querySelector('.disc-item')).toBeInTheDocument();
    expect(favoriteCanonicalRow.querySelector('.disc-item-meta-summary')).toHaveTextContent('/repos/project-12');
    expect(within(favoriteCanonicalRow).getByRole('button', { name: 'wf.unpin · Project 12' }))
      .toHaveClass('kr-favorite-toggle');
    expect(within(all).getByTestId('project-list-item-p1').querySelector('.disc-item'))
      .toHaveAttribute('data-active', 'true');

    const recentToggle = within(recent).getByRole('button', { name: /disc\.recent/ });
    fireEvent.click(recentToggle);
    expect(recentToggle).toHaveAttribute('aria-expanded', 'false');
    expect(within(recent).queryByText('Project 11')).toBeNull();
    expect(localStorage.getItem('kronn:project-sidebar-collapsed-sections')).toContain('recent');

    view.unmount();
    renderList(projects);
    const persistedRecent = screen.getByTestId('project-section-recent');
    expect(within(persistedRecent).getByRole('button', { name: /disc\.recent/ })).toHaveAttribute('aria-expanded', 'false');
    expect(within(persistedRecent).queryByText('Project 11')).toBeNull();

    fireEvent.change(screen.getByRole('textbox', { name: 'projects.search' }), { target: { value: 'Project 11' } });
    await waitFor(() => expect(within(persistedRecent).getByText('Project 11')).toBeInTheDocument());
    expect(within(persistedRecent).getByRole('button', { name: /disc\.recent/ })).toHaveAttribute('aria-expanded', 'true');
  });

  it('opens the existing add-project flow from the primary header action', () => {
    const onAddProject = vi.fn();
    render(
      <ProjectList
        projects={[proj('p1', 'Alpha', '/repos/alpha', true)]}
        discussions={[]} discussionsByProject={{}} driftByProject={{}} agents={[]} allSkills={[]} mcpConfigs={[]} workflows={[]}
        configLanguage="fr" toast={noop} onNavigate={noop} onSetDiscPrefill={noop} onAutoRunDiscussion={noop}
        onOpenDiscussion={noop} onRefetch={noop} onRefetchDiscussions={noop} onRefetchSkills={noop} onRefetchDrift={noop}
        onAddProject={onAddProject} expandedId={null} onSetExpandedId={noop}
      />,
    );

    const addButton = screen.getByRole('button', { name: 'projects.bootstrap' });
    expect(addButton).toHaveClass('collection-shell-icon', 'collection-shell-primary-action');
    expect(addButton).toHaveAttribute('data-tour-id', 'new-project-btn');
    fireEvent.click(addButton);
    expect(onAddProject).toHaveBeenCalledOnce();
  });

  it('separates icon-only filtering and sorting beside the shared search', () => {
    renderList([
      proj('p1', 'Alpha', '/repos/alpha', true),
      proj('p2', 'Beta', '/repos/beta', true),
    ]);

    const search = screen.getByRole('textbox', { name: 'projects.search' });
    fireEvent.keyDown(window, { key: '/' });
    expect(search).toHaveFocus();

    fireEvent.change(search, { target: { value: 'Alpha' } });
    const searchHeader = search.closest<HTMLElement>('.collection-shell-header')!;
    fireEvent.click(within(searchHeader).getByRole('button', { name: 'collection.clearFilters' }));
    expect(search).toHaveValue('');

    const filterButton = within(searchHeader).getByRole('button', { name: 'projects.master.filter' });
    const sortButton = within(searchHeader).getByRole('button', { name: 'projects.master.sort' });
    expect(filterButton).toHaveClass('collection-shell-search-action', 'collection-shell-search-action-icon');
    expect(sortButton).toHaveClass('collection-shell-search-action', 'collection-shell-search-action-icon');
    expect(filterButton.querySelector('span')).toBeNull();
    expect(sortButton.querySelector('span')).toBeNull();
    expect(filterButton).toHaveAttribute('aria-expanded', 'false');
    expect(sortButton).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('button', { name: 'projects.master.filter.attention' })).toBeNull();
    expect(screen.queryByRole('combobox', { name: 'projects.master.sort' })).toBeNull();

    fireEvent.click(filterButton);
    expect(filterButton).toHaveAttribute('aria-expanded', 'true');
    expect(sortButton).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(screen.getByRole('button', { name: 'projects.master.filter.attention' }));
    expect(filterButton).toHaveAttribute('data-active', 'true');

    fireEvent.click(sortButton);
    expect(filterButton).toHaveAttribute('aria-expanded', 'false');
    expect(sortButton).toHaveAttribute('aria-expanded', 'true');
    const sortSelect = screen.getByRole('combobox', { name: 'projects.master.sort' });
    const directionButton = screen.getByRole('button', { name: 'projects.master.sort.direction' });
    expect(sortSelect).toHaveValue('name');
    expect(directionButton).toHaveAttribute('aria-pressed', 'false');
    fireEvent.change(sortSelect, { target: { value: 'status' } });
    fireEvent.click(directionButton);
    expect(directionButton).toHaveAttribute('aria-pressed', 'true');

    fireEvent.click(sortButton);
    expect(screen.queryByRole('combobox', { name: 'projects.master.sort' })).toBeNull();
    fireEvent.click(sortButton);
    expect(screen.getByRole('combobox', { name: 'projects.master.sort' })).toHaveValue('status');
    expect(screen.getByRole('button', { name: 'projects.master.sort.direction' })).toHaveAttribute('aria-pressed', 'true');
  });
});
