// « Vérifier la doc »: the panel lists what the deterministic
// check found, acknowledges a section, runs the revalidation, and offers
// « Reprendre » after every error, stop or interruption.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, act, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { buildApiMock } from '../../test/apiMock';
import { expectFocusKeptIn } from '../../test/dialogFocus';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));
vi.mock('../../hooks/useMediaQuery', () => ({ useIsMobile: () => false }));

import { RevalidateAudit } from '../RevalidateAudit';
import { ProjectCard } from '../ProjectCard';
import { projects as projectsApi } from '../../lib/api';
import type { RevalidationDoneInfo } from '../../lib/api';
import type { AgentDetection, Project, RevalidationPlan } from '../../types/generated';

const PROJECT: Project = {
  id: 'p-reval',
  name: 'reval-target',
  path: '/repos/reval-target',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'Validated',
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const NO_RELEVANCE = {
  sections: 0, anchors_intact: 0, outdated: 0, no_longer_relevant: 0, unanchored: 0,
  untouched_stale: 0, missing_references: 0, prose_missing: 0, anchored_intact_pct: null,
};

const PLAN: RevalidationPlan = {
  relevance: NO_RELEVANCE,
  unanchored: [], unanchored_docs: [],
  sources_state: 'needs_reverification',
  agent_steps: [4],
  reanchor_count: 1,
  human_count: 1,
  sections: [
    {
      doc: 'docs/coding-rules.md', step: 4, action: 'agent', doc_missing: false,
      reanchor: [{ cite: 'src/a.rs:2', file: 'src/a.rs', line: 2, end_line: 2, state: 'moved', current_line: 5 }],
      broken: [{ cite: 'src/a.rs:9', file: 'src/a.rs', line: 9, end_line: 9, state: 'changed', current_line: null }],
      human: [], relevance: [],
    },
    {
      doc: 'docs/glossary.md', step: 2, action: 'human', doc_missing: false,
      reanchor: [], broken: [],
      human: [{
        section: 'owner-notes',
        citation: { cite: 'src/old.rs:3', file: 'src/old.rs', line: 3, end_line: 3, state: 'file_missing', current_line: null },
      }],
      relevance: [],
    },
  ],
};

const CLEAN: RevalidationPlan = {
  sources_state: 'clean', sections: [], agent_steps: [], reanchor_count: 0, human_count: 0,
  relevance: NO_RELEVANCE,
  unanchored: [], unanchored_docs: [],
};

type Handlers = Parameters<typeof projectsApi.revalidateAuditStream>[2];

function setup(overrides: Partial<React.ComponentProps<typeof RevalidateAudit>> = {}) {
  const props = {
    project: PROJECT,
    auditActive: false,
    agent: 'ClaudeCode' as const,
    tier: 'reasoning' as const,
    connectionId: null,
    toast: vi.fn(),
    onRefetch: vi.fn(),
    onRefetchDrift: vi.fn(),
    onOpenDiscussion: vi.fn(),
    onNavigate: vi.fn(),
    ...overrides,
  };
  render(<RevalidateAudit {...props} />);
  return props;
}

async function openPanel() {
  await act(async () => {
    fireEvent.click(screen.getByTestId('project-revalidate'));
    await Promise.resolve();
  });
  return screen.findByTestId('revalidate-panel');
}

/** Launch with a stream whose handlers the test drives. */
async function launchHeld(): Promise<Handlers> {
  let captured: Handlers | null = null;
  vi.mocked(projectsApi.revalidateAuditStream).mockImplementationOnce(async (_id, _req, handlers) => {
    captured = handlers;
  });
  await act(async () => {
    fireEvent.click(await screen.findByTestId('revalidate-launch'));
    await Promise.resolve();
  });
  expect(captured).not.toBeNull();
  return captured as unknown as Handlers;
}

async function finish(run: () => void) {
  await act(async () => { run(); await Promise.resolve(); await Promise.resolve(); });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(projectsApi.revalidationPlan).mockResolvedValue(PLAN);
});

describe('RevalidateAudit', () => {
  it('waits for the template integration gate before starting a revalidation', async () => {
    let pending: (() => void) | null = null;
    const gateLaunch = vi.fn(async (launch: () => void) => { pending = launch; });
    setup({ gateLaunch });
    await openPanel();
    await act(async () => { fireEvent.click(await screen.findByTestId('revalidate-launch')); });
    expect(gateLaunch).toHaveBeenCalledTimes(1);
    expect(projectsApi.revalidateAuditStream).not.toHaveBeenCalled();
    await act(async () => { pending?.(); });
    expect(projectsApi.revalidateAuditStream).toHaveBeenCalledTimes(1);
  });

  it('shows only for an audited or validated project', () => {
    setup({ project: { ...PROJECT, audit_status: 'NoTemplate' } });
    expect(screen.queryByTestId('project-revalidate')).toBeNull();
    setup({ project: { ...PROJECT, audit_status: 'Audited' } });
    expect(screen.getByTestId('project-revalidate')).toBeEnabled();
  });

  it('cannot start while another audit of the project runs', () => {
    setup({ auditActive: true });
    expect(screen.getByTestId('project-revalidate')).toBeDisabled();
  });

  it('an audit that starts meanwhile withdraws the launch', async () => {
    const props = {
      project: PROJECT, auditActive: false, agent: 'ClaudeCode' as const, tier: 'reasoning' as const,
      connectionId: null, toast: vi.fn(), onRefetch: vi.fn(), onRefetchDrift: vi.fn(),
      onOpenDiscussion: vi.fn(), onNavigate: vi.fn(),
    };
    const { rerender } = render(<RevalidateAudit {...props} />);
    const panel = await openPanel();
    expect(await within(panel).findByTestId('revalidate-launch')).toBeEnabled();
    rerender(<RevalidateAudit {...props} auditActive />);
    expect(within(panel).queryByTestId('revalidate-launch')).toBeNull();
  });

  it('lists each section with what Kronn will do, before anything runs', async () => {
    setup();
    const panel = await openPanel();
    expect(projectsApi.revalidationPlan).toHaveBeenCalledWith('p-reval');
    const sections = await within(panel).findAllByTestId('revalidate-section');
    expect(sections.map(s => s.getAttribute('data-action'))).toEqual(['agent', 'human']);
    expect(sections[0]).toHaveTextContent('src/a.rs:9');
    expect(sections[0]).toHaveTextContent('audit.revalidate.state.changed');
    expect(sections[0]).toHaveTextContent('audit.revalidate.state.moved 5');
    const todo = within(sections[1]).getByTestId('revalidate-human-todo');
    expect(todo).toHaveTextContent('audit.revalidate.humanTodo owner-notes');
    expect(todo).toHaveTextContent('src/old.rs:3');
    expect(panel).toHaveTextContent('audit.revalidate.summary 2 1 1 1');
    expect(projectsApi.revalidateAuditStream).not.toHaveBeenCalled();
    expect(within(panel).getByTestId('revalidate-launch')).toBeEnabled();
  });

  it('explains stale docs with their step, changed sources, and smallest actions', async () => {
    vi.mocked(projectsApi.auditSteps).mockResolvedValueOnce([
      { index: 4, target_file: 'docs/coding-rules.md' },
    ] as never);
    const onUpdateStale = vi.fn();
    const onAcknowledgeStale = vi.fn();
    const onReconfirm = vi.fn();
    setup({
      drift: {
        audit_date: '2026-10-12',
        stale_sections: [{
          ai_file: 'docs/coding-rules.md',
          audit_step: 4,
          changed_sources: ['frontend/src/App.tsx', 'backend/src/main.rs'],
          missing_paths: [{ section: 'Routing', paths: ['frontend/src/lib/gone.ts'] }],
        }],
        fresh_sections: [], total_sections: 1, sources_state: 'needs_reverification',
        broken_citations: 2, placeholder_docs: [],
      },
      reconfirmCount: 1,
      docStateUnknown: false,
      onUpdateStale,
      onAcknowledgeStale,
      onReconfirm,
      onFullAudit: vi.fn(),
    });

    const panel = await openPanel();
    const stale = within(panel).getByTestId('revalidate-stale-details');
    expect(stale).toHaveTextContent('docState.stale.explanation');
    expect(stale).toHaveTextContent('docs/coding-rules.md');
    expect(stale).toHaveTextContent('docState.stale.step 4 docs/coding-rules.md');
    expect(stale).toHaveTextContent('frontend/src/App.tsx');
    expect(stale).toHaveTextContent('backend/src/main.rs');
    // The gone prose paths are shown with their section before they can be accepted.
    expect(within(stale).getByTestId('revalidate-stale-missing-paths')).toHaveTextContent('frontend/src/lib/gone.ts');
    expect(within(stale).getByTestId('revalidate-stale-missing-paths')).toHaveTextContent('Routing');
    expect(stale).toHaveTextContent('docState.stale.updateEffect');
    expect(stale).toHaveTextContent('docState.stale.unchangedEffect');

    const docState = within(panel).getByTestId('doc-state-block');
    expect(within(docState).getByTestId('doc-state-broken')).toHaveTextContent('docState.action.fixBroken');
    expect(within(docState).getByTestId('doc-state-reanchor')).toHaveTextContent('docState.action.reanchor');
    expect(within(docState).getByTestId('doc-state-stale')).toHaveTextContent('docState.action.updateStale');
    expect(within(docState).getByTestId('doc-state-reconfirm')).toHaveTextContent('docState.action.reconfirm');

    fireEvent.click(within(docState).getByTestId('doc-state-stale'));
    expect(onUpdateStale).toHaveBeenCalledWith([4]);
    fireEvent.click(within(stale).getByTestId('revalidate-stale-ack'));
    expect(onAcknowledgeStale).toHaveBeenCalledTimes(1);
    fireEvent.click(within(docState).getByTestId('doc-state-reconfirm'));
    expect(onReconfirm).toHaveBeenCalledTimes(1);
    fireEvent.click(within(docState).getByTestId('doc-state-broken'));
    expect(projectsApi.revalidateAuditStream).toHaveBeenCalledTimes(1);
    expect(within(panel).getByText('docState.fullAudit.title')).toBeInTheDocument();
  });

  it('« Vérifié, inchangé » acknowledges one section and reloads the plan', async () => {
    const props = setup();
    const panel = await openPanel();
    vi.mocked(projectsApi.revalidationPlan).mockResolvedValueOnce({ ...PLAN, sections: [PLAN.sections[0]] });
    const human = (await within(panel).findAllByTestId('revalidate-section'))[1];
    await act(async () => {
      fireEvent.click(within(human).getByTestId('revalidate-ack'));
      await Promise.resolve();
    });
    expect(projectsApi.acknowledgeCitations).toHaveBeenCalledWith('p-reval', 'docs/glossary.md');
    expect(props.toast).toHaveBeenCalledWith('audit.revalidate.ackDone docs/glossary.md', 'success');
    expect(props.onRefetchDrift).toHaveBeenCalledWith('p-reval');
    expect(await within(panel).findAllByTestId('revalidate-section')).toHaveLength(1);
  });

  it('a failed acknowledgement says why and changes nothing', async () => {
    const props = setup();
    const panel = await openPanel();
    vi.mocked(projectsApi.acknowledgeCitations).mockRejectedValueOnce(new Error('lease held'));
    const first = (await within(panel).findAllByTestId('revalidate-section'))[0];
    await act(async () => {
      fireEvent.click(within(first).getByTestId('revalidate-ack'));
      await Promise.resolve();
    });
    expect(props.toast).toHaveBeenCalledWith('audit.revalidate.ackFailed lease held', 'error');
    expect(within(panel).getAllByTestId('revalidate-section')).toHaveLength(2);
  });

  it('runs with the chosen agent, shows the step, and Stop reaches the cancel API', async () => {
    setup({ agent: 'Codex', tier: 'economy', connectionId: 'conn-1' });
    const panel = await openPanel();
    const handlers = await launchHeld();
    expect(projectsApi.revalidateAuditStream).toHaveBeenCalledWith(
      'p-reval',
      { agent: 'Codex', tier: 'economy', connection_id: 'conn-1' },
      expect.any(Object),
      expect.any(AbortSignal),
    );
    expect(panel).toHaveAttribute('data-phase', 'running');
    await finish(() => handlers.onStepStart?.(1, 1, 'docs/coding-rules.md'));
    expect(within(panel).getByTestId('revalidate-progress'))
      .toHaveTextContent('audit.revalidate.runningStep 1 1 docs/coding-rules.md');
    expect(within(panel).queryByTestId('revalidate-launch')).toBeNull();
    await act(async () => {
      fireEvent.click(within(panel).getByTestId('revalidate-stop'));
      await Promise.resolve();
    });
    expect(projectsApi.cancelAudit).toHaveBeenCalledWith('p-reval');
  });

  it('keeps re-anchoring deterministic in a mixed plan, including after retry', async () => {
    setup({
      drift: {
        audit_date: '2026-10-12', stale_sections: [], fresh_sections: [], total_sections: 2,
        sources_state: 'needs_reverification', broken_citations: 1, placeholder_docs: [],
      },
      reconfirmCount: 0,
    });
    const panel = await openPanel();
    let firstHandlers: Handlers | null = null;
    vi.mocked(projectsApi.revalidateAuditStream).mockImplementationOnce(async (_id, _req, handlers) => {
      firstHandlers = handlers;
    });

    fireEvent.click(within(panel).getByTestId('doc-state-reanchor'));
    expect(projectsApi.revalidateAuditStream).toHaveBeenNthCalledWith(
      1,
      'p-reval',
      { agent: 'ClaudeCode', tier: 'reasoning', connection_id: undefined, deterministic_only: true },
      expect.any(Object),
      expect.any(AbortSignal),
    );

    await finish(() => (firstHandlers as unknown as Handlers).onError('temporary failure'));
    expect(await within(panel).findByRole('alert')).toHaveTextContent('audit.revalidate.error temporary failure');
    let retryHandlers: Handlers | null = null;
    vi.mocked(projectsApi.revalidateAuditStream).mockImplementationOnce(async (_id, _req, handlers) => {
      retryHandlers = handlers;
    });
    await act(async () => {
      fireEvent.click(within(panel).getByTestId('revalidate-resume'));
      await Promise.resolve();
    });

    expect(projectsApi.revalidateAuditStream).toHaveBeenNthCalledWith(
      2,
      'p-reval',
      { agent: 'ClaudeCode', tier: 'reasoning', connection_id: undefined, deterministic_only: true },
      expect.any(Object),
      expect.any(AbortSignal),
    );

    await finish(() => (retryHandlers as unknown as Handlers).onDone({
      kind: 'deterministic', status: 'todo', reanchored: 1,
    }));
    vi.mocked(projectsApi.revalidateAuditStream).mockImplementationOnce(async () => {});
    fireEvent.click(within(panel).getByTestId('doc-state-broken'));
    expect(projectsApi.revalidateAuditStream).toHaveBeenNthCalledWith(
      3,
      'p-reval',
      { agent: 'ClaudeCode', tier: 'reasoning', connection_id: undefined },
      expect.any(Object),
      expect.any(AbortSignal),
    );
  });

  it('a stop ends on « Reprendre », which relaunches', async () => {
    const props = setup();
    const panel = await openPanel();
    const handlers = await launchHeld();
    await finish(() => handlers.onCancelled());
    expect(await within(panel).findByText('audit.revalidate.cancelled')).toBeInTheDocument();
    expect(props.onRefetchDrift).toHaveBeenCalledWith('p-reval');
    expect(within(panel).queryByTestId('revalidate-launch')).toBeNull();
    await launchResume(panel);
    expect(projectsApi.revalidateAuditStream).toHaveBeenCalledTimes(2);
  });

  it('an error is never left without « Reprendre »', async () => {
    setup();
    const panel = await openPanel();
    const handlers = await launchHeld();
    await finish(() => handlers.onError('agent unreachable'));
    expect(await within(panel).findByRole('alert')).toHaveTextContent('audit.revalidate.error agent unreachable');
    await launchResume(panel);
    expect(projectsApi.revalidateAuditStream).toHaveBeenCalledTimes(2);
  });

  it('an interrupted run offers « Reprendre »', async () => {
    setup();
    const panel = await openPanel();
    const handlers = await launchHeld();
    const info: RevalidationDoneInfo = {
      kind: 'agent',
      info: {
        status: 'interrupted', discussionId: null, auditRunId: 'r1',
        succeededSteps: [], unchangedSteps: [], failedSteps: [4],
      },
    };
    await finish(() => handlers.onDone(info));
    expect(await within(panel).findByRole('alert')).toHaveTextContent('audit.revalidate.done.interrupted 1');
    expect(within(panel).getByTestId('revalidate-resume')).toBeEnabled();
  });

  it('a plan that cannot load offers « Reprendre » and loads it again', async () => {
    vi.mocked(projectsApi.revalidationPlan).mockRejectedValueOnce(new Error('backend down'));
    setup();
    const panel = await openPanel();
    expect(await within(panel).findByRole('alert')).toHaveTextContent('audit.revalidate.error backend down');
    await act(async () => {
      fireEvent.click(within(panel).getByTestId('revalidate-resume'));
      await Promise.resolve();
    });
    expect(await within(panel).findAllByTestId('revalidate-section')).toHaveLength(2);
    expect(projectsApi.revalidateAuditStream).not.toHaveBeenCalled();
  });

  it('corrections made: opens the validation discussion', async () => {
    const props = setup();
    const panel = await openPanel();
    const handlers = await launchHeld();
    vi.mocked(projectsApi.revalidationPlan).mockResolvedValueOnce(CLEAN);
    await finish(() => handlers.onDone({
      kind: 'agent',
      info: {
        status: 'complete', discussionId: 'd-val', auditRunId: 'r2',
        succeededSteps: [4], unchangedSteps: [], failedSteps: [],
      },
    }));
    expect(await within(panel).findByTestId('revalidate-clean')).toBeInTheDocument();
    expect(props.onRefetch).toHaveBeenCalled();
    fireEvent.click(within(panel).getByText('audit.revalidate.done.openValidation'));
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('d-val');
    expect(props.onNavigate).toHaveBeenCalledWith('discussions');
    expect(within(panel).queryByTestId('revalidate-resume')).toBeNull();
  });

  it('a run that rewrote nothing asks to confirm each section', async () => {
    setup();
    const panel = await openPanel();
    const handlers = await launchHeld();
    await finish(() => handlers.onDone({
      kind: 'agent',
      info: {
        status: 'no_change', discussionId: null, auditRunId: 'r3',
        succeededSteps: [], unchangedSteps: [4], failedSteps: [],
      },
    }));
    expect(await within(panel).findByText('audit.revalidate.done.noChange')).toBeInTheDocument();
    expect(within(panel).getAllByTestId('revalidate-ack').length).toBeGreaterThan(0);
  });

  it('a run that rewrote nothing on an unvalidated project opens its validation', async () => {
    const props = setup();
    const panel = await openPanel();
    const handlers = await launchHeld();
    await finish(() => handlers.onDone({
      kind: 'agent',
      info: {
        status: 'no_change', discussionId: 'd-reverified', auditRunId: 'r4',
        succeededSteps: [], unchangedSteps: [4], failedSteps: [],
      },
    }));
    expect(await within(panel).findByText('audit.revalidate.done.noChange')).toBeInTheDocument();
    fireEvent.click(within(panel).getByText('audit.revalidate.done.openValidation'));
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('d-reverified');
  });

  it('re-anchoring alone ends clean without an agent', async () => {
    setup();
    const panel = await openPanel();
    const handlers = await launchHeld();
    vi.mocked(projectsApi.revalidationPlan).mockResolvedValueOnce(CLEAN);
    await finish(() => handlers.onDone({ kind: 'deterministic', status: 'clean', reanchored: 2 }));
    expect(await within(panel).findByText('audit.revalidate.done.clean 2')).toBeInTheDocument();
    expect(within(panel).getByTestId('revalidate-clean')).toBeInTheDocument();
    expect(within(panel).queryByTestId('revalidate-launch')).toBeNull();
  });

  it('without fingerprints it explains and offers no launch', async () => {
    vi.mocked(projectsApi.revalidationPlan).mockResolvedValue({ ...CLEAN, sources_state: 'unknown' });
    setup();
    const panel = await openPanel();
    expect(await within(panel).findByTestId('revalidate-unknown')).toBeInTheDocument();
    expect(within(panel).queryByTestId('revalidate-launch')).toBeNull();
  });

  it('judges each section: verdict counts, evidence and the proposed correction', async () => {
    vi.mocked(projectsApi.revalidationPlan).mockResolvedValue({
      sources_state: 'needs_reverification',
      agent_steps: [4],
      reanchor_count: 0,
      human_count: 0,
      relevance: {
        sections: 6, anchors_intact: 3, outdated: 1, no_longer_relevant: 1, unanchored: 1,
        untouched_stale: 2, missing_references: 2, prose_missing: 1, anchored_intact_pct: 60,
      },
      unanchored: [{
        doc: 'docs/glossary.md', heading: 'Intro', verdict: 'unanchored', evidence: [],
        corrections: [{ kind: 'check_or_anchor', reference: '', proposal: 'Check it, then anchor it.' }],
        untouched_since_code_changed: false,
      }],
      unanchored_docs: [
        { doc: 'docs/glossary.md', reviewed_at: '2026-10-16T09:30:00+00:00', owned: true },
        { doc: 'docs/notes.md', reviewed_at: null, owned: true },
        { doc: 'docs/architecture/design.md', reviewed_at: null, owned: false },
      ],
      sections: [{
        doc: 'docs/coding-rules.md', step: 4, action: 'agent', doc_missing: false,
        reanchor: [], broken: [], human: [],
        relevance: [{
          doc: 'docs/coding-rules.md', heading: 'Engine', verdict: 'no_longer_relevant',
          evidence: [{ kind: 'path_reference', reference: 'src/engine.rs', state: 'file_missing' }],
          corrections: [{
            kind: 'remove_section', reference: '', proposal: 'Everything this section points at is gone.',
          }],
          untouched_since_code_changed: true,
        }],
      }],
    });
    setup();
    const panel = await openPanel();

    const counts = await within(panel).findByTestId('revalidate-relevance-counts');
    expect(counts).toHaveTextContent('audit.revalidate.relevance 3 5 1 1 1 2');
    const section = within(panel).getByTestId('revalidate-relevance-section');
    expect(section).toHaveAttribute('data-verdict', 'no_longer_relevant');
    expect(section).toHaveTextContent('Engine');
    expect(section).toHaveTextContent('src/engine.rs');
    expect(section).toHaveTextContent('Everything this section points at is gone.');
    expect(within(section).getByTestId('revalidate-untouched')).toBeInTheDocument();
    expect(within(panel).queryByTestId('revalidate-prose-changed')).toBeNull();
    expect(within(panel).getByTestId('revalidate-launch')).toBeEnabled();

    // The sections no anchor measures are listed, never called true.
    const unanchored = within(panel).getByTestId('revalidate-unanchored');
    expect(unanchored).toHaveTextContent('audit.revalidate.relevance.unanchored.title 1');
    const row = within(unanchored).getByTestId('revalidate-unanchored-section');
    expect(row).toHaveTextContent('docs/glossary.md');
    expect(row).toHaveTextContent('Intro');

    // KT-1292 — each doc made only of such sections says when it was last re-read.
    const [reread, pending, unowned] = within(unanchored).getAllByTestId('revalidate-unanchored-doc');
    expect(reread).toHaveTextContent('docs/glossary.md');
    expect(reread).toHaveTextContent(
      `audit.revalidate.relevance.unanchored.reread ${new Date('2026-10-16T09:30:00+00:00').toLocaleDateString()}`,
    );
    expect(pending).toHaveTextContent('docs/notes.md');
    expect(pending).toHaveTextContent('audit.revalidate.relevance.unanchored.pending');
    // A doc no audit step owns is never queued: it is not called "pending".
    expect(unowned).toHaveTextContent('docs/architecture/design.md');
    expect(unowned).toHaveTextContent('audit.revalidate.relevance.unanchored.unowned');
    expect(unowned).not.toHaveTextContent('unanchored.pending');
  });

  it('allows a targeted prose check while citation fingerprints are unknown', async () => {
    vi.mocked(projectsApi.revalidationPlan).mockResolvedValue({
      sources_state: 'unknown',
      agent_steps: [4],
      reanchor_count: 0,
      human_count: 0,
      relevance: NO_RELEVANCE,
      unanchored: [], unanchored_docs: [],
      sections: [{
        doc: 'docs/coding-rules.md', step: 4, action: 'agent', doc_missing: false,
        reanchor: [], broken: [], human: [], relevance: [],
      }],
    });
    setup();
    const panel = await openPanel();

    expect(await within(panel).findByTestId('revalidate-unknown'))
      .toHaveTextContent('audit.revalidate.unknownWithAgentWork');
    expect(within(panel).getByTestId('doc-state-block')).toHaveTextContent('docState.unknown');
    expect(within(panel).getByTestId('revalidate-prose-changed'))
      .toHaveTextContent('audit.revalidate.proseChanged');
    expect(within(panel).getByTestId('revalidate-launch')).toBeEnabled();

    await launchHeld();
    expect(projectsApi.revalidateAuditStream).toHaveBeenCalledWith(
      'p-reval',
      { agent: 'ClaudeCode', tier: 'reasoning', connection_id: undefined },
      expect.any(Object),
      expect.any(AbortSignal),
    );
  });
});

async function launchResume(panel: HTMLElement) {
  vi.mocked(projectsApi.revalidateAuditStream).mockImplementationOnce(async () => {});
  await act(async () => {
    fireEvent.click(within(panel).getByTestId('revalidate-resume'));
    await Promise.resolve();
  });
}

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true,
  path: '/usr/bin/claude', version: '1.0.0', latest_version: null, origin: 'host',
  install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

describe('ProjectCard — Revalider l’audit', () => {
  it('offers the action on a validated project card', async () => {
    vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
    vi.mocked(projectsApi.auditResumable).mockResolvedValue(null);
    render(<ProjectCard {...({
      project: PROJECT, isOpen: true, onToggleOpen: () => {}, discussions: [],
      driftStatus: undefined, agents: [AGENT], allSkills: [], mcpConfigs: [],
      workflows: [], configLanguage: 'fr',
      toast: vi.fn(), onNavigate: vi.fn(), onSetDiscPrefill: () => {},
      onAutoRunDiscussion: vi.fn(), onOpenDiscussion: vi.fn(),
      onRefetch: vi.fn(), onRefetchDiscussions: vi.fn(), onRefetchSkills: () => {},
      onRefetchDrift: vi.fn(),
    } as unknown as React.ComponentProps<typeof ProjectCard>)} />);
    expect(await screen.findByTestId('project-revalidate')).toBeInTheDocument();
  });

  it('the panel takes focus, keeps it, closes on Escape and returns focus to its trigger (KT-1072)', async () => {
    setup();
    const trigger = screen.getByTestId('project-revalidate');
    await userEvent.click(trigger);
    const panel = await screen.findByTestId('revalidate-panel');
    await within(panel).findByTestId('revalidate-launch');
    await expectFocusKeptIn(panel);
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByTestId('revalidate-panel')).toBeNull();
    expect(trigger).toHaveFocus();
  });
});
