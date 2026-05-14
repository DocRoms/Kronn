// ActiveAuditsPopover — 0.8.3 (#288) regression suite.
//
// Mirrors the workflow ActiveRunsPopover behavior for audits:
//   - empty state when no audit is running
//   - one row per active audit with project name + step + elapsed
//   - Stop button invokes cancelAudit + onAfterCancel
//   - outside click + Escape close the popover
//   - "Voir tous les projets" footer fires onViewAllProjects
//
// Uses the shared apiMock so projects.cancelAudit is a vi.fn we can
// assert on. No real backend, no real claude — pure DOM + behavior.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, screen, act } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// vi.mock is hoisted ABOVE imports — `buildApiMock` cannot be a
// top-level import in this file. Use the async factory form so the
// helper is imported lazily inside the factory's body.
vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import { ActiveAuditsPopover } from '../ActiveAuditsPopover';
import type { AuditProgress, Project } from '../../types/generated';

const proj = (id: string, name: string): Project => ({
  id, name, path: `/r/${id}`,
  repo_url: null, token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false,
  default_skill_ids: [],
  briefing_notes: null,
  linked_repos: [],
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
});

const audit = (project_id: string, step = 2, total = 10): AuditProgress => ({
  project_id, phase: 'auditing',
  step_index: step, total_steps: total,
  current_file: 'docs/AGENTS.md',
  started_at: new Date(Date.now() - 90_000).toISOString(),
  kind: 'full_audit',
});

const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

beforeEach(() => { vi.clearAllMocks(); });
afterEach(() => { vi.useRealTimers(); });

describe('ActiveAuditsPopover (0.8.3 #288)', () => {
  it('renders empty state when no audits are running', () => {
    wrap(<ActiveAuditsPopover
      audits={[]} projects={[]} onClose={() => {}}
      onNavigateToProject={() => {}}
      onViewAllProjects={() => {}}
    />);
    // The FR empty-state copy ships with the default locale.
    expect(screen.getByText(/Aucun audit en cours/)).toBeInTheDocument();
  });

  it('renders one row per active audit with project name + step + elapsed', () => {
    const { container } = wrap(<ActiveAuditsPopover
      audits={[audit('p1', 3, 10), audit('p2', 5, 10)]}
      projects={[proj('p1', 'kronn'), proj('p2', 'front_api')]}
      onClose={() => {}} onNavigateToProject={() => {}}
      onViewAllProjects={() => {}}
    />);
    const items = container.querySelectorAll('.wf-active-runs-item');
    expect(items).toHaveLength(2);
    expect(screen.getByText('kronn')).toBeInTheDocument();
    expect(screen.getByText('front_api')).toBeInTheDocument();
    // Step labels surface step+total via i18n placeholder.
    expect(screen.getByText(/Étape 3\/10/)).toBeInTheDocument();
    expect(screen.getByText(/Étape 5\/10/)).toBeInTheDocument();
  });

  it('clicking a row fires onNavigateToProject with the projectId', () => {
    const onNav = vi.fn();
    wrap(<ActiveAuditsPopover
      audits={[audit('p1')]} projects={[proj('p1', 'kronn')]}
      onClose={() => {}} onNavigateToProject={onNav}
      onViewAllProjects={() => {}}
    />);
    fireEvent.click(screen.getByText('kronn').closest('button')!);
    expect(onNav).toHaveBeenCalledWith('p1');
  });

  it('Stop button calls cancelAudit and fires onAfterCancel', async () => {
    const onAfter = vi.fn();
    vi.mocked(projectsApi.cancelAudit).mockResolvedValue('NoTemplate');
    const { container } = wrap(<ActiveAuditsPopover
      audits={[audit('p1')]} projects={[proj('p1', 'kronn')]}
      onClose={() => {}} onNavigateToProject={() => {}}
      onViewAllProjects={() => {}}
      onAfterCancel={onAfter}
    />);
    const stopBtn = container.querySelector('.wf-active-runs-stop-btn') as HTMLButtonElement;
    expect(stopBtn).not.toBeNull();
    await act(async () => { fireEvent.click(stopBtn); });
    expect(projectsApi.cancelAudit).toHaveBeenCalledWith('p1');
    expect(onAfter).toHaveBeenCalled();
  });

  it('Stop button click does NOT bubble to the row navigate handler', async () => {
    // Critical: the row body has its own onClick that navigates to
    // the project. Without stopPropagation on the Stop button, the
    // click would both cancel the audit AND navigate the user away
    // from where they wanted to stay (Discussions, Workflows, etc.).
    const onNav = vi.fn();
    vi.mocked(projectsApi.cancelAudit).mockResolvedValue('NoTemplate');
    const { container } = wrap(<ActiveAuditsPopover
      audits={[audit('p1')]} projects={[proj('p1', 'kronn')]}
      onClose={() => {}} onNavigateToProject={onNav}
      onViewAllProjects={() => {}}
    />);
    const stopBtn = container.querySelector('.wf-active-runs-stop-btn') as HTMLButtonElement;
    await act(async () => { fireEvent.click(stopBtn); });
    expect(onNav).not.toHaveBeenCalled();
  });

  it('Escape closes the popover', () => {
    const onClose = vi.fn();
    wrap(<ActiveAuditsPopover
      audits={[audit('p1')]} projects={[proj('p1', 'kronn')]}
      onClose={onClose} onNavigateToProject={() => {}}
      onViewAllProjects={() => {}}
    />);
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).toHaveBeenCalled();
  });

  it('footer click fires onViewAllProjects', () => {
    const onView = vi.fn();
    const { container } = wrap(<ActiveAuditsPopover
      audits={[]} projects={[]} onClose={() => {}}
      onNavigateToProject={() => {}}
      onViewAllProjects={onView}
    />);
    const footer = container.querySelector('.wf-active-runs-footer') as HTMLButtonElement;
    fireEvent.click(footer);
    expect(onView).toHaveBeenCalled();
  });

  it('row meta uses Math.max-safe elapsed formatting (no NaN on bad timestamps)', () => {
    // Defensive: a malformed `started_at` (e.g. server clock issue,
    // empty string) shouldn't render NaN. formatElapsed clamps to
    // 0 via Math.max — the chip reads "0s" rather than "NaNs".
    const broken: AuditProgress = { ...audit('p1'), started_at: 'not-a-date' };
    const { container } = wrap(<ActiveAuditsPopover
      audits={[broken]} projects={[proj('p1', 'kronn')]}
      onClose={() => {}} onNavigateToProject={() => {}}
      onViewAllProjects={() => {}}
    />);
    const meta = container.querySelector('.wf-active-runs-item-meta')?.textContent ?? '';
    expect(meta).not.toContain('NaN');
  });

  it('falls back to project_id when the project list is incomplete', () => {
    // Edge: a fresh `auditStatusAll` poll lands a project that was
    // just created and isn't yet in `projects` (lag between the two
    // fetches). The row must still render — using the id as the
    // visible label — instead of crashing or showing "undefined".
    const { container } = wrap(<ActiveAuditsPopover
      audits={[audit('p-orphan')]}
      projects={[]}   // empty — the lookup falls through
      onClose={() => {}} onNavigateToProject={() => {}}
      onViewAllProjects={() => {}}
    />);
    expect(container.textContent).toContain('p-orphan');
  });
});
