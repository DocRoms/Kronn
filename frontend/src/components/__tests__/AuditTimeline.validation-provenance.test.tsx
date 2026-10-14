// KT-1122 — the timeline shows the server's verdict on validating the latest
// run: the reason when it would refuse (never a button that fails), the
// read-only rule of an instance without the run, and who opened / validated.
import { describe, it, expect, vi } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import type { ReactElement } from 'react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import { AuditTimeline, type AuditTimelineProps } from '../AuditTimeline';
import type { AgentDetection } from '../../types/generated';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true, path: '/usr/bin/claude',
  version: '1.0.0', latest_version: null, origin: 'host', install_command: null, host_managed: false,
  host_label: null, runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

const STEP = {
  audit_run_id: 'run-1', step_index: 1, file_label: 'docs/step-1.md',
  started_at: '2026-10-02T10:00:00Z', ended_at: '2026-10-02T10:01:00Z', duration_ms: 1000,
  step_tokens: 10, cumulative_tokens: 10, cli_success: true, step_warning: null,
  step_repaired_from_template: false,
};

function mockTimeline(extra: Record<string, unknown>, withRun = true) {
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: withRun
      ? [{ id: 'run-1', kind: 'Full', project_id: 'p1', agent_type: 'ClaudeCode', started_at: '', status: 'Completed', td_total: 0 }]
      : [],
    steps: withRun ? [STEP] : [], recorded_audits: [], recorded_validated_at: null,
    recorded_validated_by: null, recorded_validation_started_by: null,
    latest_validation: null, lease_orphaned: false, validation_block: null, ...extra,
  } as never);
}

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

const FINISHED = {
  discussion_id: 'd-val', finished: true, archived: true,
  completed_at: '2026-08-28T21:37:00+00:00', started_by: 'Romu', validated_by: null, validated_at: null,
};

describe('AuditTimeline — validation reason and provenance (KT-1122)', () => {
  it('offers to validate a finished validation the server accepts, and says who opened it', async () => {
    mockTimeline({ latest_validation: FINISHED });
    wrap(<AuditTimeline {...props()} />);

    const block = await screen.findByTestId('audit-timeline-validation-finished');
    expect(within(block).getByRole('button', { name: /Valider l'audit/ })).toBeEnabled();
    expect(within(block).getByTestId('audit-timeline-validation-started-by')).toHaveTextContent('Romu');
    expect(screen.queryByTestId('audit-timeline-validation-block')).toBeNull();
  });

  it('shows the server reason instead of a validate button it would refuse', async () => {
    mockTimeline({ latest_validation: FINISHED, validation_block: 'audit_running' });
    wrap(<AuditTimeline {...props()} />);

    const block = await screen.findByTestId('audit-timeline-validation-finished');
    expect(within(block).getByTestId('audit-timeline-validation-block')).toHaveTextContent(/audit est en cours/i);
    expect(within(block).queryByRole('button', { name: /Valider l'audit/ })).toBeNull();
    // Opening the discussion stays possible: only the action is withheld.
    expect(within(block).getByRole('button', { name: /Voir la discussion de validation/ })).toBeInTheDocument();
  });

  it('says an instance without the run is read-only', async () => {
    mockTimeline({ validation_block: 'no_local_run' }, false);
    wrap(<AuditTimeline {...props()} />);

    const note = await screen.findByTestId('audit-timeline-validation-unavailable');
    expect(note).toHaveTextContent(/Lecture seule/);
    expect(screen.queryByRole('button', { name: /Valider l'audit/ })).toBeNull();
  });

  it('shows who opened and who validated a validated project', async () => {
    mockTimeline({
      recorded_validated_at: '2026-08-29',
      recorded_validated_by: 'Romu',
      recorded_validation_started_by: 'Audit-Bot',
    });
    wrap(<AuditTimeline {...props({ auditStatus: 'Validated' })} />);

    const who = await screen.findByTestId('audit-timeline-validated-by');
    expect(who).toHaveTextContent('Audit-Bot');
    expect(who).toHaveTextContent('Romu');
    expect(who).toHaveTextContent('2026-08-29');
  });
});
