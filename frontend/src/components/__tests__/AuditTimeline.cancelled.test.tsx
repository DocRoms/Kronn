// KT-1211 — a cancelled run is shown as cancelled (date, step) with the next
// action said, a stopped step says what became of its document, an unsound
// acceptance asks for an explicit override, and a source drift is kept.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import type { ReactElement } from 'react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi, externalApi } from '../../lib/api';
import { AuditTimeline, OVERRIDE_REQUIRED_PREFIX, type AuditTimelineProps } from '../AuditTimeline';
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
    projectId: 'p1', auditStatus: 'TemplateInstalled', agents: [AGENT],
    selectedAgent: 'ClaudeCode', selectedTier: 'reasoning', modelTiers: null, selectedConnectionId: null, onSelect: vi.fn(),
    briefingDone: true, onBriefingSaved: vi.fn(), auditActive: false, liveStep: 0, liveTotal: 0,
    liveFile: '', liveElapsed: null, liveTool: null, liveStartedAt: null, liveToolCalls: null, liveStepTokens: null,
    liveTotalTokens: null, onResumeBriefingDiscussion: null, onCancel: vi.fn(), resumable: null,
    onLaunch: vi.fn(), validationInProgress: false,
    onMarkValid: vi.fn().mockResolvedValue(undefined), onOpenValidation: vi.fn(), techDebtEntry: null,
    onRetryStep: vi.fn(), onAcceptStep: vi.fn().mockResolvedValue(undefined), onRelaunchFull: vi.fn(),
    onCleanupLease: vi.fn().mockResolvedValue(undefined),
    refreshTrigger: 0, toast: vi.fn(), ...over,
  };
}

function mockTimeline(run: Record<string, unknown>, steps: unknown[]) {
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: [{
      id: 'run-1', kind: 'Full', project_id: 'p1', agent_type: 'ClaudeCode',
      started_at: '2026-10-03T08:00:00Z', ended_at: '2026-10-12T10:00:00Z', td_total: 0, ...run,
    }],
    steps, recorded_audits: [], recorded_validated_at: null, latest_validation: null, lease_orphaned: false,
  } as never);
}

async function openGroups(container: HTMLElement) {
  await waitFor(() => expect(container.querySelector('.audit-tl-group-head')).not.toBeNull());
  container.querySelectorAll<HTMLElement>('.audit-tl-group-head[aria-expanded="false"]').forEach(head => fireEvent.click(head));
}

describe('AuditTimeline — cancelled runs, half-written targets, drift', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    try { localStorage.clear(); } catch { /* jsdom */ }
    vi.mocked(externalApi.list).mockResolvedValue([]);
    vi.mocked(projectsApi.auditSteps).mockResolvedValue([]);
  });

  it('the real case: cancelled during step 1 reads « annulé », not « interrompu », and offers « Relancer l\'audit »', async () => {
    // Step 1 started, never ended; the run settled Cancelled (nothing completed).
    mockTimeline({ status: 'Cancelled', cancelled_at_step: 1 }, [step(1, { ended_at: null, cli_success: null })]);
    const { container } = wrap(<AuditTimeline {...props()} />);
    const note = await screen.findByTestId('audit-timeline-cancelled');
    expect(note).toHaveTextContent(/Audit annulé le .* pendant l'étape 1/);
    expect(screen.getByTestId('audit-timeline-cancelled-next')).toHaveTextContent(fr['auditTimeline.cancelled.nextRelaunch']);
    expect(screen.getByTestId('audit-timeline-launch')).toHaveTextContent(fr['auditTimeline.cancelled.relaunch']);
    await openGroups(container);
    // The step is never the dead « interrompu » one.
    expect(screen.getByTestId('audit-timeline-step-cancelled-1')).toHaveTextContent(fr['auditTimeline.reason.cancelledHere']);
    expect(screen.queryByText(fr['auditTimeline.reason.interrupted'])).toBeNull();
  });

  it('without a briefing the next action says to finish it first', async () => {
    mockTimeline({ status: 'Cancelled', cancelled_at_step: 1 }, [step(1, { ended_at: null })]);
    wrap(<AuditTimeline {...props({ briefingDone: false })} />);
    expect(await screen.findByTestId('audit-timeline-cancelled-next')).toHaveTextContent(fr['auditTimeline.cancelled.nextBriefing']);
  });

  it('a cancel after a completed step is resumable and says where it resumes', async () => {
    mockTimeline({ status: 'Interrupted', cancelled_at_step: 4 }, [step(1), step(2), step(3), step(4, { ended_at: null })]);
    wrap(<AuditTimeline {...props({ resumable: { id: 'run-1', steps_to_redo: [], last_completed_step: 3 } as never })} />);
    expect(await screen.findByTestId('audit-timeline-cancelled')).toHaveTextContent(/pendant l'étape 4/);
    expect(screen.getByTestId('audit-timeline-cancelled-next')).toHaveTextContent('étape 4');
  });

  it('a run nobody cancelled is not labelled cancelled', async () => {
    mockTimeline({ status: 'Interrupted' }, [step(1), step(2, { ended_at: null })]);
    wrap(<AuditTimeline {...props({ resumable: { id: 'run-1', steps_to_redo: [2], last_completed_step: 1 } as never })} />);
    await waitFor(() => expect(projectsApi.auditTimeline).toHaveBeenCalled());
    expect(screen.queryByTestId('audit-timeline-cancelled')).toBeNull();
  });

  it('a failed step says what became of its document', async () => {
    mockTimeline({ status: 'Interrupted' }, [
      step(1), step(2, { cli_success: false, step_warning: 'stopped during step 2', target_state: 'restored' }),
      step(3, { cli_success: false, step_warning: 'output looks truncated', target_state: 'partial' }),
    ]);
    const { container } = wrap(<AuditTimeline {...props({ resumable: { id: 'run-1', steps_to_redo: [2, 3], last_completed_step: 1 } as never })} />);
    await openGroups(container);
    expect(await screen.findByTestId('audit-timeline-step-target-2')).toHaveTextContent(fr['auditTimeline.target.restored']);
    expect(screen.getByTestId('audit-timeline-step-target-3')).toHaveTextContent(fr['auditTimeline.target.partial']);
  });

  it('an unsound acceptance shows the diff and needs an explicit override', async () => {
    mockTimeline({ status: 'Interrupted' }, [step(1), step(3, { cli_success: false, step_warning: 'output looks truncated' })]);
    const onAcceptStep = vi.fn()
      .mockRejectedValueOnce(new Error(`${OVERRIDE_REQUIRED_PREFIX} step 3 cannot be accepted\n\n-old line\n+new line`))
      .mockResolvedValueOnce(undefined);
    const { container } = wrap(<AuditTimeline {...props({
      resumable: { id: 'run-1', steps_to_redo: [3], last_completed_step: 1 } as never, onAcceptStep,
    })} />);
    await openGroups(container);
    fireEvent.click(await screen.findByTestId('audit-timeline-step-accept-3'));
    fireEvent.click(screen.getByTestId('audit-timeline-accept-confirm-3'));
    const ask = await screen.findByTestId('audit-timeline-accept-override-3');
    expect(ask).toHaveTextContent('+new line');
    expect(onAcceptStep).toHaveBeenLastCalledWith(3, null);
    // The override is a second, explicit click.
    fireEvent.click(screen.getByTestId('audit-timeline-accept-confirm-3'));
    await waitFor(() => expect(onAcceptStep).toHaveBeenLastCalledWith(3, null, true));
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-accept-override-3')).toBeNull());
  });

  it('a source drift saved on the run is shown, with the redo of the inherited steps', async () => {
    mockTimeline(
      { status: 'Interrupted', resume_drift: 'Sources changed since step 2: src/lib.rs' },
      [step(1, { carried_from_run_id: 'run-0' }), step(2, { carried_from_run_id: 'run-0' }), step(3, { cli_success: false, step_warning: 'placeholders remain' })],
    );
    const onRedoSteps = vi.fn();
    wrap(<AuditTimeline {...props({
      resumable: { id: 'run-1', steps_to_redo: [3], last_completed_step: 2 } as never, onRedoSteps,
    })} />);
    const drift = await screen.findByTestId('audit-timeline-resume-drift');
    expect(drift).toHaveTextContent('Sources changed since step 2');
    fireEvent.click(within(drift).getByTestId('audit-timeline-redo-drifted'));
    expect(onRedoSteps).toHaveBeenCalledWith([1, 2]);
  });

  it('KT-1353: the citation problems the gate let through and the replacements it applied are listed', async () => {
    const notes = JSON.stringify({
      preexisting: [{ path: 'docs/tech-debt/TD-1.md', message: 'path does not exist `vendor/x.php`' }],
      autofixes: [{ step: 10, path: 'docs/AGENTS.md', kind: 'link_base', from: 'docs/operations/', to: 'operations' }],
    });
    mockTimeline({ status: 'Completed', documentary_notes: notes }, [step(1)]);
    wrap(<AuditTimeline {...props()} />);
    const block = await screen.findByTestId('audit-timeline-documentary-notes');
    const held = within(block).getByTestId('audit-timeline-documentary-preexisting');
    expect(held).toHaveTextContent(fr['auditTimeline.docnotes.preexisting'].replace('{0}', '1'));
    expect(held).toHaveTextContent('docs/tech-debt/TD-1.md');
    const fixed = within(block).getByTestId('audit-timeline-documentary-autofixes');
    expect(fixed).toHaveTextContent(fr['auditTimeline.docnotes.autofixes'].replace('{0}', '1'));
    expect(fixed).toHaveTextContent('operations');
  });

  it('KT-1353: nothing is shown without notes, or with notes that cannot be read', async () => {
    mockTimeline({ status: 'Completed', documentary_notes: '{not json' }, [step(1)]);
    const { container } = wrap(<AuditTimeline {...props()} />);
    await waitFor(() => expect(container.querySelector('.audit-tl-group-head')).not.toBeNull());
    expect(screen.queryByTestId('audit-timeline-documentary-notes')).toBeNull();
  });
});
