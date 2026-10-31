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

// The revalidation entry only matters here for the choice it is handed.
vi.mock('../RevalidateAudit', () => ({
  RevalidateAudit: (p: { agent: string; tier: string; connectionId: string | null }) => (
    <div data-testid="revalidate" data-agent={p.agent} data-tier={p.tier} data-connection={p.connectionId ?? ''} />
  ),
}));

import { ProjectCard } from '../ProjectCard';
import { projects as projectsApi, externalApi } from '../../lib/api';
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

// A named OpenAI-compatible connection: it runs as `Custom`, which no detection reports.
const NAMED = {
  id: 'named-1', display_name: 'Named gateway', origin_preset: 'other', endpoint: 'https://gw.example/v1',
  economy_model: 'cheap', default_model: 'mid', reasoning_model: 'big', mention_alias: 'gw',
};
const RUN_ON_NAMED = { ...RESUMABLE, agent_type: 'Custom', connection_id: 'named-1' };

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
  // The connection list is whatever the previous spec left it: back to none.
  vi.mocked(externalApi.list).mockReset().mockResolvedValue([]);
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

  it("an untouched picker leaves the run's connection to the server (it may have been deleted)", async () => {
    vi.mocked(projectsApi.auditResumable).mockResolvedValue({ ...RESUMABLE, connection_id: 'conn-gone' });
    const { container } = renderCard();
    await openGroups(container);
    await screen.findByTestId('audit-timeline-selection-from-run');
    await act(async () => { fireEvent.click(await screen.findByTestId('audit-timeline-step-retry-3')); });
    await waitFor(() => expect(launches()).toHaveLength(1));
    expect(launches()[0]).toMatchObject({ resume_run_id: 'run-x', tier: 'economy' });
    expect(launches()[0].connection_id).toBeUndefined();
  });

  it("a run on a named Custom connection is shown and retried on it, tier included", async () => {
    vi.mocked(externalApi.list).mockResolvedValue([NAMED] as never);
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(RUN_ON_NAMED);
    const { container } = renderCard();
    await openGroups(container);
    expect(await screen.findByTestId('audit-timeline-selection-from-run')).toHaveTextContent('disc.tier.economy');
    // The panel lights the named connection and the run's tier, not a detected agent.
    await waitFor(() => {
      const lit = Array.from(container.querySelectorAll<HTMLElement>('.audit-tl-choice[aria-pressed="true"]'));
      expect(lit.map(b => b.textContent)).toEqual([
        expect.stringContaining('Named gateway'), expect.stringContaining('disc.tier.economy'),
      ]);
    });
    await act(async () => { fireEvent.click(await screen.findByTestId('audit-timeline-step-retry-3')); });
    await waitFor(() => expect(launches()).toHaveLength(1));
    // The connection itself is the server's to inherit on a resume.
    expect(launches()[0]).toMatchObject({ resume_run_id: 'run-x', agent: 'Custom', tier: 'economy' });
    expect(launches()[0].connection_id).toBeUndefined();
  });

  it("a touched picker sends the connection it shows", async () => {
    vi.mocked(externalApi.list).mockResolvedValue([NAMED] as never);
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(RUN_ON_NAMED);
    const { container } = renderCard();
    await openGroups(container);
    await screen.findByTestId('audit-timeline-selection-from-run');
    fireEvent.click(container.querySelectorAll<HTMLElement>('.audit-tl-tier')[0]);
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-selection-from-run')).toBeNull());
    await act(async () => { fireEvent.click(await screen.findByTestId('audit-timeline-step-retry-3')); });
    await waitFor(() => expect(launches()).toHaveLength(1));
    expect(launches()[0]).toMatchObject({ agent: 'Custom', tier: 'reasoning', connection_id: 'named-1' });
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

  it("a partial audit runs on the choice the panel shows, not on one it hides", async () => {
    vi.mocked(projectsApi.partialAuditStream).mockResolvedValue(undefined as never);
    renderCard();
    expect(await screen.findByTestId('audit-timeline-selection-from-run')).toHaveTextContent('disc.tier.economy');
    await act(async () => { fireEvent.click(await screen.findByTestId('run-partial')); });
    await waitFor(() => expect(vi.mocked(projectsApi.partialAuditStream)).toHaveBeenCalledTimes(1));
    expect(vi.mocked(projectsApi.partialAuditStream).mock.calls[0][1]).toMatchObject({ agent: 'ClaudeCode', tier: 'economy', steps: [3] });
  });

  it("a partial audit on a named connection sends that connection", async () => {
    vi.mocked(externalApi.list).mockResolvedValue([NAMED] as never);
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(RUN_ON_NAMED);
    vi.mocked(projectsApi.partialAuditStream).mockResolvedValue(undefined as never);
    renderCard();
    await waitFor(() => expect(screen.getByTestId('revalidate')).toHaveAttribute('data-agent', 'Custom'));
    await act(async () => { fireEvent.click(await screen.findByTestId('run-partial')); });
    await waitFor(() => expect(vi.mocked(projectsApi.partialAuditStream)).toHaveBeenCalledTimes(1));
    expect(vi.mocked(projectsApi.partialAuditStream).mock.calls[0][1])
      .toMatchObject({ agent: 'Custom', tier: 'economy', connection_id: 'named-1' });
  });

  it("the revalidation is handed the choice the panel shows", async () => {
    renderCard();
    await screen.findByTestId('audit-timeline-selection-from-run');
    await waitFor(() => expect(screen.getByTestId('revalidate')).toHaveAttribute('data-tier', 'economy'));
    expect(screen.getByTestId('revalidate')).toHaveAttribute('data-agent', 'ClaudeCode');
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

// The class: the run's agent and tier are never swapped for the picker's when its
// connection cannot be resolved at click time — loading, failed, or really gone.
describe('ProjectCard — KT-1320 the run keeps its agent and tier whatever its connection', () => {
  const retry = async (container: HTMLElement) => {
    await openGroups(container);
    await act(async () => { fireEvent.click(await screen.findByTestId('audit-timeline-step-retry-3')); });
    await waitFor(() => expect(launches()).toHaveLength(1));
  };

  it('B — a LiteLlm run whose connection is deleted, another LiteLlm connection existing', async () => {
    const other = { ...NAMED, id: 'lite-2', display_name: 'Other lite', origin_preset: 'lite_llm' };
    vi.mocked(externalApi.list).mockResolvedValue([other] as never);
    vi.mocked(projectsApi.auditResumable).mockResolvedValue({ ...RESUMABLE, agent_type: 'LiteLlm', connection_id: 'lite-gone' });
    const { container } = renderCard();
    // The note is there before anything is launched.
    expect(await screen.findByTestId('audit-timeline-connection-gone')).toHaveTextContent('LiteLLM');
    await retry(container);
    expect(launches()[0]).toMatchObject({ resume_run_id: 'run-x', agent: 'LiteLlm', tier: 'economy' });
    expect(launches()[0].connection_id).toBeUndefined();
  });

  it('C — a Custom run whose connection is deleted, no connection left', async () => {
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(RUN_ON_NAMED);
    const { container } = renderCard();
    expect(await screen.findByTestId('audit-timeline-connection-gone')).toBeInTheDocument();
    await retry(container);
    expect(launches()[0]).toMatchObject({ resume_run_id: 'run-x', agent: 'Custom', tier: 'economy' });
    expect(launches()[0].connection_id).toBeUndefined();
  });

  it('D — a Custom run on a valid connection, the connection list failing to load', async () => {
    vi.mocked(externalApi.list).mockRejectedValue(new Error('boom'));
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(RUN_ON_NAMED);
    const { container } = renderCard();
    expect(await screen.findByTestId('audit-timeline-selection-from-run')).toHaveTextContent('disc.tier.economy');
    // Unknown is not deleted: no note announces a fallback that may not happen.
    expect(screen.queryByTestId('audit-timeline-connection-gone')).toBeNull();
    await retry(container);
    expect(launches()[0]).toMatchObject({ resume_run_id: 'run-x', agent: 'Custom', tier: 'economy' });
    expect(launches()[0].connection_id).toBeUndefined();
  });

  it('E — a retry clicked while the connection list is still loading', async () => {
    vi.mocked(externalApi.list).mockReturnValue(new Promise(() => {}) as never);
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(RUN_ON_NAMED);
    const { container } = renderCard();
    expect(await screen.findByTestId('audit-timeline-selection-from-run')).toHaveTextContent('disc.tier.economy');
    expect(screen.queryByTestId('audit-timeline-connection-gone')).toBeNull();
    await retry(container);
    expect(launches()[0]).toMatchObject({ resume_run_id: 'run-x', agent: 'Custom', tier: 'economy' });
    expect(launches()[0].connection_id).toBeUndefined();
  });

  it('a partial audit and the revalidation keep the same agent and tier when the list failed', async () => {
    vi.mocked(externalApi.list).mockRejectedValue(new Error('boom'));
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(RUN_ON_NAMED);
    vi.mocked(projectsApi.partialAuditStream).mockResolvedValue(undefined as never);
    renderCard();
    await waitFor(() => expect(screen.getByTestId('revalidate')).toHaveAttribute('data-agent', 'Custom'));
    expect(screen.getByTestId('revalidate')).toHaveAttribute('data-tier', 'economy');
    await act(async () => { fireEvent.click(await screen.findByTestId('run-partial')); });
    await waitFor(() => expect(vi.mocked(projectsApi.partialAuditStream)).toHaveBeenCalledTimes(1));
    expect(vi.mocked(projectsApi.partialAuditStream).mock.calls[0][1])
      .toMatchObject({ agent: 'Custom', tier: 'economy' });
  });
});
