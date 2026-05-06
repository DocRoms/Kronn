// ProjectCard — docs migration banner (legacy ai/ → docs/ pivot, 0.7.1).
//
// Locks the operator-facing flow that was missing from the original Sprint 2
// scope: when a project still has `ai/index.md` and no migrated `docs/`,
// expanding the card surfaces a banner with a "Migrer vers docs/" button +
// an opt-out checkbox for the rétro-compat symlink.
//
// API is mocked at the module boundary; we drive the UI and assert that
// `migrateDocs` is called with the right shape and that the banner reacts
// to success / failure.

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

function legacyProject(overrides: Partial<Project> = {}): Project {
  return {
    id: 'p-legacy',
    name: 'LegacyApp',
    path: '/tmp/LegacyApp',
    repo_url: null,
    token_override: null,
    ai_config: { detected: false, configs: [] },
    audit_status: 'NoTemplate',
    ai_todo_count: 0,
    needs_docs_migration: true,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

function renderCard(project: Project, onRefetch = vi.fn()) {
  render(
    <ProjectCard
      project={project}
      isOpen={true}
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

describe('ProjectCard — docs migration banner', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders the banner when needs_docs_migration is true', () => {
    renderCard(legacyProject());
    expect(screen.getByTestId('migration-banner-p-legacy')).toBeInTheDocument();
    expect(screen.getByText('migration.title')).toBeInTheDocument();
  });

  it('does NOT render the banner when needs_docs_migration is false', () => {
    renderCard(legacyProject({ needs_docs_migration: false }));
    expect(screen.queryByTestId('migration-banner-p-legacy')).not.toBeInTheDocument();
  });

  it('calls migrateDocs with create_symlink=true by default and refetches on success', async () => {
    (projectsApi.migrateDocs as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: 'Migrated',
      files_moved: 7,
      refs_rewritten: 3,
      symlink_created: true,
    });
    const { onRefetch } = renderCard(legacyProject());
    fireEvent.click(screen.getByTestId('migrate-docs-btn-p-legacy'));
    await waitFor(() => {
      expect(projectsApi.migrateDocs).toHaveBeenCalledWith('p-legacy', { create_symlink: true });
    });
    // Refetch is delayed by ~1.6 s so the operator sees the success
    // confirmation row before the banner disappears.
    await waitFor(() => expect(onRefetch).toHaveBeenCalled(), { timeout: 3_000 });
  });

  it('passes create_symlink=false when the operator unchecks the opt-out', async () => {
    (projectsApi.migrateDocs as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: 'Migrated',
      files_moved: 4,
    });
    renderCard(legacyProject());
    const checkbox = screen.getByLabelText('migration.symlink') as HTMLInputElement;
    expect(checkbox.checked).toBe(true);
    fireEvent.click(checkbox);
    expect(checkbox.checked).toBe(false);
    fireEvent.click(screen.getByTestId('migrate-docs-btn-p-legacy'));
    await waitFor(
      () => expect(projectsApi.migrateDocs).toHaveBeenCalledWith('p-legacy', { create_symlink: false }),
      { timeout: 3_000 }
    );
  });

  it('renders the inline error when the backend returns Failed', async () => {
    (projectsApi.migrateDocs as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: 'Failed',
      reason: 'docs/ exists with non-empty content — manual merge required',
    });
    renderCard(legacyProject());
    fireEvent.click(screen.getByTestId('migrate-docs-btn-p-legacy'));
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent(/manual merge required/);
    });
  });

  it('renders the inline error when the request itself throws', async () => {
    (projectsApi.migrateDocs as unknown as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('500 internal'));
    renderCard(legacyProject());
    fireEvent.click(screen.getByTestId('migrate-docs-btn-p-legacy'));
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent(/500 internal/);
    });
  });

  it('refetches on AlreadyMigrated outcome (banner stale)', async () => {
    (projectsApi.migrateDocs as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: 'AlreadyMigrated',
    });
    const { onRefetch } = renderCard(legacyProject());
    fireEvent.click(screen.getByTestId('migrate-docs-btn-p-legacy'));
    await waitFor(() => {
      expect(onRefetch).toHaveBeenCalled();
    });
  });

  it('shows the success row inline before refetch fires', async () => {
    // Hold the API resolution behind a manual promise so we can assert
    // the in-flight UI before letting the call complete.
    let resolveMigrate!: (v: unknown) => void;
    (projectsApi.migrateDocs as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      () => new Promise(r => { resolveMigrate = r; })
    );
    vi.useFakeTimers();
    try {
      const { onRefetch } = renderCard(legacyProject());
      fireEvent.click(screen.getByTestId('migrate-docs-btn-p-legacy'));

      // While the API call is pending, the inline progress row shows.
      expect(screen.getByText('migration.inProgress')).toBeInTheDocument();

      // Resolve the API → success row should appear, refetch is delayed.
      await vi.waitFor(() => expect(resolveMigrate).toBeDefined());
      resolveMigrate({ status: 'Migrated', files_moved: 12 });
      await vi.waitFor(() =>
        expect(screen.getByText(/migration.successInline 12/)).toBeInTheDocument()
      );
      expect(onRefetch).not.toHaveBeenCalled();

      // After the dwell timeout, the refetch fires.
      vi.advanceTimersByTime(2000);
      expect(onRefetch).toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('disables the button while migrating and again after success dwell', async () => {
    let resolveMigrate!: (v: unknown) => void;
    (projectsApi.migrateDocs as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      () => new Promise(r => { resolveMigrate = r; })
    );
    renderCard(legacyProject());
    const btn = screen.getByTestId('migrate-docs-btn-p-legacy');
    fireEvent.click(btn);
    await waitFor(() => expect(btn).toBeDisabled());
    expect(screen.getByText('migration.ctaPending')).toBeInTheDocument();

    resolveMigrate({ status: 'Migrated', files_moved: 1 });
    await waitFor(() => expect(screen.getByText('migration.ctaDone')).toBeInTheDocument());
    expect(btn).toBeDisabled();
  });
});
