// KT-977 — the audit tab as a timeline.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act, waitFor, within } from '@testing-library/react';
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

const agent = (agent_type: AgentDetection['agent_type'], name: string): AgentDetection => ({
  name, agent_type, installed: true, enabled: true, path: `/usr/bin/${name}`, version: '1.0.0',
  latest_version: null, origin: 'host', install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
});

const step = (index: number, extra: Record<string, unknown> = {}) => ({
  audit_run_id: 'run-1', step_index: index, file_label: `docs/step-${index}.md`,
  started_at: '2026-10-02T10:00:00Z', ended_at: '2026-10-02T10:01:00Z', duration_ms: 61_000,
  step_tokens: 10, cumulative_tokens: 10, cli_success: true, step_warning: null,
  step_repaired_from_template: false, ...extra,
});

type RunStub = { id: string; kind?: string; td_total?: number; status?: string; started_at?: string };

/** The timeline's single request: runs, their steps, the branch's recorded audits. */
function mockTimeline(runs: RunStub[], steps: unknown[], recorded: unknown[] = [], latestValidation: unknown = null) {
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: runs.map(r => ({
      project_id: 'p1', agent_type: 'ClaudeCode', started_at: '', status: 'Completed', td_total: 0, kind: 'Full', ...r,
    })),
    steps, recorded_audits: recorded, recorded_validated_at: null, latest_validation: latestValidation,
  } as never);
}

function props(over: Partial<AuditTimelineProps> = {}): AuditTimelineProps {
  return {
    projectId: 'p1', auditStatus: 'TemplateInstalled', techDebtCount: 0,
    agents: [agent('ClaudeCode', 'Claude Code'), agent('LiteLlm', 'LiteLLM'), agent('Ollama', 'Ollama')],
    selectedAgent: 'ClaudeCode', selectedTier: 'reasoning', modelTiers: null, selectedConnectionId: null, onSelect: vi.fn(),
    briefingDone: false, onBriefingSaved: vi.fn(), auditActive: false, liveStep: 0, liveTotal: 0,
    liveFile: '', liveElapsed: null, liveTool: null, liveStartedAt: null, liveToolCalls: null, liveStepTokens: null,
    liveTotalTokens: null, onResumeBriefingDiscussion: null, onCancel: vi.fn(), resumable: null,
    onLaunch: vi.fn(), validationInProgress: false, onValidate: vi.fn(),
    onMarkValid: vi.fn().mockResolvedValue(undefined), onOpenValidation: vi.fn(), onViewTechDebts: vi.fn(),
    refreshTrigger: 0, toast: vi.fn(), ...over,
  };
}

describe('AuditTimeline', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    try { localStorage.clear(); } catch { /* jsdom */ }
    mockTimeline([], []);
    vi.mocked(externalApi.list).mockResolvedValue([]);
    vi.mocked(projectsApi.auditSteps).mockResolvedValue([]);
  });

  it('shows each step of the resumable run with its status, reason, duration and file', async () => {
    const steps = Array.from({ length: 16 }, (_, i) => step(i + 1));
    steps[2] = step(3, { cli_success: false, step_warning: 'step did not fill `docs/step-3.md`: 4 raw `{{...}}` placeholders remain' });
    steps.pop();
    mockTimeline([], steps);
    const p = props({ resumable: { id: 'run-1', steps_to_redo: [3, 16], last_completed_step: 14 } });
    wrap(<AuditTimeline {...p} />);

    const failed = await screen.findByTestId('audit-timeline-step-3');
    expect(failed).toHaveClass('is-failed');
    expect(failed).toHaveTextContent('docs/step-3.md');
    expect(failed).toHaveTextContent('1 min 01');
    // The technical warning becomes a readable reason, and stays as its title.
    expect(failed.querySelector('.audit-tl-reason')).toHaveAttribute('title', expect.stringContaining('placeholders remain'));
    expect(screen.getByTestId('audit-timeline-step-16')).toHaveClass('is-todo');
    // One request for the whole timeline (FE-12), not one per run.
    expect(projectsApi.auditTimeline).toHaveBeenCalledWith('p1');
    expect(projectsApi.auditRunSteps).not.toHaveBeenCalled();
  });

  it('shows each step its tokens, the live step its running count (KT-994)', async () => {
    const steps = [
      step(1, {
        started_at: '2026-10-03T09:46:30Z', step_tokens: 48_213, input_tokens: 52_000, output_tokens: 8_213, cache_read_tokens: 12_000,
        breakdown: { uncached_input: 40_000, output: 8_213, cache_read: 12_000, cache_write: 500, total_with_cache: 60_713 },
      }),
      step(2, { ended_at: null, duration_ms: null, step_tokens: null, started_at: '2026-10-03T09:50:00Z' }),
    ];
    mockTimeline([{ id: 'run-1', started_at: '2026-10-03T09:46:00Z' }], steps);
    const p = props({
      auditActive: true, liveStep: 2, liveTotal: 16, liveFile: 'docs/step-2.md',
      liveStartedAt: Date.parse('2026-10-03T09:46:00Z'), liveStepTokens: 1_310_000,
    });
    wrap(<AuditTimeline {...p} />);

    const done = await screen.findByTestId('audit-timeline-step-tokens-1');
    expect(done).toHaveTextContent(/48[.,]2 k tk/);
    // The headline is the fresh traffic; the hover adds the cache and the total with it.
    expect(done.getAttribute('title')).toMatch(/40[\s,.\u202f]?000.*8[\s,.\u202f]?213.*12[\s,.\u202f]?000.*500.*60[\s,.\u202f]?713/);
    expect(screen.getByTestId('audit-timeline-step-tokens-2')).toHaveTextContent(/1[.,]31 M tk/);
  });

  it('offers the running step, and only it, the agent\'s latest actions', async () => {
    const steps = [
      step(1, { started_at: '2026-10-03T09:46:30Z' }),
      step(2, { ended_at: null, duration_ms: null, step_tokens: null, started_at: '2026-10-03T09:50:00Z' }),
    ];
    mockTimeline([{ id: 'run-1', started_at: '2026-10-03T09:46:00Z' }], steps);
    const p = props({
      auditActive: true, liveStep: 2, liveTotal: 16, liveFile: 'docs/step-2.md',
      liveStartedAt: Date.parse('2026-10-03T09:46:00Z'),
      liveTool: 'Read',
      liveActivity: { entries: [{ category: 'Read', at: new Date().toISOString() }] },
    });
    wrap(<AuditTimeline {...p} />);

    const running = await screen.findByTestId('audit-timeline-step-2');
    expect(screen.getByTestId('audit-timeline-step-1').querySelector('[data-testid="audit-step-activity"]')).toBeNull();
    const toggle = running.querySelector('button[aria-expanded]') as HTMLButtonElement;
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(toggle);
    expect(running).toHaveTextContent(/Lecture|Read/);
    // The chip shows the category's label too.
    expect(screen.getByTestId('audit-timeline-live')).toHaveTextContent(/Lecture|Read/);
  });

  it('does not present its own choice as the running agent when the server does not say (KT-994)', async () => {
    const p = props({ auditActive: true, liveStep: 1, liveTotal: 16, selectedAgent: 'ClaudeCode', liveAuditor: null });
    wrap(<AuditTimeline {...p} />);
    const claude = await screen.findByRole('button', { name: /Claude Code/ });
    expect(claude).toHaveAttribute('aria-pressed', 'false');
    expect(claude).toBeDisabled();
    expect(screen.queryByTestId('audit-timeline-auditor')).toBeNull();
    expect(screen.getAllByRole('button').filter(b => b.classList.contains('audit-tl-tier') && b.getAttribute('aria-pressed') === 'true')).toHaveLength(0);
  });

  it('counts a finished partial run, newest result per step (KT-994)', async () => {
    // A partial audit of step 3 that succeeded used to vanish: only Full runs
    // were merged, so the card read "0 of 16" right after it.
    mockTimeline([
      { id: 'partial-2', kind: 'Partial', started_at: '2026-10-03T10:47:00Z' },
      { id: 'full-1', kind: 'Full', started_at: '2026-10-02T10:00:00Z' },
    ], [
      step(3, { audit_run_id: 'partial-2', step_tokens: 328_708 }),
      step(1, { audit_run_id: 'full-1' }),
      step(3, { audit_run_id: 'full-1', cli_success: false, step_warning: 'interrupted' }),
    ]);
    const { container } = wrap(<AuditTimeline {...props()} />);
    await waitFor(() => expect(container.querySelector('.audit-tl-group-head')).not.toBeNull());
    container.querySelectorAll<HTMLElement>('.audit-tl-group-head').forEach(head => fireEvent.click(head));

    await waitFor(() => expect(screen.getByTestId('audit-timeline-step-3')).toHaveClass('is-done'));
    expect(screen.getByTestId('audit-timeline-step-1')).toHaveClass('is-done');
    expect(screen.getByTestId('audit-timeline-step-tokens-3')).toHaveTextContent(/328[.,]7 k tk/);
  });

  it('a partial run marks the step it runs and keeps the other results (KT-994)', async () => {
    // Progress counts the run's own steps: a partial audit of step 3 is at 1/1.
    vi.mocked(projectsApi.auditSteps).mockResolvedValue(
      Array.from({ length: 16 }, (_, i) => ({ index: i + 1, target_file: `docs/step-${i + 1}.md` })));
    mockTimeline([{ id: 'run-1', started_at: '2026-10-02T10:00:00Z' }], [step(1)]);
    const p = props({
      auditActive: true, liveStep: 1, liveTotal: 1, liveFile: 'docs/step-3.md',
      liveStartedAt: Date.parse('2026-10-03T10:47:00Z'),
    });
    wrap(<AuditTimeline {...p} />);

    await waitFor(() => expect(screen.getByTestId('audit-timeline-step-3')).toHaveClass('is-running'));
    expect(screen.getByTestId('audit-timeline-step-1')).toHaveClass('is-done');
    expect(screen.getByTestId('audit-timeline-live').textContent).toMatch(/3.*16.*docs\/step-3\.md/);
  });

  it('freezes the agent panel on the agent running the audit (KT-994)', async () => {
    // A card that did not launch the audit (page reloaded, another client)
    // still shows who runs it, and nothing can be changed or relaunched.
    vi.mocked(externalApi.list).mockResolvedValue([{
      id: 'conn-or', display_name: 'OpenRouter perso', mention_alias: 'openrouter', endpoint: 'https://openrouter.ai/api/v1',
      credential_slug: 'openrouter', origin_preset: 'open_router', economy_model: null, default_model: 'or/default',
      reasoning_model: 'or/reasoning', created_at: '', updated_at: '',
    }] as never);
    const p = props({
      auditActive: true, liveStep: 1, liveTotal: 16, liveFile: 'docs/AGENTS.md',
      liveTool: 'Read', liveToolCalls: 7,
      selectedAgent: 'ClaudeCode', selectedTier: 'default',
      liveAuditor: { agent: 'Custom', tier: 'reasoning', connectionId: 'conn-or' },
    });
    wrap(<AuditTimeline {...p} />);

    const auditor = await screen.findByTestId('audit-timeline-auditor');
    await waitFor(() => expect(auditor).toHaveTextContent('OpenRouter perso'));
    expect(auditor).toHaveTextContent('or/reasoning');
    const openRouter = screen.getByRole('button', { name: /OpenRouter perso/ });
    expect(openRouter).toHaveAttribute('aria-pressed', 'true');
    expect(openRouter).toBeDisabled();
    expect(screen.getByRole('button', { name: /Claude Code/ })).toBeDisabled();
    for (const tier of screen.getAllByRole('button').filter(b => b.classList.contains('audit-tl-tier'))) {
      expect(tier).toBeDisabled();
    }
    const launch = screen.getByTestId('audit-timeline-launch');
    expect(launch).toBeDisabled();
    // Disabled is not enough: the button says the audit is running.
    expect(launch).toHaveTextContent(/Audit en cours|Audit running/);
    // The audit read the briefing at start: editing it now would change nothing.
    expect(screen.getByTestId('audit-timeline-briefing-open')).toBeDisabled();
    expect(screen.getByTestId('audit-timeline-live')).toHaveTextContent(/(Lecture|Read) \(7\)/);
  });

  it('resumes a failed step through the resume launcher and says consolidation reruns', async () => {
    const steps = Array.from({ length: 15 }, (_, i) => step(i + 1));
    steps[2] = step(3, { cli_success: false, step_warning: 'interrupted' });
    mockTimeline([], steps);
    // The note needs to know which step consolidates: the plan names it.
    vi.mocked(projectsApi.auditSteps).mockResolvedValue(
      Array.from({ length: 16 }, (_, i) => ({ index: i + 1, target_file: i === 15 ? 'docs/decisions.md' : `docs/step-${i + 1}.md` })));
    const p = props({ resumable: { id: 'run-1', steps_to_redo: [3, 16], last_completed_step: 14 } });
    wrap(<AuditTimeline {...p} />);

    const failed = await screen.findByTestId('audit-timeline-step-3');
    // One resume per group, beside its title: every failed step reruns together.
    const resume = failed.closest('.audit-tl-group')?.querySelector('.audit-tl-group-row .audit-tl-btn-warn');
    expect(resume).not.toBeNull();
    expect(failed.querySelector('button')).toBeNull();
    fireEvent.click(resume as HTMLElement);
    expect(p.onLaunch).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(document.querySelector('.audit-tl-note')).not.toBeNull());
  });

  it('groups agents and warns for an HTTP agent, a local model and a lower tier', async () => {
    const p = props();
    const { rerender } = wrap(<AuditTimeline {...p} />);
    expect(screen.queryByTestId('audit-timeline-agent-warning')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /^LiteLLM/ }));
    expect(p.onSelect).toHaveBeenCalledWith('LiteLlm', 'reasoning', null);

    rerender(<I18nProvider><AuditTimeline {...props({ selectedAgent: 'LiteLlm' })} /></I18nProvider>);
    const httpWarning = screen.getByTestId('audit-timeline-agent-warning');
    expect(httpWarning.querySelectorAll('p')).toHaveLength(1);

    const local = props({ selectedAgent: 'Ollama', selectedTier: 'economy' });
    rerender(<I18nProvider><AuditTimeline {...local} /></I18nProvider>);
    const warning = screen.getByTestId('audit-timeline-agent-warning');
    expect(warning.querySelectorAll('p')).toHaveLength(2);
    fireEvent.click(warning.querySelector('button')!);
    expect(local.onSelect).toHaveBeenCalledWith('Ollama', 'reasoning', null);
  });

  it('lists named connections as HTTP agents and selects one with its id (KT-980)', async () => {
    vi.mocked(externalApi.list).mockResolvedValue([{
      id: 'conn-or', display_name: 'OpenRouter perso', mention_alias: 'openrouter', endpoint: 'https://openrouter.ai/api/v1',
      credential_slug: 'openrouter', origin_preset: 'open_router', economy_model: null, default_model: 'or/default',
      reasoning_model: 'or/reasoning', created_at: '', updated_at: '',
    }] as never);
    const p = props();
    const { rerender } = wrap(<AuditTimeline {...p} />);
    fireEvent.click(await screen.findByRole('button', { name: /^OpenRouter perso/ }));
    expect(p.onSelect).toHaveBeenCalledWith('Custom', 'reasoning', 'conn-or');

    rerender(<I18nProvider><AuditTimeline {...props({ selectedAgent: 'Custom', selectedConnectionId: 'conn-or' })} /></I18nProvider>);
    expect(await screen.findByText('or/reasoning')).toBeInTheDocument();
    expect(screen.getByTestId('audit-timeline-agent-warning')).toBeInTheDocument();
  });

  it('merges a resumed run with the run it continued, step by step', async () => {
    // A resume records only the steps it reran (here 9-16); 1-8 are in the parent.
    mockTimeline([{ id: 'resumed' }, { id: 'parent', status: 'Interrupted' }], [
      ...Array.from({ length: 8 }, (_, i) => step(i + 9, { audit_run_id: 'resumed' })),
      ...Array.from({ length: 8 }, (_, i) => step(i + 1, { audit_run_id: 'parent' })),
    ]);
    wrap(<AuditTimeline {...props({ auditStatus: 'Audited' })} />);
    const progress = await screen.findByRole('progressbar');
    await waitFor(() => expect(progress).toHaveAttribute('aria-valuenow', '16'));
  });

  it('shows a step that never ended as interrupted when no audit runs', async () => {
    const steps = Array.from({ length: 16 }, (_, i) => step(i + 1));
    steps[5] = step(6, { ended_at: null, duration_ms: null });
    mockTimeline([], steps);
    wrap(<AuditTimeline {...props({ resumable: { id: 'run-1', steps_to_redo: [6, 16], last_completed_step: 14 } })} />);
    const interrupted = await screen.findByTestId('audit-timeline-step-6');
    expect(interrupted).toHaveClass('is-failed');
    expect(interrupted.querySelector('.audit-tl-reason')).not.toBeNull();
  });

  it('shows every step, its file and what it produces before any audit ran', async () => {
    // feat/0.14.2 order: the consolidation (decisions.md) is step 9, not 16.
    const files = ['AGENTS', 'glossary', 'repo-map', 'coding-rules', 'testing-quality', 'architecture/overview',
      'operations/debug-operations', 'inconsistencies-tech-debt', 'decisions', 'inconsistencies-security',
      'inconsistencies-docker', 'inconsistencies-performance', 'inconsistencies-accessibility',
      'inconsistencies-database', 'inconsistencies-api', 'inconsistencies-code-quality'];
    vi.mocked(projectsApi.auditSteps).mockResolvedValue(files.map((f, i) => ({ index: i + 1, target_file: `docs/${f}.md` })));
    wrap(<AuditTimeline {...props()} />);

    await waitFor(() => expect(document.querySelectorAll('.audit-tl-group-head').length).toBe(3));
    for (const group of document.querySelectorAll('.audit-tl-group-head')) fireEvent.click(group);
    const consolidation = await screen.findByTestId('audit-timeline-step-9');
    expect(consolidation).toHaveTextContent('docs/decisions.md');
    expect(consolidation.querySelector('.audit-tl-step-desc')).not.toBeNull();
    expect(consolidation.closest('.audit-tl-group')?.querySelector('.audit-tl-group-head'))
      .toHaveTextContent(/consolidation/i);
    expect(screen.getByTestId('audit-timeline-step-10').closest('.audit-tl-group'))
      .not.toBe(consolidation.closest('.audit-tl-group'));
  });

  it('dates each step with its own last run', async () => {
    const steps = Array.from({ length: 16 }, (_, i) => step(i + 1));
    steps[2] = step(3, { cli_success: false, step_warning: 'interrupted', ended_at: '2026-09-30T19:45:00Z' });
    mockTimeline([], steps);
    wrap(<AuditTimeline {...props({ resumable: { id: 'run-1', steps_to_redo: [3], last_completed_step: 15 } })} />);
    const row = await screen.findByTestId('audit-timeline-step-3');
    expect(row.querySelector('time')).toHaveAttribute('dateTime', '2026-09-30T19:45:00Z');
  });

  it('does not count an older run as done while a fresh audit runs', async () => {
    mockTimeline([], Array.from({ length: 16 }, (_, i) => step(i + 1)));
    wrap(<AuditTimeline {...props({
      auditStatus: 'Audited', auditActive: true, liveStep: 2, liveTotal: 16, liveFile: 'docs/x.md',
      liveStartedAt: Date.parse('2026-10-03T08:00:00Z'),
    })} />);
    const progress = await screen.findByRole('progressbar');
    await waitFor(() => expect(progress).toHaveAttribute('aria-valuenow', '0'));
    expect(screen.getByTestId('audit-timeline-step-2')).toHaveClass('is-running');
  });

  it('says how many debts the last audit produced', async () => {
    mockTimeline([{ id: 'r', td_total: 40 }], Array.from({ length: 16 }, (_, i) => step(i + 1, { audit_run_id: 'r' })));
    wrap(<AuditTimeline {...props({ auditStatus: 'Audited' })} />);
    expect(await screen.findByTestId('audit-timeline-td-count')).toHaveTextContent('40');
  });

  it('shows audits known only from the state file, with date and provenance (KT-993)', async () => {
    mockTimeline([], [], [
      { date: '2026-08-13', kronn_version: '0.9.6', type: 'attested', provenance: 'human_attestation' },
      { date: '2026-09-01', kronn_version: '0.14.1', type: 'full', provenance: 'kronn_audit' },
      { date: '2026-05-17', kronn_version: 'legacy', type: 'legacy', provenance: 'legacy_evidence' },
    ]);
    wrap(<AuditTimeline {...props({ auditStatus: 'Audited' })} />);

    const entries = await screen.findAllByTestId('audit-timeline-recorded-entry');
    // Newest record first; each keeps its own date, provenance and version.
    expect(entries).toHaveLength(3);
    expect(entries[0]).toHaveTextContent('2026-05-17');
    expect(entries[0]).toHaveTextContent(/Preuve héritée|Legacy evidence/);
    expect(entries[0]).toHaveTextContent(/version inconnue|version unknown/);
    expect(entries[1]).toHaveTextContent('2026-09-01');
    expect(entries[1]).toHaveTextContent(/autre instance|another instance/);
    expect(entries[1]).toHaveTextContent('Kronn 0.14.1');
    expect(entries[2]).toHaveTextContent('2026-08-13');
    expect(entries[2]).toHaveTextContent(/Attesté par une personne|Attested by a person/);
    expect(screen.queryByText(/Aucun audit lancé|No audit launched/)).toBeNull();
  });

  it('keeps the recorded audits out of the way once this instance has runs', async () => {
    mockTimeline([{ id: 'run-1' }], [step(1)], [
      { date: '2026-08-13', kronn_version: '0.9.6', type: 'attested', provenance: 'human_attestation' },
    ]);
    wrap(<AuditTimeline {...props({ auditStatus: 'Audited' })} />);
    await screen.findByRole('progressbar');
    expect(screen.queryByTestId('audit-timeline-recorded')).toBeNull();
  });

  it('says no audit ran when neither a run nor a record exists', async () => {
    wrap(<AuditTimeline {...props()} />);
    expect(await screen.findByText(/Aucun audit lancé|No audit launched/)).toBeInTheDocument();
    expect(screen.queryByTestId('audit-timeline-recorded')).toBeNull();
  });

  it('tells unknown tokens from zero and names the run a carried step came from (KT-1021)', async () => {
    mockTimeline([{ id: 'run-1' }], [
      step(1, { step_tokens: null, input_tokens: null, output_tokens: null }),
      step(2, { step_tokens: 0, input_tokens: 0, output_tokens: 0 }),
      step(3, { step_tokens: 5, input_tokens: 4, output_tokens: 1, carried_from_run_id: 'abcdef1234567890' }),
    ]);
    const { container } = wrap(<AuditTimeline {...props({ auditStatus: 'Audited' })} />);
    await waitFor(() => expect(container.querySelector('.audit-tl-group-head')).not.toBeNull());
    container.querySelectorAll<HTMLElement>('.audit-tl-group-head').forEach(head => {
      if (head.getAttribute('aria-expanded') === 'false') fireEvent.click(head);
    });

    const unknown = await screen.findByTestId('audit-timeline-step-tokens-1');
    expect(unknown).toHaveTextContent('?');
    expect(unknown.getAttribute('title')).toMatch(/inconnu|unknown/);
    expect(screen.getByTestId('audit-timeline-step-tokens-2')).toHaveTextContent(/^0 tk$/);
    const carried = screen.getByTestId('audit-timeline-step-tokens-3');
    expect(carried).toHaveTextContent('5 tk');
    expect(carried.getAttribute('title')).toContain('abcdef12');
  });

  const expandAll = (container: HTMLElement) => {
    container.querySelectorAll<HTMLElement>('.audit-tl-group-head').forEach(head => {
      if (head.getAttribute('aria-expanded') === 'false') fireEvent.click(head);
    });
  };

  it('shows each step\'s reported cost, a real zero as 0 and an unreported one as unknown (KT-997)', async () => {
    mockTimeline([{ id: 'run-1' }], [
      step(1, { cost_usd_micros: 420_000 }),
      step(2, { cost_usd_micros: 0 }),
      step(3, { cost_usd_micros: null }),
      step(4, { cost_usd_micros: 3_100 }),
    ]);
    const { container } = wrap(<AuditTimeline {...props({ auditStatus: 'Audited' })} />);
    await waitFor(() => expect(container.querySelector('.audit-tl-group-head')).not.toBeNull());
    expandAll(container);

    expect(await screen.findByTestId('audit-timeline-step-cost-1')).toHaveTextContent(/^~0[.,]42 \$$/);
    expect(screen.getByTestId('audit-timeline-step-cost-2')).toHaveTextContent(/^~0[.,]00 \$$/);
    const unknown = screen.getByTestId('audit-timeline-step-cost-3');
    expect(unknown).toHaveTextContent(/\?$/);
    expect(unknown).not.toHaveTextContent(/0/);
    expect(unknown.getAttribute('title')).toMatch(/inconnu|unknown/);
    expect(screen.getByTestId('audit-timeline-step-cost-4')).toHaveTextContent(/^~0[.,]0031 \$$/);
    // One unknown step: the total is a floor, and says so.
    const total = screen.getByTestId('audit-timeline-cost-total');
    expect(total).toHaveTextContent(/≥ 0[.,]42 \$/);
    expect(total).toHaveTextContent(/1/);
  });

  it('says the cache split is unknown for an agent pricing does not know', async () => {
    mockTimeline([{ id: 'run-1' }], [
      step(1, { step_tokens: 1_500, breakdown: { input_as_reported: 1_000, output: 500, cache_read: 300 } }),
    ]);
    const { container } = wrap(<AuditTimeline {...props({ auditStatus: 'Audited' })} />);
    await waitFor(() => expect(container.querySelector('.audit-tl-group-head')).not.toBeNull());
    expandAll(container);
    const figure = await screen.findByTestId('audit-timeline-step-tokens-1');
    expect(figure.getAttribute('title')).toMatch(/inconnue|unknown/);
    expect(figure.getAttribute('title')).not.toMatch(/total/i);
  });

  it('shows an estimated cost apart from a reported one, and why a cost is unknown', async () => {
    const steps = [
      step(1, { cost_usd_micros: 420_000 }),
      step(2, { estimated_cost_usd_micros: 30_000 }),
      step(3, { cost_unknown_reason: 'no confirmed rate for the serving model' }),
    ];
    mockTimeline([{ id: 'run-1', started_at: '2026-10-02T09:59:00Z' }], steps);
    const { container } = wrap(<AuditTimeline {...props({ auditStatus: 'Audited' })} />);
    await waitFor(() => expect(container.querySelector('.audit-tl-group-head')).not.toBeNull());
    expandAll(container);

    expect(await screen.findByTestId('audit-timeline-step-cost-1')).toHaveTextContent(/~0[.,]42 \$/);
    const estimated = screen.getByTestId('audit-timeline-step-cost-2');
    expect(estimated).toHaveTextContent(/≈0[.,]03 \$/);
    expect(estimated.getAttribute('title')).toBeTruthy();
    const unknown = screen.getByTestId('audit-timeline-step-cost-3');
    expect(unknown).toHaveTextContent(/\?/);
    // A known pricing reason reads in the UI's language.
    expect(unknown.getAttribute('title')).toMatch(/aucun tarif confirmé|no confirmed rate/);
    // One estimated step makes the total estimated; the unknown one makes it a floor.
    expect(screen.getByTestId('audit-timeline-cost-total')).toHaveTextContent(/≥ ≈0[.,]45 \$/);
  });

  it('totals the run exactly when every step reported, and unknown when none did (KT-997)', async () => {
    mockTimeline([{ id: 'run-1' }], [step(1, { cost_usd_micros: 400_000 }), step(2, { cost_usd_micros: 20_000 })]);
    const { unmount } = wrap(<AuditTimeline {...props({ auditStatus: 'Audited' })} />);
    expect(await screen.findByTestId('audit-timeline-cost-total')).toHaveTextContent(/~0[.,]42 \$/);
    expect(screen.getByTestId('audit-timeline-cost-total')).not.toHaveTextContent('≥');
    unmount();

    mockTimeline([{ id: 'run-1' }], [step(1), step(2)]);
    wrap(<AuditTimeline {...props({ auditStatus: 'Audited' })} />);
    const total = await screen.findByTestId('audit-timeline-cost-total');
    expect(total).toHaveTextContent(/\?/);
    expect(total).not.toHaveTextContent(/\d/);
  });

  it('saves the briefing without creating a discussion', async () => {
    const p = props();
    wrap(<AuditTimeline {...p} />);
    fireEvent.click(screen.getByTestId('audit-timeline-briefing-open'));
    const fields = document.querySelectorAll('.briefing-form textarea');
    fields.forEach((field, i) => { if (i < 5) fireEvent.change(field, { target: { value: `answer ${i}` } }); });
    await act(async () => { fireEvent.submit(document.querySelector('.briefing-form')!); });

    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    expect(projectsApi.startBriefing).not.toHaveBeenCalled();
    expect(p.onBriefingSaved).toHaveBeenCalledTimes(1);
  });

  it('explains once what changed, and remembers the dismissal', () => {
    const { unmount } = wrap(<AuditTimeline {...props()} />);
    const news = document.querySelector('.audit-tl-news')!;
    fireEvent.click(news.querySelector('button')!);
    expect(document.querySelector('.audit-tl-news')).toBeNull();
    unmount();
    wrap(<AuditTimeline {...props()} />);
    expect(document.querySelector('.audit-tl-news')).toBeNull();
  });

  it('shows the live step and cancels from the timeline while an audit runs', () => {
    const p = props({ auditActive: true, liveStep: 4, liveTotal: 16, liveFile: 'docs/coding-rules.md' });
    wrap(<AuditTimeline {...p} />);
    const live = screen.getByTestId('audit-timeline-live');
    fireEvent.click(live.querySelector('button')!);
    expect(p.onCancel).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('audit-timeline-launch')).toBeDisabled();
  });
  it('validates directly from a finished linked validation, archived, and opens it', async () => {
    mockTimeline([{ id: 'run-1' }], [step(1)], [], { discussion_id: 'd-val', finished: true, archived: true });
    const p = props({ auditStatus: 'Audited' });
    wrap(<AuditTimeline {...p} />);

    const block = await screen.findByTestId('audit-timeline-validation-finished');
    fireEvent.click(within(block).getByRole('button', { name: /Valider l'audit/ }));
    await waitFor(() => expect(p.onMarkValid).toHaveBeenCalledTimes(1));
    expect(p.onValidate).not.toHaveBeenCalled();
    fireEvent.click(within(block).getByRole('button', { name: /Voir la discussion de validation/ }));
    expect(p.onOpenValidation).toHaveBeenCalledWith('d-val');
  });

  it('keeps starting or resuming a validation while the linked one is unfinished', async () => {
    mockTimeline([{ id: 'run-1' }], [step(1)], [], { discussion_id: 'd-val', finished: false, archived: false });
    const p = props({ auditStatus: 'Audited' });
    wrap(<AuditTimeline {...p} />);

    fireEvent.click(await screen.findByRole('button', { name: /Valider l'audit/ }));
    expect(p.onValidate).toHaveBeenCalledTimes(1);
    expect(p.onMarkValid).not.toHaveBeenCalled();
    expect(screen.queryByTestId('audit-timeline-validation-finished')).toBeNull();
    expect(screen.queryByRole('button', { name: /Voir la discussion de validation/ })).toBeNull();
  });
});
