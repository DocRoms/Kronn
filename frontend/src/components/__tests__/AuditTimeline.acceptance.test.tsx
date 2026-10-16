// KT-1169 — acceptance: in every state an audit run can be left in, and under
// every kind of error, the timeline offers an action, every error it shows
// carries its own enabled button, and nothing reads "running" once stopped.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent, act } from '@testing-library/react';
import type { ReactElement } from 'react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi, externalApi } from '../../lib/api';
import { AuditTimeline, type AuditTimelineProps } from '../AuditTimeline';
import type { AgentDetection } from '../../types/generated';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true, path: '/usr/bin/claude',
  version: '1.0.0', latest_version: null, origin: 'host', install_command: null, host_managed: false,
  host_label: null, runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

const step = (index: number, extra: Record<string, unknown> = {}) => ({
  audit_run_id: 'run-1', step_index: index, file_label: `docs/step-${index}.md`,
  started_at: '2026-10-02T10:00:00Z', ended_at: '2026-10-02T10:01:00Z', duration_ms: 1000,
  step_tokens: 10, cumulative_tokens: 10, cli_success: true, step_warning: null,
  step_repaired_from_template: false, ...extra,
});
const failedStep = (index: number) => step(index, { cli_success: false, step_warning: 'placeholders remain' });

interface Timeline {
  status?: string;
  steps?: unknown[];
  orphaned?: boolean;
  validation?: unknown;
}

function mockTimeline({ status = 'Completed', steps = [], orphaned = false, validation = null }: Timeline) {
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: steps.length ? [{ id: 'run-1', kind: 'Full', project_id: 'p1', agent_type: 'ClaudeCode', started_at: '', status, td_total: 0 }] : [],
    steps, recorded_audits: [], recorded_validated_at: null, latest_validation: validation, lease_orphaned: orphaned,
  } as never);
}

function baseProps(over: Partial<AuditTimelineProps> = {}): AuditTimelineProps {
  return {
    projectId: 'p1', auditStatus: 'TemplateInstalled', techDebtCount: 0, agents: [AGENT],
    selectedAgent: 'ClaudeCode', selectedTier: 'reasoning', modelTiers: null, selectedConnectionId: null, onSelect: vi.fn(),
    briefingDone: true, onBriefingSaved: vi.fn(), auditActive: false, liveStep: 0, liveTotal: 0,
    liveFile: '', liveElapsed: null, liveTool: null, liveStartedAt: null, liveToolCalls: null, liveStepTokens: null,
    liveTotalTokens: null, onResumeBriefingDiscussion: null, onCancel: vi.fn(), resumable: null,
    onLaunch: vi.fn(), onRetryStep: vi.fn(), onAcceptStep: vi.fn().mockResolvedValue(undefined),
    onRelaunchFull: vi.fn(), onCleanupLease: vi.fn().mockResolvedValue(undefined),
    validationInProgress: false, onMarkValid: vi.fn().mockResolvedValue(undefined),
    onOpenValidation: vi.fn(), onViewTechDebts: vi.fn(), refreshTrigger: 0, toast: vi.fn(), ...over,
  };
}

const ALL_DONE = Array.from({ length: 3 }, (_, i) => step(i + 1));
const STEP_3_FAILED = [step(1), step(2), failedStep(3)];
const RESUMABLE = { id: 'run-1', steps_to_redo: [3], last_completed_step: 2 };

interface RunState {
  name: string;
  props: Partial<AuditTimelineProps>;
  timeline: Timeline;
  /** The action this state offers, enabled (see docs/architecture/audit-run-states.md). */
  action: string;
}

const RUN_STATES: RunState[] = [
  { name: 'never audited', props: { auditStatus: 'NoTemplate' }, timeline: {}, action: 'audit-timeline-launch' },
  {
    name: 'running', timeline: { status: 'Running', steps: [step(1)] }, action: 'audit-timeline-cancel',
    props: { auditActive: true, liveStep: 2, liveTotal: 3, liveFile: 'docs/step-2.md', liveStartedAt: Date.parse('2026-10-02T09:59:00Z') },
  },
  {
    name: 'cancel not acknowledged', timeline: { status: 'Running', steps: [step(1)] }, action: 'audit-timeline-cancel',
    props: { auditActive: true, cancelPending: true, liveStep: 2, liveTotal: 3, liveFile: 'docs/step-2.md', liveStartedAt: Date.parse('2026-10-02T09:59:00Z') },
  },
  {
    name: 'first audit interrupted, steps to redo', timeline: { status: 'Interrupted', steps: STEP_3_FAILED },
    props: { auditStatus: 'TemplateInstalled', resumable: RESUMABLE }, action: 'audit-timeline-step-retry-3',
  },
  {
    name: 're-audit interrupted, steps to redo', timeline: { status: 'Interrupted', steps: STEP_3_FAILED },
    props: { auditStatus: 'Audited', resumable: RESUMABLE }, action: 'audit-timeline-validation-launch',
  },
  {
    name: 'interrupted, every failed step accepted', action: 'audit-timeline-launch',
    timeline: { status: 'Interrupted', steps: [step(1), step(2), step(3, { cli_success: false, accepted_at: '2026-10-02T11:00:00Z', accepted_by: 'romu' })] },
    props: { auditStatus: 'TemplateInstalled', resumable: { id: 'run-1', steps_to_redo: [], last_completed_step: 3 } },
  },
  {
    name: 'interrupted with nothing done', action: 'audit-timeline-validation-launch',
    timeline: { status: 'Interrupted', steps: [step(1, { ended_at: null, duration_ms: null })] },
    props: { auditStatus: 'TemplateInstalled' },
  },
  {
    name: 'cancelled', action: 'audit-timeline-launch',
    timeline: { status: 'Cancelled', steps: [step(1), step(2, { ended_at: null, duration_ms: null })] },
    props: { auditStatus: 'Audited' },
  },
  { name: 'completed, not validated', timeline: { steps: ALL_DONE }, props: { auditStatus: 'Audited' }, action: 'audit-timeline-launch' },
  {
    name: 'completed, validation finished', action: 'audit-timeline-validation-finished',
    timeline: { steps: ALL_DONE, validation: { discussion_id: 'd1', finished: true } }, props: { auditStatus: 'Audited' },
  },
  { name: 'validated', timeline: { steps: ALL_DONE }, props: { auditStatus: 'Validated' }, action: 'audit-timeline-launch' },
  {
    name: 'orphaned lease', timeline: { status: 'Interrupted', steps: STEP_3_FAILED, orphaned: true },
    props: { auditStatus: 'Audited', resumable: RESUMABLE }, action: 'audit-timeline-lease-release',
  },
  {
    name: 'resumable run unknown', timeline: { steps: ALL_DONE },
    props: { auditStatus: 'Audited', resumableUnknown: true, onReloadResumable: vi.fn() }, action: 'audit-timeline-resumable-reload',
  },
];

type ErrorKind = 'none' | 'launch' | 'timeline';
const ERRORS: ErrorKind[] = ['none', 'launch', 'timeline'];

/** An element is an action when it is an enabled button, or holds one. */
function enabledAction(testId: string): HTMLButtonElement | null {
  const el = screen.queryByTestId(testId);
  if (!el) return null;
  const button = el instanceof HTMLButtonElement ? el : el.querySelector('button');
  return button && !button.disabled ? button : null;
}

describe('AuditTimeline — KT-1169 acceptance: never an error without an action', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    try { localStorage.setItem('kr.audit.timeline.news.0.14.2', '1'); } catch { /* jsdom */ }
    vi.mocked(externalApi.list).mockResolvedValue([]);
    vi.mocked(projectsApi.auditSteps).mockResolvedValue([]);
  });

  const cases = RUN_STATES.flatMap(state => ERRORS.map(error => ({ state, error })));

  it.each(cases.map(c => [c.state.name, c.error, c] as const))('%s × error %s', async (_n, _e, { state, error }) => {
    const retry = vi.fn();
    if (error === 'timeline') vi.mocked(projectsApi.auditTimeline).mockRejectedValue(new Error('offline'));
    else mockTimeline(state.timeline);
    const props = baseProps({
      ...state.props,
      ...(error === 'launch' ? { launchError: { message: 'Audit already running for this project', retry }, onDismissLaunchError: vi.fn() } : {}),
    });
    const { container } = wrap(<AuditTimeline {...props} />);
    await waitFor(() => expect(projectsApi.auditTimeline).toHaveBeenCalled());
    await act(async () => { await Promise.resolve(); });
    container.querySelectorAll<HTMLElement>('.audit-tl-group-head[aria-expanded="false"]').forEach(h => fireEvent.click(h));

    // The state's own action; when the run could not be read, its reload.
    await waitFor(() => {
      const action = enabledAction(state.action)
        ?? (error === 'timeline' ? enabledAction('audit-timeline-load-retry') : null);
      expect(action, state.action).not.toBeNull();
    });

    // Every error shown carries its own enabled button.
    await waitFor(() => {
      const alerts = [...container.querySelectorAll<HTMLElement>('[role="alert"]')];
      if (error !== 'none' && !(error === 'launch' && props.auditActive)) expect(alerts.length).toBeGreaterThan(0);
      for (const alert of alerts) {
        // An action is an enabled button that says what it does.
        const buttons = [...alert.querySelectorAll('button')].filter(b => !b.disabled && !b.hidden && b.textContent?.trim());
        expect(buttons.length, alert.textContent ?? '').toBeGreaterThan(0);
      }
    });
    if (error === 'launch' && !props.auditActive) {
      expect(screen.getByTestId('audit-timeline-launch-error-retry')).toHaveTextContent('Reprendre');
      fireEvent.click(screen.getByTestId('audit-timeline-launch-error-retry'));
      expect(retry).toHaveBeenCalledTimes(1);
    }
    if (error === 'timeline') {
      expect(screen.getByTestId('audit-timeline-load-error')).toBeInTheDocument();
      expect(screen.queryByText(/Aucun audit lancé|No audit launched/)).toBeNull();
    }

    // Nothing reads "running" once stopped; no launch next to a live audit.
    if (!props.auditActive) {
      expect(container.querySelector('.audit-tl-tag.is-running')).toBeNull();
      expect(container.querySelector('.spin')).toBeNull();
    } else {
      expect(screen.getByTestId('audit-timeline-launch')).toBeDisabled();
      expect(screen.queryByTestId('audit-timeline-relaunch-full')).toBeNull();
    }
    // A run to resume, or one whose state is unknown, never offers "re-verify".
    if (props.resumable || props.resumableUnknown || props.auditActive) {
      expect(screen.queryByTestId('audit-timeline-reverify-hint')).toBeNull();
    }
    // Launching waits while the state it must start from is unknown or locked.
    if (props.resumableUnknown || state.timeline.orphaned) {
      if (error !== 'timeline') expect(screen.getByTestId('audit-timeline-launch')).toBeDisabled();
    }
  });

  it('a refused acceptance keeps the form, its reason and a button to try again', async () => {
    mockTimeline({ status: 'Interrupted', steps: STEP_3_FAILED });
    const onAcceptStep = vi.fn()
      .mockRejectedValueOnce(new Error('`docs/step-3.md` still holds template placeholders'))
      .mockResolvedValueOnce(undefined);
    wrap(<AuditTimeline {...baseProps({ auditStatus: 'Audited', resumable: RESUMABLE, onAcceptStep })} />);
    fireEvent.click(await screen.findByTestId('audit-timeline-step-accept-3'));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'kept by hand' } });
    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-accept-confirm-3')); });

    expect(await screen.findByTestId('audit-timeline-accept-error-3')).toHaveTextContent('template placeholders');
    expect(screen.getByRole('textbox')).toHaveValue('kept by hand');
    const again = screen.getByTestId('audit-timeline-accept-confirm-3');
    expect(again).toBeEnabled();
    await act(async () => { fireEvent.click(again); });
    expect(onAcceptStep).toHaveBeenLastCalledWith(3, 'kept by hand');
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-accept-form-3')).toBeNull());
  });

  it('a timeline that failed to load comes back on "Réessayer"', async () => {
    mockTimeline({ status: 'Interrupted', steps: STEP_3_FAILED });
    vi.mocked(projectsApi.auditTimeline).mockRejectedValueOnce(new Error('offline'));
    wrap(<AuditTimeline {...baseProps({ auditStatus: 'Audited', resumable: RESUMABLE })} />);
    fireEvent.click(await screen.findByTestId('audit-timeline-load-retry'));
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-load-error')).toBeNull());
    expect(await screen.findByTestId('audit-timeline-step-3')).toHaveClass('is-failed');
  });
});
