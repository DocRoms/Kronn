// KT-1320 — retrying a step (or resuming) from the project card runs on the
// agent, tier and connection of the run it continues, not on the picker's
// default (Reasoning), unless the human picks something else in the panel.
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

// A partial audit is launched from this notice: a button stands for its run action.
vi.mock('../TemplateUpdateNotice', () => ({
  TemplateUpdateNotice: ({ onRunSteps }: { onRunSteps?: (steps: number[]) => void }) => (
    <button data-testid="run-partial" onClick={() => onRunSteps?.([3])}>partial</button>
  ),
}));

import { ProjectCard } from '../ProjectCard';
import { projects as projectsApi } from '../../lib/api';
import type { Project, AgentDetection } from '../../types/generated';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-t', name: 'tier-target', path: '/repos/tier-target',
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
  started_at: '2026-10-02T10:00:00Z', agent_type: 'ClaudeCode', tier: 'economy',
};

function renderCard() {
  const props = {
    project: PROJECT, isOpen: true, onToggleOpen: noop, discussions: [],
    driftStatus: null, agents: [AGENT], allSkills: [], mcpConfigs: [],
    workflows: [], configLanguage: 'fr',
    toast: vi.fn(), onNavigate: vi.fn(), onSetDiscPrefill: noop,
    onAutoRunDiscussion: vi.fn(), onOpenDiscussion: vi.fn(),
    onRefetch: vi.fn(), onRefetchDiscussions: vi.fn(), onRefetchSkills: noop,
    onRefetchDrift: vi.fn(),
  };
  return render(<ProjectCard {...(props as any)} />);
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
    runs: [{ id: 'run-x', kind: 'Full', project_id: 'p-t', agent_type: 'ClaudeCode', started_at: '', status: 'Interrupted', td_total: 0 }],
    steps: [step(1), step(2), step(3, { cli_success: false, step_warning: 'placeholders remain' })],
    recorded_audits: [], recorded_validated_at: null, latest_validation: null, lease_orphaned: false,
  } as never);
  vi.mocked(projectsApi.fullAuditStream).mockImplementation(async (_id, _req, handlers) => {
    handlers.onDone(null, false, 'interrupted');
  });
});
afterEach(() => { vi.restoreAllMocks(); });

describe('ProjectCard — KT-1320 a retry runs on the tier of the run it continues', () => {
  it("a step retry sends the run's tier, not the picker's default, and the panel shows it", async () => {
    const { container } = renderCard();
    await openGroups(container);
    expect(await screen.findByTestId('audit-timeline-selection-from-run')).toHaveTextContent('disc.tier.economy');
    await act(async () => { fireEvent.click(await screen.findByTestId('audit-timeline-step-retry-3')); });
    await waitFor(() => expect(launches()).toHaveLength(1));
    expect(launches()[0]).toMatchObject({ resume_run_id: 'run-x', retry_steps: [3], agent: 'ClaudeCode', tier: 'economy' });
  });

  it("a resume sends the run's tier too", async () => {
    vi.mocked(projectsApi.auditResumable).mockResolvedValue({ ...RESUMABLE, tier: 'default' });
    renderCard();
    const launch = await screen.findByTestId('audit-timeline-launch');
    await waitFor(() => expect(launch).toBeEnabled());
    await act(async () => { fireEvent.click(launch); });
    await waitFor(() => expect(launches()).toHaveLength(1));
    expect(launches()[0]).toMatchObject({ resume_run_id: 'run-x', tier: 'default' });
  });

  it("a tier the human picks in the panel wins over the run's", async () => {
    const { container } = renderCard();
    await openGroups(container);
    await screen.findByTestId('audit-timeline-selection-from-run');
    const tiers = Array.from(container.querySelectorAll<HTMLElement>('.audit-tl-tier'));
    expect(tiers).toHaveLength(3);
    // Panel order: reasoning, default, economy; the run's economy is the lit one.
    expect(tiers[2]).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(tiers[0]);
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-selection-from-run')).toBeNull());
    await act(async () => { fireEvent.click(await screen.findByTestId('audit-timeline-step-retry-3')); });
    await waitFor(() => expect(launches()).toHaveLength(1));
    expect(launches()[0]).toMatchObject({ resume_run_id: 'run-x', tier: 'reasoning' });
  });

  it("a run that kept no tier leaves the picker's choice", async () => {
    vi.mocked(projectsApi.auditResumable).mockResolvedValue({ ...RESUMABLE, tier: undefined });
    renderCard();
    const launch = await screen.findByTestId('audit-timeline-launch');
    await waitFor(() => expect(launch).toBeEnabled());
    expect(screen.queryByTestId('audit-timeline-selection-from-run')).toBeNull();
    await act(async () => { fireEvent.click(launch); });
    await waitFor(() => expect(launches()).toHaveLength(1));
    expect(launches()[0]).toMatchObject({ tier: 'reasoning' });
  });

  it("a partial audit runs on the picker's choice, not on the interrupted run's", async () => {
    vi.mocked(projectsApi.partialAuditStream).mockResolvedValue(undefined as never);
    renderCard();
    await screen.findByTestId('audit-timeline-selection-from-run');
    await act(async () => { fireEvent.click(await screen.findByTestId('run-partial')); });
    await waitFor(() => expect(vi.mocked(projectsApi.partialAuditStream)).toHaveBeenCalledTimes(1));
    expect(vi.mocked(projectsApi.partialAuditStream).mock.calls[0][1]).toMatchObject({ agent: 'ClaudeCode', tier: 'reasoning', steps: [3] });
  });

  it("a pick made for one run does not follow the next run to continue", async () => {
    const { container } = renderCard();
    await openGroups(container);
    await screen.findByTestId('audit-timeline-selection-from-run');
    fireEvent.click(container.querySelectorAll<HTMLElement>('.audit-tl-tier')[0]);
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-selection-from-run')).toBeNull());
    // The retry ends interrupted again, as another run: the picker starts over from its tier.
    vi.mocked(projectsApi.auditResumable).mockResolvedValue({ ...RESUMABLE, id: 'run-y', tier: 'default' });
    await act(async () => { fireEvent.click(await screen.findByTestId('audit-timeline-step-retry-3')); });
    expect(await screen.findByTestId('audit-timeline-selection-from-run')).toHaveTextContent('disc.tier.default');
    await act(async () => { fireEvent.click(await screen.findByTestId('audit-timeline-step-retry-3')); });
    await waitFor(() => expect(launches()).toHaveLength(2));
    expect(launches()[1]).toMatchObject({ resume_run_id: 'run-y', tier: 'default' });
  });
});
