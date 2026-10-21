// KT-1131 / KT-1134 — the project card drives the timeline's actions: a failed
// step retried alone or accepted, a confirmed fresh audit, a cancel the backend
// has not acknowledged, and no "validation in progress" while an audit runs.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, screen, waitFor, within } from '@testing-library/react';
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
import type { Project, AgentDetection, Discussion } from '../../types/generated';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-states', name: 'states-target', path: '/repos/states-target',
  repo_url: null, token_override: null, ai_config: { detected: false, configs: [] },
  audit_status: 'Audited', briefing_notes: '# Project Briefing', ai_todo_count: 0, tech_debt_count: 0,
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

function renderCard(discussions: Discussion[] = []) {
  const props = {
    project: PROJECT, isOpen: true, onToggleOpen: noop, discussions,
    driftStatus: null, agents: [AGENT], allSkills: [], mcpConfigs: [],
    workflows: [], configLanguage: 'fr',
    toast: vi.fn(), onNavigate: vi.fn(), onSetDiscPrefill: noop,
    onAutoRunDiscussion: vi.fn(), onOpenDiscussion: vi.fn(),
    onRefetch: vi.fn(), onRefetchDiscussions: vi.fn(), onRefetchSkills: noop,
    onRefetchDrift: vi.fn(),
  };
  return { ...render(<ProjectCard {...(props as any)} />), props };
}

async function openGroups(container: HTMLElement) {
  await waitFor(() => expect(container.querySelector('.audit-tl-group-head')).not.toBeNull());
  container.querySelectorAll<HTMLElement>('.audit-tl-group-head[aria-expanded="false"]').forEach(h => fireEvent.click(h));
}

beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
  vi.mocked(projectsApi.auditResumable).mockResolvedValue(RESUMABLE);
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: [{ id: 'run-x', kind: 'Full', project_id: 'p-states', agent_type: 'ClaudeCode', started_at: '', status: 'Interrupted', td_total: 0 }],
    steps: [step(1), step(2), step(3, { cli_success: false, step_warning: 'placeholders remain' })],
    recorded_audits: [], recorded_validated_at: null, latest_validation: null, lease_orphaned: false,
  } as never);
  vi.mocked(projectsApi.fullAuditStream).mockImplementation(async (_id, _req, handlers) => {
    handlers.onDone(null, false, 'interrupted');
  });
});
afterEach(() => { vi.restoreAllMocks(); });

describe('ProjectCard — audit run states and their actions', () => {
  it('retrying one failed step resumes the run with that step only', async () => {
    const { container } = renderCard();
    await openGroups(container);
    const retry = await screen.findByTestId('audit-timeline-step-retry-3');
    await act(async () => { fireEvent.click(retry); await Promise.resolve(); });
    expect(projectsApi.fullAuditStream).toHaveBeenCalledTimes(1);
    const req = vi.mocked(projectsApi.fullAuditStream).mock.calls[0][1];
    expect(req.resume_run_id).toBe('run-x');
    expect(req.retry_steps).toEqual([3]);
  });

  it('accepting a failed step records it on the resumable run and refreshes the resume', async () => {
    const { container, props } = renderCard();
    await openGroups(container);
    fireEvent.click(await screen.findByTestId('audit-timeline-step-accept-3'));
    const form = screen.getByTestId('audit-timeline-accept-form-3');
    fireEvent.change(within(form).getByRole('textbox'), { target: { value: 'kept by hand' } });
    const resumableCalls = vi.mocked(projectsApi.auditResumable).mock.calls.length;
    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-accept-confirm-3')); await Promise.resolve(); });
    expect(projectsApi.acceptAuditStep).toHaveBeenCalledWith('p-states', 'run-x', 3, 'kept by hand');
    expect(props.toast).toHaveBeenCalledWith('audit.stepAccepted 3', 'success');
    await waitFor(() => expect(vi.mocked(projectsApi.auditResumable).mock.calls.length).toBeGreaterThan(resumableCalls));
    expect(projectsApi.fullAuditStream).not.toHaveBeenCalled();
  });

  it('a refused acceptance says why and changes nothing', async () => {
    vi.mocked(projectsApi.acceptAuditStep).mockRejectedValueOnce(new Error('`docs/step-3.md` still holds template placeholders'));
    const { container, props } = renderCard();
    await openGroups(container);
    fireEvent.click(await screen.findByTestId('audit-timeline-step-accept-3'));
    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-accept-confirm-3')); await Promise.resolve(); });
    await waitFor(() => expect(props.toast).toHaveBeenCalledWith(expect.stringContaining('template placeholders'), 'error'));
  });

  it('a confirmed fresh audit ignores the resumable run; resuming keeps it', async () => {
    renderCard();
    fireEvent.click(await screen.findByTestId('audit-timeline-relaunch-full'));
    await act(async () => { fireEvent.click(screen.getByTestId('audit-timeline-relaunch-confirm-full')); await Promise.resolve(); });
    const fresh = vi.mocked(projectsApi.fullAuditStream).mock.calls[0][1];
    expect(fresh.resume_run_id).toBeNull();
    expect(fresh.kind).toBe('Full');
    expect(fresh.retry_steps).toBeUndefined();
  });

  it('a cancel the backend does not acknowledge leaves the audit shown, pending, never idle', async () => {
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(null);
    vi.mocked(projectsApi.auditStatus).mockResolvedValue({
      project_id: 'p-states', phase: 'auditing', step_index: 3, total_steps: 16,
      current_file: 'docs/AGENTS.md', started_at: '2026-10-02T10:00:00Z', kind: 'full_audit',
    });
    vi.mocked(projectsApi.cancelAudit).mockRejectedValue(new Error('Cancel requested but not acknowledged'));
    const { props } = renderCard();
    const cancel = await screen.findByTestId('audit-timeline-cancel');
    await act(async () => { fireEvent.click(cancel); await Promise.resolve(); await Promise.resolve(); });
    expect(await screen.findByTestId('audit-timeline-cancel-pending')).toBeInTheDocument();
    expect(screen.getByTestId('audit-timeline-live')).toBeInTheDocument();
    expect(props.toast).toHaveBeenCalledWith('audit.cancelPending', 'warning');
    expect(props.toast).not.toHaveBeenCalledWith('audit.cancelled', 'success');

    // Once the server reports nothing running, the pending state clears.
    vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-live')).toBeNull(), { timeout: 4000 });
    expect(screen.queryByTestId('audit-timeline-cancel-pending')).toBeNull();
  });

  it('a running audit never shows the previous run\'s validation as in progress', async () => {
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(null);
    vi.mocked(projectsApi.auditStatus).mockResolvedValue({
      project_id: 'p-states', phase: 'auditing', step_index: 3, total_steps: 16,
      current_file: 'docs/AGENTS.md', started_at: '2026-10-02T10:00:00Z', kind: 'full_audit',
    });
    const partialValidation = {
      id: 'd-partial', project_id: 'p-states', title: 'Validation audit AI', archived: false,
      message_count: 2, agent: 'ClaudeCode', participants: [], messages: [],
    } as unknown as Discussion;
    renderCard([partialValidation]);
    await screen.findByTestId('audit-timeline-live');
    expect(screen.queryByText('audit.resumeValidation')).toBeNull();
    expect(screen.queryByText('projects.status.validating')).toBeNull();
  });
});
