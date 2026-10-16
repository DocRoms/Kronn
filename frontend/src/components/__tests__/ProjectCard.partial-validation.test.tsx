// Partial refresh → scoped validation UX (Codex A5 lot 2).
//
// A fully-successful partial creates a validation discussion scoped to the
// refreshed sections. The card must surface it exactly like the Full flow:
// refetch discussions, toast, open + NAVIGATE — without auto-running the
// agent (the backend already spawned it post-commit). An interrupted
// refresh must never toast green, and a user cancel must not fall into the
// interrupted branch. A refresh that rewrote nothing completed (KT-1207).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, screen, within } from '@testing-library/react';
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
import type { PartialDoneInfo } from '../../lib/api';
import type { Project, AgentDetection, DriftCheckResponse, Discussion } from '../../types/generated';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-partial',
  name: 'partial-target',
  path: '/repos/partial-target',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'Audited',
  briefing_notes: '# Project Briefing', ai_todo_count: 0, tech_debt_count: 3, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true,
  path: '/usr/bin/claude', version: '1.0.0', latest_version: null, origin: 'host',
  install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

const CODEX_AGENT: AgentDetection = {
  ...AGENT,
  name: 'Codex', agent_type: 'Codex', path: '/usr/bin/codex',
};

const DRIFT: DriftCheckResponse = {
  audit_date: '2026-07-20',
  stale_sections: [
    { ai_file: 'docs/repo-map.md', audit_step: 3, changed_sources: ['src/x.rs'] },
  ],
  fresh_sections: [],
  total_sections: 1,
  sources_state: 'needs_reverification',
  broken_citations: 0,
  placeholder_docs: [],
};

function renderCard(overrides: Partial<Record<string, unknown>> = {}) {
  const props = {
    project: PROJECT, isOpen: true, onToggleOpen: noop, discussions: [],
    driftStatus: DRIFT, agents: [AGENT], allSkills: [], mcpConfigs: [],
    workflows: [], configLanguage: 'fr',
    toast: vi.fn(), onNavigate: vi.fn(), onSetDiscPrefill: noop,
    onAutoRunDiscussion: vi.fn(), onOpenDiscussion: vi.fn(),
    onRefetch: vi.fn(), onRefetchDiscussions: vi.fn(), onRefetchSkills: noop,
    onRefetchDrift: vi.fn(),
    ...overrides,
  };
  return { ...render(<ProjectCard {...(props as any)} />), props };
}

beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
  vi.mocked(projectsApi.auditResumable).mockResolvedValue(null);
});
afterEach(() => { vi.restoreAllMocks(); });

async function clickUpdate() {
  const verify = await screen.findByTestId('project-revalidate');
  await act(async () => { fireEvent.click(verify); await Promise.resolve(); });
  const btn = await screen.findByTestId('revalidate-stale-update');
  await act(async () => { fireEvent.click(btn); await Promise.resolve(); });
}

async function openVerification() {
  const verify = await screen.findByTestId('project-revalidate');
  await act(async () => { fireEvent.click(verify); await Promise.resolve(); });
  return screen.findByTestId('revalidate-panel');
}

describe('ProjectCard — partial refresh validation UX', () => {
  it('complete + discussion id: refetch, toast, open AND navigate — no auto-run', async () => {
    vi.mocked(projectsApi.partialAuditStream).mockImplementation(async (_id, _req, handlers) => {
      handlers.onValidationCreated?.('d-scoped');
      handlers.onDone({
        status: 'complete', discussionId: 'd-scoped', auditRunId: 'run-1',
        succeededSteps: [3], unchangedSteps: [], failedSteps: [],
      });
    });
    const { props } = renderCard();
    await clickUpdate();
    expect(props.onRefetchDiscussions).toHaveBeenCalled();
    expect(props.toast).toHaveBeenCalledWith(expect.stringContaining('audit.partialValidationCreated'), 'success');
    // Opening the discussion is the navigation: it lands on its address.
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('d-scoped');
    expect(props.onNavigate).not.toHaveBeenCalled();
    expect(props.onAutoRunDiscussion).not.toHaveBeenCalled();
  });

  it('interrupted: honest error toast, never a green one', async () => {
    vi.mocked(projectsApi.partialAuditStream).mockImplementation(async (_id, _req, handlers) => {
      handlers.onDone({
        status: 'interrupted', discussionId: null, auditRunId: 'run-3',
        succeededSteps: [], unchangedSteps: [], failedSteps: [3],
      });
    });
    const { props } = renderCard();
    await clickUpdate();
    expect(props.toast).toHaveBeenCalledWith(expect.stringContaining('audit.partialInterrupted'), 'error');
    const greens = (props.toast as ReturnType<typeof vi.fn>).mock.calls
      .filter(([, kind]) => kind === 'success');
    expect(greens).toHaveLength(0);
    expect(props.onOpenDiscussion).not.toHaveBeenCalled();
  });

  it('no_change: re-verified, the run completed — green toast, no discussion to open', async () => {
    vi.mocked(projectsApi.partialAuditStream).mockImplementation(async (_id, _req, handlers) => {
      handlers.onDone({
        status: 'no_change', discussionId: null, auditRunId: 'run-2',
        succeededSteps: [], unchangedSteps: [3], failedSteps: [],
      });
    });
    const { props } = renderCard();
    await clickUpdate();
    expect(props.toast).toHaveBeenCalledWith('audit.partialReverified 1', 'success');
    expect(props.onOpenDiscussion).not.toHaveBeenCalled();
    expect(props.onRefetchDrift).toHaveBeenCalledWith(PROJECT.id);
  });

  it('no_change with a discussion: opens the validation of the re-verified sections', async () => {
    vi.mocked(projectsApi.partialAuditStream).mockImplementation(async (_id, _req, handlers) => {
      handlers.onDone({
        status: 'no_change', discussionId: 'd-reverified', auditRunId: 'run-4',
        succeededSteps: [], unchangedSteps: [3], failedSteps: [],
      });
    });
    const { props } = renderCard();
    await clickUpdate();
    expect(props.toast).toHaveBeenCalledWith('audit.partialReverifiedValidation 1', 'success');
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('d-reverified');
    expect(props.onNavigate).toHaveBeenCalledWith('discussions');
  });

  it('interrupted with done steps: error toast naming their validation, no navigation', async () => {
    vi.mocked(projectsApi.partialAuditStream).mockImplementation(async (_id, _req, handlers) => {
      handlers.onDone({
        status: 'interrupted', discussionId: 'd-done', auditRunId: 'run-5',
        succeededSteps: [3], unchangedSteps: [], failedSteps: [8],
      });
    });
    const { props } = renderCard();
    await clickUpdate();
    expect(props.toast).toHaveBeenCalledWith('audit.partialInterruptedValidation 1', 'error');
    expect(props.onRefetchDiscussions).toHaveBeenCalled();
    expect(props.onNavigate).not.toHaveBeenCalledWith('discussions');
  });

  it('« Vérifié, inchangé » acknowledges every stale section once, then refetches the drift', async () => {
    let release: (() => void) | undefined;
    vi.mocked(projectsApi.acknowledgeDrift).mockImplementation(
      () => new Promise((resolve) => { release = () => resolve(DRIFT); }),
    );
    const { props } = renderCard({
      driftStatus: {
        ...DRIFT,
        stale_sections: [
          ...DRIFT.stale_sections,
          { ai_file: 'docs/repo-map.md', audit_step: 9, changed_sources: [] },
          { ai_file: 'docs/glossary.md', audit_step: 2, changed_sources: [] },
        ],
      },
    });
    await openVerification();
    const btn = await screen.findByTestId('revalidate-stale-ack');
    await act(async () => { fireEvent.click(btn); fireEvent.click(btn); await Promise.resolve(); });
    expect(projectsApi.acknowledgeDrift).toHaveBeenCalledTimes(1);
    expect(projectsApi.acknowledgeDrift).toHaveBeenCalledWith(PROJECT.id, ['docs/repo-map.md', 'docs/glossary.md']);
    await act(async () => { release?.(); await Promise.resolve(); await Promise.resolve(); });
    expect(props.toast).toHaveBeenCalledWith('audit.driftAckDone 2', 'success');
    expect(props.onRefetchDrift).toHaveBeenCalledWith(PROJECT.id);
  });

  it('a refused acknowledgement says why and keeps the button', async () => {
    vi.mocked(projectsApi.acknowledgeDrift).mockRejectedValue(new Error('An audit is currently running'));
    const { props } = renderCard();
    await openVerification();
    const btn = await screen.findByTestId('revalidate-stale-ack');
    await act(async () => { fireEvent.click(btn); await Promise.resolve(); await Promise.resolve(); });
    expect(props.toast).toHaveBeenCalledWith(expect.stringContaining('audit.driftAckFailed'), 'error');
    expect(props.onRefetchDrift).not.toHaveBeenCalled();
    expect(screen.getByTestId('revalidate-stale-ack')).not.toBeDisabled();
  });

  it('opens and loads reconfirmation details through ProjectCard → Verify docs → Reconfirm', async () => {
    vi.mocked(projectsApi.revalidationPlan).mockResolvedValue({
      sources_state: 'clean', sections: [], agent_steps: [], reanchor_count: 0, human_count: 0, unanchored: [], relevance: { sections: 0, anchors_intact: 0, outdated: 0, no_longer_relevant: 0, unanchored: 0, untouched_stale: 0, missing_references: 0, prose_missing: 0, anchored_intact_pct: null },
    });
    vi.mocked(projectsApi.docProvenance).mockImplementation(async (_id, detail = false) => ({
      to_reconfirm: 1,
      docs: detail ? [{
        doc: 'docs/repo-map.md', state: 'to_reconfirm', reason: 'audit_validation', scope: 'doc',
        validated_at: '2026-10-01T10:00:00Z', sections: [],
        kronn_writes: [{ writer: 'partial_audit', at: '2026-10-02T10:00:00Z' }],
        changes: [{ section: null, before: '# Before', after: '# After', truncated: false }],
      }] : [],
    } as never));
    renderCard({
      driftStatus: {
        ...DRIFT, stale_sections: [], total_sections: 0, sources_state: 'clean', broken_citations: 0,
      },
    });

    const verifyPanel = await openVerification();
    fireEvent.click(within(verifyPanel).getByTestId('doc-state-reconfirm'));

    const reconfirmPanel = await screen.findByTestId('doc-reconfirm-panel');
    expect(await within(reconfirmPanel).findByText('docs/repo-map.md')).toBeInTheDocument();
    expect(projectsApi.docProvenance).toHaveBeenCalledWith(PROJECT.id, true);
    expect(screen.queryByTestId('revalidate-panel')).toBeNull();
  });

  it('a non-terminal warning (stamp/baseline failure) toasts WITHOUT terminal cleanup', async () => {
    vi.mocked(projectsApi.partialAuditStream).mockImplementation(async (_id, _req, handlers) => {
      handlers.onWarning?.('Step 3 (docs/x.md): audit-date stamp failed: disk full');
      handlers.onDone({
        status: 'complete', discussionId: 'd-scoped', auditRunId: 'run-1',
        succeededSteps: [3], unchangedSteps: [], failedSteps: [],
      });
    });
    const { props } = renderCard();
    await clickUpdate();
    expect(props.toast).toHaveBeenCalledWith(expect.stringContaining('audit.streamWarning'), 'error');
    // The warning did not eat the terminal: the complete flow still runs.
    expect(props.toast).toHaveBeenCalledWith(expect.stringContaining('audit.partialValidationCreated'), 'success');
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('d-scoped');
  });

  it('a user cancel never falls into the interrupted branch', async () => {
    // The stream hangs until the signal aborts, then reports the clean
    // no-status done exactly like fetchAndParseSSE does on AbortError.
    let capturedHandlers: { onDone: (info?: PartialDoneInfo) => void } | null = null;
    vi.mocked(projectsApi.partialAuditStream).mockImplementation(async (_id, _req, handlers, signal) => {
      capturedHandlers = handlers;
      await new Promise<void>((resolve) => {
        signal?.addEventListener('abort', () => resolve());
      });
      handlers.onDone();
    });
    vi.mocked(projectsApi.cancelAudit).mockResolvedValue('Audited' as never);
    const { props } = renderCard();
    await clickUpdate();
    const cancelBtn = await screen.findByText(/audit.cancelAudit/);
    await act(async () => { fireEvent.click(cancelBtn); await Promise.resolve(); await Promise.resolve(); });
    expect(capturedHandlers).not.toBeNull();
    const interrupted = (props.toast as ReturnType<typeof vi.fn>).mock.calls
      .filter(([msg]) => String(msg).includes('partialInterrupted'));
    expect(interrupted).toHaveLength(0);
    expect(props.toast).toHaveBeenCalledWith(expect.stringContaining('audit.cancelled'), 'success');
  });
});

// ── Full flow: a NON-terminal step_error must stay visible (Codex msg 166).
// The Full done has no interrupted toast, so without this toast a failed
// spawn would end the run with zero user-visible signal.
describe('ProjectCard — full audit step_error visibility', () => {
  it('step_error toasts an error without cleanup; done then cleans; never a green toast', async () => {
    // Causal: the handlers are captured and fired one at a time, with
    // assertions between the two — proving the cleanup belongs to done
    // ALONE, not to a step_error that happened to precede it.
    type FullHandlers = {
      onStepError?: (error: string, step?: number) => void;
      onDone: (discussionId: string | null, templateWasInstalled: boolean) => void;
    };
    let captured: FullHandlers | null = null;
    let release: (() => void) | undefined;
    vi.mocked(projectsApi.fullAuditStream).mockImplementation(async (_id, _req, handlers) => {
      captured = handlers as FullHandlers;
      await new Promise<void>((resolve) => { release = resolve; });
    });
    const { props, container } = renderCard({
      project: { ...PROJECT, audit_status: 'TemplateInstalled' },
      driftStatus: null,
      agents: [AGENT, CODEX_AGENT],
    });
    const trackerPrerequisite = screen.getByTestId('audit-tracker-prerequisite');
    // The briefing is saved (KT-1192 gate): the prerequisite precedes the launch.
    const launchButton = screen.getByTestId('audit-timeline-launch');
    expect(trackerPrerequisite.compareDocumentPosition(launchButton) & Node.DOCUMENT_POSITION_FOLLOWING)
      .toBeTruthy();
    expect(container.querySelector('select')).toBeNull();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /^Codex/ })); });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /disc\.tier\.economy/ })); });
    const btn = await screen.findByText(/audit.startFullAudit/);
    await act(async () => { fireEvent.click(btn); await Promise.resolve(); });
    expect(captured).not.toBeNull();
    expect(projectsApi.fullAuditStream).toHaveBeenCalledWith(
      PROJECT.id,
      expect.objectContaining({ agent: 'Codex', tier: 'economy', kind: 'Full', resume_run_id: null }),
      expect.any(Object),
      expect.any(AbortSignal),
    );

    await act(async () => { captured!.onStepError?.('Step 2 (docs/x.md): spawn failed', 2); });
    expect(props.toast).toHaveBeenCalledWith(expect.stringContaining('audit.streamWarning'), 'error');
    expect(props.onRefetch).not.toHaveBeenCalled();
    expect(props.onRefetchDiscussions).not.toHaveBeenCalled();

    await act(async () => { captured!.onDone(null, false); release?.(); await Promise.resolve(); });
    expect(props.onRefetch).toHaveBeenCalled();
    const greens = (props.toast as ReturnType<typeof vi.fn>).mock.calls
      .filter(([, kind]) => kind === 'success');
    expect(greens).toHaveLength(0);
  });

  // KT-1203 — docs still holding placeholders survive a reload through the drift API.
  // The list card and the detail page both reach the same verification dialog.
  it.each([false, true])('verification dialog (detailMode=%s) fills placeholder docs with their scoped steps', async (detailMode) => {
    vi.mocked(projectsApi.partialAuditStream).mockResolvedValue(undefined);
    renderCard({
      detailMode,
      driftStatus: {
        ...DRIFT,
        stale_sections: [],
        placeholder_docs: [
          { path: 'docs/glossary.md', step: 2 },
          { path: 'docs/inconsistencies-tech-debt.md', step: 8 },
          { path: 'docs/workflow/tickets.md', step: null },
        ],
      },
    });
    await openVerification();
    const fill = await screen.findByTestId('revalidate-placeholder-fill');
    await act(async () => { fireEvent.click(fill); await Promise.resolve(); });
    expect(projectsApi.partialAuditStream).toHaveBeenCalledTimes(1);
    expect(vi.mocked(projectsApi.partialAuditStream).mock.calls[0][1]).toMatchObject({ agent: 'ClaudeCode', steps: [2, 8] });
  });
});

// ── Addresses: the validation badge and the tracker hint open their page in
// a new tab on a modified click, and keep their in-app navigation otherwise.
describe('ProjectCard — new-tab addresses', () => {
  it('the validation-in-progress badge opens its discussion in a new tab on Ctrl-click', async () => {
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    const validation = {
      id: 'd-running-val', project_id: PROJECT.id, title: 'Validation audit AI', archived: false,
    } as unknown as Discussion;
    // The badge follows the run's recorded validation link (KT-1206), never a
    // guess from the discussion title.
    vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
      runs: [{
        id: 'run-val', project_id: PROJECT.id, kind: 'Full', agent_type: 'ClaudeCode',
        started_at: '2026-10-12T10:00:00Z', status: 'Completed', td_total: 0,
        validation_discussion_id: 'd-running-val',
      }],
      steps: [], recorded_audits: [], recorded_validated_at: null,
      latest_validation: { discussion_id: 'd-running-val', finished: false, archived: false },
      lease_orphaned: false,
    } as never);
    const { props } = renderCard({ discussions: [validation], driftStatus: null });
    const badge = await screen.findByTestId('project-validation-badge');

    fireEvent.click(badge, { ctrlKey: true });
    expect(open).toHaveBeenCalledWith(`${window.location.origin}/discussions/d-running-val`, '_blank', 'noopener,noreferrer');
    expect(props.onOpenDiscussion).not.toHaveBeenCalled();

    fireEvent.click(badge);
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('d-running-val');
    expect(open).toHaveBeenCalledTimes(1);
    open.mockRestore();
  });

  it('the tracker hint configure button opens /plugins in a new tab on Ctrl-click', () => {
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    const { props } = renderCard({ project: { ...PROJECT, audit_status: 'TemplateInstalled' }, driftStatus: null });
    const hint = screen.getByTestId('audit-tracker-prerequisite');
    const configure = hint.querySelector('.dash-tracker-hint-actions .dash-icon-btn:not(.dash-tracker-hint-dismiss)') as HTMLElement;

    fireEvent.click(configure, { ctrlKey: true });
    expect(open).toHaveBeenCalledWith(`${window.location.origin}/plugins`, '_blank', 'noopener,noreferrer');
    expect(props.onNavigate).not.toHaveBeenCalled();

    fireEvent.click(configure);
    expect(props.onNavigate).toHaveBeenCalledWith('mcps');
    open.mockRestore();
  });
});
