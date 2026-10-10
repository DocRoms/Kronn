// KT-1130 — the timeline shows an `unchanged` step as a neutral « Inchangé »,
// never as a failure to resume.
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
import fr from '../../lib/i18n/locales/fr';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true, path: '/usr/bin/claude',
  version: '1.0.0', latest_version: null, origin: 'host', install_command: null, host_managed: false,
  host_label: null, runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

const step = (index: number, extra: Record<string, unknown> = {}) => ({
  audit_run_id: 'run-1', step_index: index, file_label: `docs/step-${index}.md`,
  started_at: '2026-10-02T10:00:00Z', ended_at: '2026-10-02T10:01:00Z', duration_ms: 61_000,
  step_tokens: 10, cumulative_tokens: 10, cli_success: true, step_warning: null,
  step_repaired_from_template: false, unchanged: false, ...extra,
});

function props(over: Partial<AuditTimelineProps> = {}): AuditTimelineProps {
  return {
    projectId: 'p1', auditStatus: 'Audited', techDebtCount: 0, agents: [AGENT],
    selectedAgent: 'ClaudeCode', selectedTier: 'reasoning', modelTiers: null, selectedConnectionId: null, onSelect: vi.fn(),
    briefingDone: true, onBriefingSaved: vi.fn(), auditActive: false, liveStep: 0, liveTotal: 0,
    liveFile: '', liveElapsed: null, liveTool: null, liveStartedAt: null, liveToolCalls: null, liveStepTokens: null,
    liveTotalTokens: null, onResumeBriefingDiscussion: null, onCancel: vi.fn(), resumable: null,
    onLaunch: vi.fn(), validationInProgress: false, onValidate: vi.fn(),
    onMarkValid: vi.fn().mockResolvedValue(undefined), onOpenValidation: vi.fn(), onViewTechDebts: vi.fn(),
    refreshTrigger: 0, toast: vi.fn(), ...over,
  };
}

function mockSteps(runs: { id: string; kind: string }[], steps: unknown[]) {
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: runs.map(r => ({ project_id: 'p1', agent_type: 'ClaudeCode', started_at: '', status: 'Completed', td_total: 0, ...r })),
    steps, recorded_audits: [], recorded_validated_at: null, latest_validation: null,
  } as never);
}

async function openGroups(container: HTMLElement) {
  await waitFor(() => expect(container.querySelector('.audit-tl-group-head')).not.toBeNull());
  container.querySelectorAll<HTMLElement>('.audit-tl-group-head').forEach(head => fireEvent.click(head));
}

describe('AuditTimeline — unchanged steps', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    try { localStorage.clear(); } catch { /* jsdom */ }
    vi.mocked(externalApi.list).mockResolvedValue([]);
    vi.mocked(projectsApi.auditSteps).mockResolvedValue([]);
  });

  it('a re-audit whose steps are unchanged reads « Inchangé », counted done, never « Échec »', async () => {
    const steps = [step(1, { unchanged: true }), step(2, { unchanged: true }), step(3)];
    mockSteps([{ id: 'run-1', kind: 'Full' }], steps);
    const { container } = wrap(<AuditTimeline {...props()} />);
    await openGroups(container);

    const unchanged = await screen.findByTestId('audit-timeline-step-1');
    expect(unchanged).toHaveClass('is-unchanged');
    expect(unchanged).not.toHaveClass('is-failed');
    expect(unchanged.querySelector('.audit-tl-tag')).toHaveTextContent(fr['auditTimeline.status.unchanged']);
    expect(unchanged.querySelector('.audit-tl-reason')).toHaveTextContent(fr['auditTimeline.reason.verified']);
    expect(screen.getByTestId('audit-timeline-step-3')).toHaveClass('is-done');
    expect(container).not.toHaveTextContent(fr['auditTimeline.status.failed']);
    expect(screen.queryByText(fr['auditTimeline.audit.resume'])).toBeNull();
    // Every step is done: the progress bar is full.
    expect(container.querySelector('[role="progressbar"]')).toHaveAttribute('aria-valuenow', '3');
  });

  it('a partial refresh that found its section unchanged shows it unchanged too', async () => {
    mockSteps([{ id: 'partial-1', kind: 'Partial' }, { id: 'run-1', kind: 'Full' }], [
      step(2, { audit_run_id: 'partial-1', cli_success: false, unchanged: true }),
      step(1), step(2),
    ]);
    const { container } = wrap(<AuditTimeline {...props()} />);
    await openGroups(container);
    await waitFor(() => expect(screen.getByTestId('audit-timeline-step-2')).toHaveClass('is-unchanged'));
  });
});
