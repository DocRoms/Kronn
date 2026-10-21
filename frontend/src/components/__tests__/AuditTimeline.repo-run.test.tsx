// KT-1199 — an audit run another machine committed: the timeline shows the
// repository's state (steps done / to resume / to re-verify, TD settled) and
// offers to validate only when the server says the repository state can.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { ReactElement } from 'react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi, externalApi } from '../../lib/api';
import { AuditTimeline, type AuditTimelineProps } from '../AuditTimeline';
import type { AgentDetection } from '../../types/generated';
import { stepRanges } from '../../lib/stepRanges';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true, path: '/usr/bin/claude',
  version: '1.0.0', latest_version: null, origin: 'host', install_command: null, host_managed: false,
  host_label: null, runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

function props(over: Partial<AuditTimelineProps> = {}): AuditTimelineProps {
  return {
    projectId: 'p1', auditStatus: 'Audited', techDebtCount: 0, agents: [AGENT],
    selectedAgent: 'ClaudeCode', selectedTier: 'reasoning', modelTiers: null, selectedConnectionId: null, onSelect: vi.fn(),
    briefingDone: true, onBriefingSaved: vi.fn(), auditActive: false, liveStep: 0, liveTotal: 0,
    liveFile: '', liveElapsed: null, liveTool: null, liveStartedAt: null, liveToolCalls: null, liveStepTokens: null,
    liveTotalTokens: null, onResumeBriefingDiscussion: null, onCancel: vi.fn(), resumable: null,
    onLaunch: vi.fn(), validationInProgress: false,
    onMarkValid: vi.fn().mockResolvedValue(undefined), onOpenValidation: vi.fn(), onViewTechDebts: vi.fn(),
    onRetryStep: vi.fn(), onAcceptStep: vi.fn().mockResolvedValue(undefined), onRelaunchFull: vi.fn(),
    onCleanupLease: vi.fn().mockResolvedValue(undefined),
    refreshTrigger: 0, toast: vi.fn(), ...over,
  };
}

const repoRun = (over: Record<string, unknown> = {}) => ({
  run_id: 'run-elsewhere', kind: 'Full', status: 'Interrupted', started_by: 'Colleague',
  started_at: '2026-10-11T09:00:00Z', ended_at: null, updated_at: '2026-10-11T10:00:00Z', total_steps: 16,
  done_steps: [1, 2, 3, 4, 5, 6, 7, 8, 9], resume_steps: [10, 11, 12, 13, 14, 15, 16], recheck_steps: [],
  committed: true, newer_than_local: true, resumable: true, briefing_changed: false,
  td_total: 83, td_settled: 40, validated_at: null, validated_by: null, ...over,
});

function mockTimeline(extra: Record<string, unknown>) {
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: [], steps: [], recorded_audits: [], recorded_validated_at: null, latest_validation: null,
    lease_orphaned: false, validation_block: null, repo: null, repo_can_validate: false, ...extra,
  } as never);
}

describe('stepRanges', () => {
  it('compresses consecutive steps', () => {
    expect(stepRanges([1, 2, 3, 5, 9, 10])).toBe('1-3, 5, 9-10');
    expect(stepRanges([])).toBe('');
  });
});

describe('AuditTimeline — the audit run of the repository', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    try { localStorage.clear(); } catch { /* jsdom */ }
    vi.mocked(externalApi.list).mockResolvedValue([]);
    vi.mocked(projectsApi.auditSteps).mockResolvedValue([]);
  });

  it('shows the colleague’s run: steps done, to resume, TD settled', async () => {
    mockTimeline({ repo: repoRun() });
    wrap(<AuditTimeline {...props({
      resumable: {
        id: 'run-elsewhere', last_completed_step: 9, steps_to_redo: [10, 11, 12, 13, 14, 15, 16],
        from_repository: true,
      },
    })} />);
    const summary = await screen.findByTestId('audit-timeline-repo-run-summary');
    expect(summary.textContent).toContain('Colleague');
    expect(summary.textContent).toContain('1-9');
    expect(summary.textContent).toContain('10-16');
    expect(screen.getByTestId('audit-timeline-repo-run-td').textContent).toContain('83');
    expect(screen.getByTestId('audit-timeline-repo-run-td').textContent).toContain('40');
    expect(screen.getByTestId('audit-timeline-repo-run-resumable')).toBeTruthy();
    expect(screen.queryByTestId('audit-timeline-repo-run-validate')).toBeNull();
  });

  it('never says the run can be resumed here when no resume is offered', async () => {
    mockTimeline({ repo: repoRun() });
    wrap(<AuditTimeline {...props({ resumable: null })} />);
    await screen.findByTestId('audit-timeline-repo-run-summary');
    expect(screen.queryByTestId('audit-timeline-repo-run-resumable')).toBeNull();
  });

  it('offers no per-step retry or accept on a run that is still in the repository', async () => {
    mockTimeline({ repo: repoRun() });
    wrap(<AuditTimeline {...props({
      resumable: {
        id: 'run-elsewhere', last_completed_step: 9, steps_to_redo: [10, 11], from_repository: true,
      },
    })} />);
    await screen.findByTestId('audit-timeline-repo-run-summary');
    expect(screen.queryByTestId('audit-timeline-step-retry-10')).toBeNull();
    expect(screen.queryByTestId('audit-timeline-step-accept-10')).toBeNull();
  });

  it('says a step whose file changed is to re-verify, never done', async () => {
    mockTimeline({ repo: repoRun({ done_steps: [1, 2], recheck_steps: [3], resume_steps: [4] }) });
    wrap(<AuditTimeline {...props()} />);
    const summary = await screen.findByTestId('audit-timeline-repo-run-summary');
    expect(summary.textContent).toContain('1-2');
    expect(summary.textContent).toContain('3');
  });

  it('shows nothing when the local run is the project’s state', async () => {
    mockTimeline({ repo: repoRun({ newer_than_local: false }) });
    wrap(<AuditTimeline {...props()} />);
    await waitFor(() => expect(projectsApi.auditTimeline).toHaveBeenCalled());
    expect(screen.queryByTestId('audit-timeline-repo-run')).toBeNull();
  });

  it('offers to validate the repository state only when the server accepts it', async () => {
    const onMarkValid = vi.fn().mockResolvedValue(undefined);
    mockTimeline({
      repo: repoRun({ status: 'Completed', done_steps: [1, 2, 3], resume_steps: [], resumable: false }),
      repo_can_validate: true,
    });
    wrap(<AuditTimeline {...props({ onMarkValid })} />);
    fireEvent.click(await screen.findByTestId('audit-timeline-repo-run-validate'));
    await waitFor(() => expect(onMarkValid).toHaveBeenCalledTimes(1));
  });

  it('shows the reason instead of the action when the gate refuses', async () => {
    mockTimeline({
      repo: repoRun({ status: 'Completed', resume_steps: [], resumable: false }),
      repo_can_validate: false,
      validation_block: 'repo_td_open',
    });
    wrap(<AuditTimeline {...props()} />);
    expect(await screen.findByTestId('audit-timeline-repo-run-block')).toBeTruthy();
    expect(screen.queryByTestId('audit-timeline-repo-run-validate')).toBeNull();
  });
});
