// « Revalider l'audit » (KT-1168): the panel lists what the deterministic
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

const PLAN: RevalidationPlan = {
  sources_state: 'needs_reverification',
  agent_steps: [4],
  reanchor_count: 1,
  human_count: 1,
  sections: [
    {
      doc: 'docs/coding-rules.md', step: 4, action: 'agent', doc_missing: false,
      reanchor: [{ cite: 'src/a.rs:2', file: 'src/a.rs', line: 2, end_line: 2, state: 'moved', current_line: 5 }],
      broken: [{ cite: 'src/a.rs:9', file: 'src/a.rs', line: 9, end_line: 9, state: 'changed', current_line: null }],
      human: [],
    },
    {
      doc: 'docs/glossary.md', step: 2, action: 'human', doc_missing: false,
      reanchor: [], broken: [],
      human: [{
        section: 'owner-notes',
        citation: { cite: 'src/old.rs:3', file: 'src/old.rs', line: 3, end_line: 3, state: 'file_missing', current_line: null },
      }],
    },
  ],
};

const CLEAN: RevalidationPlan = {
  sources_state: 'clean', sections: [], agent_steps: [], reanchor_count: 0, human_count: 0,
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
