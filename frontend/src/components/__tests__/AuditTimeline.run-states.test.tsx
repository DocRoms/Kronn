// KT-1131 / KT-1134 — every state an audit run can be left in offers an action
// in the timeline, and nothing reads "en cours" while it is not.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
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

const failed = (index: number) => step(index, { cli_success: false, step_warning: 'placeholders remain' });

function props(over: Partial<AuditTimelineProps> = {}): AuditTimelineProps {
  return {
    projectId: 'p1', auditStatus: 'Audited', techDebtCount: 0, agents: [AGENT],
    selectedAgent: 'ClaudeCode', selectedTier: 'reasoning', modelTiers: null, selectedConnectionId: null, onSelect: vi.fn(),
    briefingDone: true, onBriefingSaved: vi.fn(), auditActive: false, liveStep: 0, liveTotal: 0,
    liveFile: '', liveElapsed: null, liveTool: null, liveStartedAt: null, liveToolCalls: null, liveStepTokens: null,
    liveTotalTokens: null, onResumeBriefingDiscussion: null, onCancel: vi.fn(), resumable: null,
    onLaunch: vi.fn(), validationInProgress: false, onValidate: vi.fn(),
    onMarkValid: vi.fn().mockResolvedValue(undefined), onOpenValidation: vi.fn(), onViewTechDebts: vi.fn(),
    onRetryStep: vi.fn(), onAcceptStep: vi.fn().mockResolvedValue(undefined), onRelaunchFull: vi.fn(),
    onCleanupLease: vi.fn().mockResolvedValue(undefined),
    refreshTrigger: 0, toast: vi.fn(), ...over,
  };
}

function mockTimeline(status: string, steps: unknown[], extra: Record<string, unknown> = {}) {
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: steps.length || status !== 'none'
      ? [{ id: 'run-1', kind: 'Full', project_id: 'p1', agent_type: 'ClaudeCode', started_at: '', status, td_total: 0 }]
      : [],
    steps, recorded_audits: [], recorded_validated_at: null, latest_validation: null, lease_orphaned: false, ...extra,
  } as never);
}

async function openGroups(container: HTMLElement) {
  await waitFor(() => expect(container.querySelector('.audit-tl-group-head')).not.toBeNull());
  container.querySelectorAll<HTMLElement>('.audit-tl-group-head[aria-expanded="false"]').forEach(head => fireEvent.click(head));
}

/** The validation phase (phase 3) as rendered. */
function validationPhase(container: HTMLElement): HTMLElement {
  const title = within(container).getByText(fr['auditTimeline.validation.title']);
  return title.closest('.audit-tl-phase') as HTMLElement;
}

const resumable = (stepsToRedo: number[]) => ({ id: 'run-1', steps_to_redo: stepsToRedo, last_completed_step: 2 });

describe('AuditTimeline — every run state offers an action', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    try { localStorage.clear(); } catch { /* jsdom */ }
    vi.mocked(externalApi.list).mockResolvedValue([]);
    vi.mocked(projectsApi.auditSteps).mockResolvedValue([]);
  });

  // state → what the user can do. One row per terminal (or stuck) state.
  const TABLE: {
    state: string;
    auditStatus: string;
    runStatus: string;
    steps: unknown[];
    over?: Partial<AuditTimelineProps>;
    action: string;
  }[] = [
    { state: 'never audited', auditStatus: 'NoTemplate', runStatus: 'none', steps: [], action: fr['audit.startFullAudit'] },
    { state: 'completed, to validate', auditStatus: 'Audited', runStatus: 'Completed', steps: [step(1), step(2)], action: fr['audit.validate'] },
    { state: 'validated', auditStatus: 'Validated', runStatus: 'Completed', steps: [step(1), step(2)], action: fr['auditTimeline.relaunch'] },
    { state: 'cancelled', auditStatus: 'Audited', runStatus: 'Cancelled', steps: [step(1)], action: fr['auditTimeline.relaunch'] },
    {
      state: 'first audit interrupted', auditStatus: 'TemplateInstalled', runStatus: 'Interrupted',
      steps: [step(1), step(2), failed(3)], over: { resumable: resumable([3]) },
      action: fr['audit.resumeRedoSteps'].replace('{0}', '3'),
    },
    {
      state: 'restarted with nothing to resume', auditStatus: 'TemplateInstalled', runStatus: 'Interrupted',
      steps: [step(1, { ended_at: null })], action: fr['audit.startFullAudit'],
    },
    {
      state: 're-audit interrupted', auditStatus: 'Audited', runStatus: 'Interrupted',
      steps: [step(1), step(2), failed(3)], over: { resumable: resumable([3]) },
      action: fr['audit.resumeRedoSteps'].replace('{0}', '3'),
    },
    {
      state: 'every failure accepted', auditStatus: 'Audited', runStatus: 'Interrupted',
      steps: [step(1), step(2), failed(3)].map(s => (s.step_index === 3 ? { ...s, accepted_at: '2026-10-02T11:00:00Z', accepted_by: 'romu' } : s)),
      over: { resumable: resumable([]) }, action: fr['auditTimeline.audit.finalize'],
    },
  ];

  it.each(TABLE)('$state → « $action » is offered', async ({ auditStatus, runStatus, steps, over, action }) => {
    mockTimeline(runStatus, steps);
    wrap(<AuditTimeline {...props({ auditStatus, ...over })} />);
    const buttons = await screen.findAllByRole('button', { name: new RegExp(action.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) });
    expect(buttons.some(b => !(b as HTMLButtonElement).disabled)).toBe(true);
  });

  it('a running audit offers its cancel, and validation never reads « En cours » meanwhile', async () => {
    mockTimeline('Running', [step(1)]);
    const { container } = wrap(<AuditTimeline {...props({ auditActive: true, validationInProgress: true, liveTotal: 16, liveStep: 2 })} />);
    expect(await screen.findByTestId('audit-timeline-cancel')).toHaveTextContent(fr['audit.cancelAudit']);
    const phase = validationPhase(container);
    expect(phase.querySelector('.audit-tl-phase-head .audit-tl-tag')).toHaveTextContent(fr['auditTimeline.status.waitingAudit']);
    expect(within(phase).queryByText(fr['audit.resumeValidation'])).toBeNull();
  });

  it('a cancel the backend has not acknowledged stays pending, with a retry, never idle', async () => {
    mockTimeline('Running', [step(1)]);
    const onCancel = vi.fn();
    wrap(<AuditTimeline {...props({ auditActive: true, cancelPending: true, onCancel, liveTotal: 16 })} />);
    expect(await screen.findByTestId('audit-timeline-cancel-pending')).toHaveTextContent(fr['auditTimeline.cancelPending']);
    fireEvent.click(screen.getByTestId('audit-timeline-cancel'));
    expect(onCancel).toHaveBeenCalled();
    expect(screen.getByTestId('audit-timeline-cancel')).toHaveTextContent(fr['auditTimeline.cancelRetry']);
  });

  it('a lease nobody could settle is signalled with the action that releases it', async () => {
    mockTimeline('Interrupted', [step(1)], { lease_orphaned: true });
    const onCleanupLease = vi.fn().mockResolvedValue(undefined);
    wrap(<AuditTimeline {...props({ onCleanupLease })} />);
    expect(await screen.findByTestId('audit-timeline-lease-orphaned')).toHaveTextContent(fr['auditTimeline.lease.stuck']);
    expect(screen.getByTestId('audit-timeline-launch')).toBeDisabled();
    fireEvent.click(screen.getByTestId('audit-timeline-lease-release'));
    await waitFor(() => expect(onCleanupLease).toHaveBeenCalled());
  });

  it('a failed step retries alone, or is accepted with a reason', async () => {
    mockTimeline('Interrupted', [step(1), step(2), failed(3)]);
    const onRetryStep = vi.fn();
    const onAcceptStep = vi.fn().mockResolvedValue(undefined);
    const { container } = wrap(<AuditTimeline {...props({ resumable: resumable([3]), onRetryStep, onAcceptStep })} />);
    await openGroups(container);
    fireEvent.click(await screen.findByTestId('audit-timeline-step-retry-3'));
    expect(onRetryStep).toHaveBeenCalledWith(3);
    // Done steps carry no step action.
    expect(screen.queryByTestId('audit-timeline-step-actions-1')).toBeNull();

    fireEvent.click(screen.getByTestId('audit-timeline-step-accept-3'));
    const form = screen.getByTestId('audit-timeline-accept-form-3');
    fireEvent.change(within(form).getByRole('textbox'), { target: { value: ' kept by hand ' } });
    fireEvent.click(screen.getByTestId('audit-timeline-accept-confirm-3'));
    await waitFor(() => expect(onAcceptStep).toHaveBeenCalledWith(3, 'kept by hand'));
  });

  it('an interrupted re-audit sends validation to the failed steps first, never to an unlinked discussion', async () => {
    mockTimeline('Interrupted', [step(1), step(2), failed(3)]);
    const onValidate = vi.fn();
    const onLaunch = vi.fn();
    const { container } = wrap(<AuditTimeline {...props({ resumable: resumable([3]), onValidate, onLaunch })} />);
    const blocked = await screen.findByTestId('audit-timeline-validation-blocked');
    expect(blocked).toHaveTextContent(fr['auditTimeline.validation.afterRun']);
    expect(within(validationPhase(container)).queryByText(fr['audit.validate'])).toBeNull();
    fireEvent.click(screen.getByTestId('audit-timeline-validation-launch'));
    expect(onLaunch).toHaveBeenCalled();
    expect(onValidate).not.toHaveBeenCalled();
  });

  it('relaunching the whole audit over an interrupted run asks first, offering retry or acceptance', async () => {
    mockTimeline('Interrupted', [step(1), step(2), failed(3)]);
    const onRelaunchFull = vi.fn();
    const onLaunch = vi.fn();
    const { container } = wrap(<AuditTimeline {...props({ resumable: resumable([3]), onRelaunchFull, onLaunch })} />);
    fireEvent.click(await screen.findByTestId('audit-timeline-relaunch-full'));
    expect(onRelaunchFull).not.toHaveBeenCalled();
    const dialog = screen.getByTestId('audit-timeline-relaunch-confirm');
    expect(dialog).toHaveTextContent(fr['auditTimeline.confirmRelaunch.body'].replace('{0}', '1'));

    // « Voir les étapes en échec » opens the group holding them.
    fireEvent.click(screen.getByTestId('audit-timeline-relaunch-confirm-review'));
    expect(screen.queryByTestId('audit-timeline-relaunch-confirm')).toBeNull();
    expect(await screen.findByTestId('audit-timeline-step-accept-3')).toBeInTheDocument();
    expect(container.querySelector('.audit-tl-group-head')).toHaveAttribute('aria-expanded', 'true');

    fireEvent.click(screen.getByTestId('audit-timeline-relaunch-full'));
    fireEvent.click(screen.getByTestId('audit-timeline-relaunch-confirm-resume'));
    expect(onLaunch).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByTestId('audit-timeline-relaunch-full'));
    fireEvent.click(screen.getByTestId('audit-timeline-relaunch-confirm-full'));
    expect(onRelaunchFull).toHaveBeenCalledTimes(1);
  });

  it('an accepted step reads « Accepté » with who and why, counted done, apart from unchanged', async () => {
    mockTimeline('Interrupted', [
      step(1), step(2, { unchanged: true }),
      { ...failed(3), accepted_at: '2026-10-02T11:00:00Z', accepted_by: 'romu', accepted_reason: 'kept by hand' },
    ]);
    const { container } = wrap(<AuditTimeline {...props({ resumable: resumable([]) })} />);
    await openGroups(container);
    const accepted = await screen.findByTestId('audit-timeline-step-3');
    expect(accepted).toHaveClass('is-accepted');
    expect(accepted.querySelector('.audit-tl-step-side .audit-tl-tag')).toHaveTextContent(fr['auditTimeline.status.accepted']);
    expect(screen.getByTestId('audit-timeline-step-accepted-3')).toHaveTextContent('romu');
    expect(screen.getByTestId('audit-timeline-step-accepted-3')).toHaveTextContent('kept by hand');
    expect(screen.getByTestId('audit-timeline-step-2')).toHaveClass('is-unchanged');
    expect(screen.getByTestId('audit-timeline-step-1')).toHaveClass('is-done');
    expect(container.querySelector('[role="progressbar"]')).toHaveAttribute('aria-valuenow', '3');
    expect(screen.queryByTestId('audit-timeline-step-actions-3')).toBeNull();
  });
});
