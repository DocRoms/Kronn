// KT-1169 — on the project card, an audit launch that stops on an error leaves
// a "Reprendre" that replays exactly that launch, and no launch ignores the
// state it must start from: a run to resume, or a state that could not be read.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, screen, waitFor } from '@testing-library/react';
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
  id: 'p-r', name: 'reprendre-target', path: '/repos/reprendre-target',
  repo_url: null, token_override: null, ai_config: { detected: false, configs: [] },
  audit_status: 'Audited', ai_todo_count: 0, tech_debt_count: 0,
  needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
};

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true,
  path: '/usr/bin/claude', version: '1.0.0', latest_version: null, origin: 'host',
  install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

const step = (index: number, extra: Record<string, unknown> = {}) => ({
  audit_run_id: 'run-x', step_index: index, file_label: `docs/step-${index}.md`,
  started_at: '2026-10-02T10:00:00Z', ended_at: '2026-10-02T10:01:00Z', duration_ms: 1000,
  step_tokens: 10, cumulative_tokens: 10, cli_success: true, step_warning: null,
  step_repaired_from_template: false, unchanged: false, ...extra,
});

const RESUMABLE = {
  id: 'run-x', kind: 'Full' as const, last_completed_step: 2, steps_to_redo: [3],
  started_at: '2026-10-02T10:00:00Z',
};

const DRIFT = {
  stale_sections: [{ ai_file: 'docs/step-2.md', audit_step: 2, changed_sources: ['src/a.rs'] }],
  fresh_sections: [], total_sections: 1, citation_drift: null,
};

function renderCard(over: Record<string, unknown> = {}) {
  const props = {
    project: PROJECT, isOpen: true, onToggleOpen: noop, discussions: [],
    driftStatus: null, agents: [AGENT], allSkills: [], mcpConfigs: [],
    workflows: [], configLanguage: 'fr',
    toast: vi.fn(), onNavigate: vi.fn(), onSetDiscPrefill: noop,
    onAutoRunDiscussion: vi.fn(), onOpenDiscussion: vi.fn(),
    onRefetch: vi.fn(), onRefetchDiscussions: vi.fn(), onRefetchSkills: noop,
    onRefetchDrift: vi.fn(), ...over,
  };
  return { ...render(<ProjectCard {...(props as any)} />), props };
}

async function openGroups(container: HTMLElement) {
  await waitFor(() => expect(container.querySelector('.audit-tl-group-head')).not.toBeNull());
  container.querySelectorAll<HTMLElement>('.audit-tl-group-head[aria-expanded="false"]').forEach(h => fireEvent.click(h));
}

const launches = () => vi.mocked(projectsApi.fullAuditStream).mock.calls.map(call => call[1]);

beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
  vi.mocked(projectsApi.auditResumable).mockResolvedValue(RESUMABLE);
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: [{ id: 'run-x', kind: 'Full', project_id: 'p-r', agent_type: 'ClaudeCode', started_at: '', status: 'Interrupted', td_total: 0 }],
    steps: [step(1), step(2), step(3, { cli_success: false, step_warning: 'placeholders remain' })],
    recorded_audits: [], recorded_validated_at: null, latest_validation: null, lease_orphaned: false,
  } as never);
  vi.mocked(projectsApi.fullAuditStream).mockImplementation(async (_id, _req, handlers) => {
    handlers.onDone(null, false, 'interrupted');
  });
});
afterEach(() => { vi.restoreAllMocks(); });

describe('ProjectCard — KT-1169 "Reprendre" after every audit error', () => {
  it('a stream error leaves an inline "Reprendre" that replays the same resume', async () => {
    vi.mocked(projectsApi.fullAuditStream).mockImplementationOnce(async (_id, _req, handlers) => {
      handlers.onError('agent crashed');
    });
    const { container } = renderCard();
    await openGroups(container);
    await act(async () => { fireEvent.click(await screen.findByTestId('audit-timeline-step-retry-3')); });
    const alert = await screen.findByTestId('audit-timeline-launch-error');
    expect(alert).toHaveTextContent('agent crashed');

    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-launch-error-retry')); });
    await waitFor(() => expect(launches()).toHaveLength(2));
    expect(launches()[1]).toMatchObject({ resume_run_id: 'run-x', retry_steps: [3] });
    expect(launches()[1].fresh).toBeUndefined();
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-launch-error')).toBeNull());
  });

  it('a launch that throws before any event still leaves "Reprendre"', async () => {
    vi.mocked(projectsApi.fullAuditStream).mockRejectedValueOnce(new Error('network down'));
    renderCard();
    const launch = await screen.findByTestId('audit-timeline-launch');
    await waitFor(() => expect(launch).toBeEnabled());
    await act(async () => { fireEvent.click(launch); });
    expect(await screen.findByTestId('audit-timeline-launch-error')).toHaveTextContent('network down');
    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-launch-error-retry')); });
    await waitFor(() => expect(launches()).toHaveLength(2));
    expect(launches()[1].resume_run_id).toBe('run-x');
  });

  it('a confirmed fresh audit says so to the server; a resume never does', async () => {
    renderCard();
    fireEvent.click(await screen.findByTestId('audit-timeline-relaunch-full'));
    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-relaunch-confirm-full')); });
    await waitFor(() => expect(launches()).toHaveLength(1));
    expect(launches()[0]).toMatchObject({ resume_run_id: null, kind: 'Full', fresh: true });

    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-launch')); });
    await waitFor(() => expect(launches()).toHaveLength(2));
    expect(launches()[1]).toMatchObject({ resume_run_id: 'run-x' });
    expect(launches()[1].fresh).toBeUndefined();
  });

  it('when the run to resume cannot be read, nothing launches until it is', async () => {
    vi.mocked(projectsApi.auditResumable).mockRejectedValueOnce(new Error('offline'));
    renderCard();
    const unknown = await screen.findByTestId('audit-timeline-resumable-unknown');
    expect(unknown).toBeInTheDocument();
    expect(screen.getByTestId('audit-timeline-launch')).toBeDisabled();
    expect(screen.queryByTestId('audit-timeline-reverify-hint')).toBeNull();

    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-resumable-reload')); });
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-resumable-unknown')).toBeNull());
    const launch = screen.getByTestId('audit-timeline-launch');
    expect(launch).toBeEnabled();
    expect(launch).toHaveTextContent('audit.resumeRedoSteps');
    await act(async () => { fireEvent.click(launch); });
    await waitFor(() => expect(launches()).toHaveLength(1));
    expect(launches()[0].resume_run_id).toBe('run-x');
  });

  it('a step retried after its run stopped being resumable never launches a whole audit', async () => {
    const { container } = renderCard();
    await openGroups(container);
    const retry = await screen.findByTestId('audit-timeline-step-retry-3');
    // The first launch fails and the run is superseded meanwhile.
    vi.mocked(projectsApi.fullAuditStream).mockImplementationOnce(async (_id, _req, handlers) => {
      handlers.onError('resume_run_id run-x is not the project\'s most recent run');
    });
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(null);
    await act(async () => { fireEvent.click(retry); });
    await screen.findByTestId('audit-timeline-launch-error');
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-step-retry-3')).toBeNull());

    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-launch-error-retry')); });
    expect(launches()).toHaveLength(1);
    expect(await screen.findByTestId('audit-timeline-launch-error')).toHaveTextContent('audit.retryNeedsRun');
    // Its "Reprendre" reads the state again, and still launches nothing.
    const reads = vi.mocked(projectsApi.auditResumable).mock.calls.length;
    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-launch-error-retry')); });
    await waitFor(() => expect(vi.mocked(projectsApi.auditResumable).mock.calls.length).toBeGreaterThan(reads));
    expect(launches()).toHaveLength(1);
  });

  it('a refused acceptance keeps the form open with its error and reason', async () => {
    vi.mocked(projectsApi.acceptAuditStep).mockRejectedValueOnce(new Error('`docs/step-3.md` still holds template placeholders'));
    const { container } = renderCard();
    await openGroups(container);
    fireEvent.click(await screen.findByTestId('audit-timeline-step-accept-3'));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'kept by hand' } });
    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-accept-confirm-3')); });
    expect(await screen.findByTestId('audit-timeline-accept-error-3')).toHaveTextContent('template placeholders');
    expect(screen.getByRole('textbox')).toHaveValue('kept by hand');
    expect(screen.getByTestId('audit-timeline-accept-confirm-3')).toBeEnabled();
  });

  it('no stale-sections refresh is offered while a run waits to be resumed', async () => {
    const { rerender, props } = renderCard({ driftStatus: DRIFT });
    await screen.findByTestId('audit-timeline-launch');
    await waitFor(() => expect(projectsApi.auditResumable).toHaveBeenCalled());
    await waitFor(() => expect(screen.queryAllByTitle('audit.updateStale 1')).toHaveLength(0));

    vi.mocked(projectsApi.auditResumable).mockResolvedValue(null);
    rerender(<ProjectCard {...(props as any)} project={{ ...PROJECT, audit_status: 'Validated' }} />);
    await waitFor(() => expect(screen.queryAllByTitle('audit.updateStale 1').length).toBeGreaterThan(0));
  });
});
