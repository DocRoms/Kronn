// ProjectCard — remap banner (0.8.9).
//
// When a project's directory no longer resolves on disk (`path_exists === false`,
// typically after a cross-OS DB import where absolute paths don't translate),
// the card surfaces an always-visible banner with an inline path input + a
// "Remap" button wired to `projectsApi.remapPath`. Locks: the banner only shows
// on a genuinely-missing path, it's visible even on a collapsed card, the call
// shape is right, success refetches, and a backend rejection shows inline.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));
vi.mock('../../hooks/useMediaQuery', () => ({ useIsMobile: () => false }));

import { ProjectCard } from '../ProjectCard';
import { projects as projectsApi } from '../../lib/api';
import type { Project } from '../../types/generated';

const noop = () => {};
const toast = vi.fn();

function project(overrides: Partial<Project> = {}): Project {
  return {
    id: 'p-ghost',
    name: 'GhostApp',
    path: '/home/priol/Repos/GhostApp',
    repo_url: null,
    token_override: null,
    ai_config: { detected: false, configs: [] },
    audit_status: 'NoTemplate',
    ai_todo_count: 0, tech_debt_count: 0,
    needs_docs_migration: false,
    path_exists: false,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

function renderCard(p: Project, { isOpen = false, onRefetch = vi.fn() } = {}) {
  render(
    <ProjectCard
      project={p}
      isOpen={isOpen}
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
    />
  );
  return { onRefetch };
}

describe('ProjectCard — remap banner', () => {
  beforeEach(() => vi.clearAllMocks());

  it('renders the banner when path_exists === false, even on a collapsed card', () => {
    renderCard(project(), { isOpen: false });
    expect(screen.getByTestId('remap-banner-p-ghost')).toBeInTheDocument();
  });

  it('does NOT render when path_exists is true or absent', () => {
    const { rerender } = render(<div />);
    void rerender;
    renderCard(project({ path_exists: true }));
    expect(screen.queryByTestId('remap-banner-p-ghost')).not.toBeInTheDocument();
  });

  it('does NOT render when path_exists is undefined (legacy payload)', () => {
    const p = project();
    delete (p as { path_exists?: boolean }).path_exists;
    renderCard(p);
    expect(screen.queryByTestId('remap-banner-p-ghost')).not.toBeInTheDocument();
  });

  it('remaps to the typed path, toasts, and refetches on success', async () => {
    const { onRefetch } = renderCard(project());
    const input = screen.getByLabelText('projects.remap.title') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '/Users/me/Repos/GhostApp' } });
    fireEvent.click(screen.getByText('projects.remap.cta'));
    await waitFor(() =>
      expect(projectsApi.remapPath).toHaveBeenCalledWith('p-ghost', '/Users/me/Repos/GhostApp')
    );
    await waitFor(() => expect(onRefetch).toHaveBeenCalled());
    expect(toast).toHaveBeenCalledWith(expect.stringContaining('projects.remap.successToast'), 'success');
  });

  it('trims the path before sending', async () => {
    renderCard(project());
    const input = screen.getByLabelText('projects.remap.title') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '  /Users/me/Repos/GhostApp  ' } });
    fireEvent.click(screen.getByText('projects.remap.cta'));
    await waitFor(() =>
      expect(projectsApi.remapPath).toHaveBeenCalledWith('p-ghost', '/Users/me/Repos/GhostApp')
    );
  });

  it('disables the button while the input is empty', () => {
    renderCard(project());
    expect(screen.getByText('projects.remap.cta').closest('button')).toBeDisabled();
  });

  it('shows the backend error inline and does NOT refetch', async () => {
    (projectsApi.remapPath as unknown as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error('Path does not exist')
    );
    const { onRefetch } = renderCard(project());
    fireEvent.change(screen.getByLabelText('projects.remap.title'), {
      target: { value: '/wrong/path' },
    });
    fireEvent.click(screen.getByText('projects.remap.cta'));
    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent(/Path does not exist/)
    );
    expect(onRefetch).not.toHaveBeenCalled();
  });

  // ── Clone-and-remap (0.8.9) ──────────────────────────────────────────────
  // When the project has a repo_url, the banner offers a one-click "Clone
  // repository" action that re-clones via the linked Git credentials and
  // re-points the project — an alternative to typing a local path.

  it('does NOT show the clone button when the project has no repo_url', () => {
    renderCard(project({ repo_url: null }));
    expect(screen.queryByText('projects.remap.cloneCta')).not.toBeInTheDocument();
  });

  it('shows the clone button when the project has a repo_url', () => {
    renderCard(project({ repo_url: 'https://github.com/me/GhostApp.git' }));
    expect(screen.getByText('projects.remap.cloneCta')).toBeInTheDocument();
  });

  it('clones with no parent_dir when the input is empty, toasts, and refetches', async () => {
    const { onRefetch } = renderCard(project({ repo_url: 'https://github.com/me/GhostApp.git' }));
    fireEvent.click(screen.getByText('projects.remap.cloneCta'));
    await waitFor(() =>
      expect(projectsApi.cloneAndRemap).toHaveBeenCalledWith('p-ghost', { parent_dir: null })
    );
    await waitFor(() => expect(onRefetch).toHaveBeenCalled());
    expect(toast).toHaveBeenCalledWith(
      expect.stringContaining('projects.remap.cloneSuccessToast'),
      'success',
    );
  });

  it('passes the typed path as the target parent directory when cloning', async () => {
    renderCard(project({ repo_url: 'https://github.com/me/GhostApp.git' }));
    fireEvent.change(screen.getByLabelText('projects.remap.title'), {
      target: { value: '  /Users/me/Repos  ' },
    });
    fireEvent.click(screen.getByText('projects.remap.cloneCta'));
    await waitFor(() =>
      expect(projectsApi.cloneAndRemap).toHaveBeenCalledWith('p-ghost', { parent_dir: '/Users/me/Repos' })
    );
  });

  it('shows a clone failure inline and does NOT refetch', async () => {
    (projectsApi.cloneAndRemap as unknown as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error('git clone failed: auth')
    );
    const { onRefetch } = renderCard(project({ repo_url: 'https://github.com/me/GhostApp.git' }));
    fireEvent.click(screen.getByText('projects.remap.cloneCta'));
    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent(/git clone failed: auth/)
    );
    expect(onRefetch).not.toHaveBeenCalled();
  });
});
