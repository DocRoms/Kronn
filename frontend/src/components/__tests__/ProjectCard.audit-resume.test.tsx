// ProjectCard — audit resume regression (0.8.3 #280 follow-up).
//
// Bug observed by user: clicking "Start audit" launched the run server-
// side, but refreshing the browser dropped the audit progress bar — the
// user saw "Start audit" again even though the backend was still
// auditing. Root cause: the mount-time resume effect SKIPPED the
// backend poll when localStorage had no checkpoint. Anything that
// wiped the checkpoint between the click and the refresh (dev-mode
// HMR, cross-tab navigation, a browser that cleared storage on the
// way out) would leave the audit running invisibly.
//
// Fix: always poll the backend at mount. The localStorage checkpoint
// becomes a UX optimization (seeds the bar before the network round-
// trip) but is no longer a precondition.
//
// These tests pin the new contract:
//   1. Backend says "audit running" → bar appears even when no
//      localStorage checkpoint exists.
//   2. Backend says null → no bar shown (idle card).
//   3. With checkpoint AND backend confirms → bar seeded from
//      checkpoint, then refreshed from backend on first poll.
//   4. No spurious `onRefetch` calls when the card is idle (an
//      always-on poll musn't spam the projects list endpoint).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, screen } from '@testing-library/react';
import { readFileSync } from 'node:fs';
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
import type { Project, AgentDetection } from '../../types/generated';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-resume',
  name: 'resume-target',
  path: '/repos/resume-target',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
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
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

function renderCard(onRefetch = noop, detailMode = false) {
  return render(
    <ProjectCard
      project={PROJECT}
      detailMode={detailMode}
      isOpen={true}
      onToggleOpen={noop}
      discussions={[]}
      driftStatus={undefined}
      agents={[AGENT]}
      allSkills={[]}
      mcpConfigs={[]}
      workflows={[]}
      configLanguage="fr"
      toast={vi.fn()}
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
}

beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('ProjectCard — audit resume after refresh (0.8.3 #280-fix)', () => {
  // The "audit running" panel is identified by the `dash-audit-step`
  // text + the Cancel button it carries — NOT just `dash-audit-pad`
  // (that class is reused by the "no audit yet → Start" call-to-
  // action above the bar). We look for the i18n key `audit.step` which
  // only renders inside the live progress bar.
  const isAuditBarMounted = (container: HTMLElement) =>
    container.querySelector('.dash-audit-step') !== null;

  it('surfaces the audit bar when backend reports running, even WITHOUT a local checkpoint', async () => {
    // The killer regression. Pre-fix: no checkpoint → no poll → no
    // bar even though the audit is alive server-side.
    vi.mocked(projectsApi.auditStatus).mockResolvedValue({
      project_id: 'p-resume',
      phase: 'auditing',
      step_index: 3,
      total_steps: 16,
      current_file: 'docs/AGENTS.md',
      started_at: '2026-05-14T17:44:14Z',
      kind: 'full_audit',
    });

    const { container } = renderCard();
    // Let the immediate poll fire and resolve.
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });

    expect(projectsApi.auditStatus).toHaveBeenCalledWith('p-resume');
    expect(isAuditBarMounted(container)).toBe(true);
  });

  it('does NOT surface the audit bar when backend reports null (no audit)', async () => {
    vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
    const onRefetch = vi.fn();
    const { container } = renderCard(onRefetch);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });

    expect(isAuditBarMounted(container)).toBe(false);
    // CRITICAL: an idle ProjectCard MUST NOT call onRefetch on every
    // poll. Pre-fix the `else` branch fired onRefetch unconditionally
    // → every idle card spammed the projects list endpoint at
    // mount + every 2 s. The fix gates it on "was the bar active?".
    expect(onRefetch).not.toHaveBeenCalled();
  });

  it('seeds the bar from localStorage checkpoint when present (UX optim)', async () => {
    // When the checkpoint IS there, we don't have to wait for the
    // first network round-trip — the bar appears immediately. The
    // first poll then reconciles with whatever the backend reports.
    const cp = {
      v: 1,
      projectId: 'p-resume',
      kind: 'full_audit',
      startedAt: new Date().toISOString(),
      stepIndex: 2,
      totalSteps: 16,
      currentFile: 'docs/glossary.md',
    };
    localStorage.setItem('kronn:audit-checkpoint:p-resume', JSON.stringify(cp));
    vi.mocked(projectsApi.auditStatus).mockResolvedValue({
      project_id: 'p-resume',
      phase: 'auditing',
      step_index: 2,
      total_steps: 16,
      current_file: 'docs/glossary.md',
      started_at: cp.startedAt,
      kind: 'full_audit',
    });

    const { container } = renderCard();
    // React 19 schedules state-from-effect through microtasks; one
    // flush is enough to commit the seed setState.
    await act(async () => { await Promise.resolve(); });
    expect(isAuditBarMounted(container)).toBe(true);
  });

  it('clears bar + refetches when backend reports null AFTER having been active', async () => {
    // Steady-state polling: bar was up because backend said "running",
    // then the audit wraps. Next poll returns null → bar comes down
    // AND onRefetch is called (so the project_card's audit_status
    // catches up to "Audited").
    let firstCall = true;
    vi.mocked(projectsApi.auditStatus).mockImplementation(async () => {
      if (firstCall) {
        firstCall = false;
        return {
          project_id: 'p-resume', phase: 'auditing', step_index: 15,
          total_steps: 16, current_file: 'docs/x.md',
          started_at: '2026-05-14T17:44:14Z', kind: 'full_audit',
        };
      }
      return null;
    });
    const onRefetch = vi.fn();
    const { container } = renderCard(onRefetch);
    // First poll → running → bar mounts.
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    expect(isAuditBarMounted(container)).toBe(true);

    // Advance the 2 s polling interval → next poll → null → bar
    // un-mounts AND onRefetch fires.
    await act(async () => {
      vi.advanceTimersByTime(2000);
      await Promise.resolve(); await Promise.resolve();
    });
    expect(isAuditBarMounted(container)).toBe(false);
    expect(onRefetch).toHaveBeenCalled();
  });

  it.each([
    ['Failed', 'projects.docAi.auditRecap.status.failed'],
    ['Completed', 'projects.docAi.auditRecap.status.completed'],
  ])('shows a %s terminal result on the project card and opens its exact Audit run', async (status, label) => {
    const terminalRunId = `run-${status.toLowerCase()}`;
    vi.mocked(projectsApi.auditHistory).mockResolvedValue([{
      id: 'run-still-running', project_id: PROJECT.id, kind: 'Full', agent_type: 'ClaudeCode',
      started_at: '2026-05-14T17:46:14Z', ended_at: null,
      duration_ms: null, status: 'Running', td_total: 0,
    }, {
      id: terminalRunId, project_id: PROJECT.id, kind: 'Full', agent_type: 'ClaudeCode',
      started_at: '2026-05-14T17:44:14Z', ended_at: '2026-05-14T17:45:14Z',
      duration_ms: 60_000, status, td_total: 0,
    }]);

    const { container } = renderCard(noop, true);
    // The suite uses fake timers for the audit poll. Flush the effect's
    // resolved promise directly instead of using findBy*, which polls using
    // those frozen timers and can therefore time out.
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    const outcome = screen.getByTestId('project-audit-outcome');
    expect(outcome.textContent).toContain(label);

    await act(async () => { fireEvent.click(outcome); await Promise.resolve(); });
    expect(container.querySelector('.dash-card-body')).toHaveAttribute('data-detail-view', 'audit');
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    expect(projectsApi.auditRunSteps).toHaveBeenCalledWith(terminalRunId);
  });

  it('refreshes the terminal outcome after cancelling an active audit', async () => {
    vi.mocked(projectsApi.auditStatus).mockResolvedValue({
      project_id: PROJECT.id, phase: 'auditing', step_index: 3, total_steps: 16,
      current_file: 'docs/AGENTS.md', started_at: '2026-05-14T17:44:14Z', kind: 'full_audit',
    });
    vi.mocked(projectsApi.auditHistory).mockResolvedValue([{
      id: 'run-cancelled', project_id: PROJECT.id, kind: 'Full', agent_type: 'ClaudeCode',
      started_at: '2026-05-14T17:44:14Z', ended_at: '2026-05-14T17:45:14Z',
      duration_ms: 60_000, status: 'Cancelled', td_total: 0,
    }]);

    renderCard();
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    const historyCallsBeforeCancel = vi.mocked(projectsApi.auditHistory).mock.calls.length;
    await act(async () => { fireEvent.click(screen.getByText('audit.cancelAudit')); await Promise.resolve(); });

    expect(projectsApi.cancelAudit).toHaveBeenCalledWith(PROJECT.id);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    expect(vi.mocked(projectsApi.auditHistory).mock.calls.length).toBeGreaterThan(historyCallsBeforeCancel);
    expect(screen.getByTestId('project-audit-outcome')).toHaveTextContent('projects.docAi.auditRecap.status.cancelled');
  });

  it('defines the green success treatment for terminal audit outcome chips', () => {
    const css = readFileSync('src/pages/Dashboard.css', 'utf8');
    expect(css).toMatch(/\.project-alert-chip\[data-tone='success'\]\s*\{\s*color:\s*var\(--kr-success\);/);
  });
});
