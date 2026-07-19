// Partial refresh → scoped validation UX (Codex A5 lot 2).
//
// A fully-successful partial creates a validation discussion scoped to the
// refreshed sections. The card must surface it exactly like the Full flow:
// refetch discussions, toast, open + NAVIGATE — without auto-running the
// agent (the backend already spawned it post-commit). An interrupted
// refresh must never toast green, and a user cancel must not fall into the
// interrupted branch.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, screen } from '@testing-library/react';
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
import type { Project, AgentDetection, DriftCheckResponse } from '../../types/generated';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-partial',
  name: 'partial-target',
  path: '/repos/partial-target',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'Audited',
  ai_todo_count: 0, tech_debt_count: 3, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true,
  path: '/usr/bin/claude', version: '1.0.0', latest_version: null, origin: 'host',
  install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

const DRIFT: DriftCheckResponse = {
  audit_date: '2026-07-20',
  stale_sections: [
    { ai_file: 'docs/repo-map.md', audit_step: 3, changed_sources: ['src/x.rs'] },
  ],
  fresh_sections: [],
  total_sections: 1,
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
  const btn = await screen.findByText(/audit.updateStale/);
  await act(async () => { fireEvent.click(btn); await Promise.resolve(); });
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
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('d-scoped');
    expect(props.onNavigate).toHaveBeenCalledWith('discussions');
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

  it('no_change: honest stale toast, no discussion, no relaunch nudge as success', async () => {
    vi.mocked(projectsApi.partialAuditStream).mockImplementation(async (_id, _req, handlers) => {
      handlers.onDone({
        status: 'no_change', discussionId: null, auditRunId: 'run-2',
        succeededSteps: [], unchangedSteps: [3], failedSteps: [],
      });
    });
    const { props } = renderCard();
    await clickUpdate();
    expect(props.toast).toHaveBeenCalledWith(expect.stringContaining('audit.partialNoChange'), 'error');
    expect(props.onOpenDiscussion).not.toHaveBeenCalled();
    const greens = (props.toast as ReturnType<typeof vi.fn>).mock.calls
      .filter(([, kind]) => kind === 'success');
    expect(greens).toHaveLength(0);
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
    const { props } = renderCard({
      project: { ...PROJECT, audit_status: 'TemplateInstalled' },
      driftStatus: null,
    });
    const btn = await screen.findByText(/audit.kindSelector.launchLabel/);
    await act(async () => { fireEvent.click(btn); await Promise.resolve(); });
    expect(captured).not.toBeNull();

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
});
