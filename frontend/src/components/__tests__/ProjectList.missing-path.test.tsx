// ProjectList — missing-path banner + filter (0.8.9).
//
// After a cross-OS import, projects whose directory doesn't resolve are flagged
// `path_exists === false`. The list surfaces a persistent banner (count, sing.
// vs plural) and a one-click toggle to filter down to just those projects.
// ProjectCard is stubbed — this exercises the list's own logic only.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
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

import { ProjectList } from '../ProjectList';

const noop = () => {};

function proj(id: string, name: string, path: string, path_exists?: boolean): Project {
  return {
    id, name, path,
    repo_url: null, token_override: null,
    ai_config: { detected: false, configs: [] },
    audit_status: 'NoTemplate', ai_todo_count: 0, tech_debt_count: 0,
    needs_docs_migration: false, path_exists,
    created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
  } as Project;
}

function renderList(projects: Project[]) {
  render(
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
    // Both cards visible initially.
    expect(screen.getByTestId('card-p1')).toBeInTheDocument();
    expect(screen.getByTestId('card-p2')).toBeInTheDocument();

    fireEvent.click(screen.getByText('projects.missingBanner.showOnly'));

    // Now only the missing one remains; the toggle flips to "show all".
    expect(screen.queryByTestId('card-p1')).not.toBeInTheDocument();
    expect(screen.getByTestId('card-p2')).toBeInTheDocument();
    expect(screen.getByText('projects.missingBanner.showAll')).toBeInTheDocument();
  });
});
