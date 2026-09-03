import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePageDetail, LivePageWorkflowBinding, WorkflowRun } from '../../types/generated';

const linkRelay = vi.hoisted(() => ({ connect: vi.fn(), dispose: vi.fn() }));
const actionRelay = vi.hoisted(() => ({
  connect: vi.fn(), dispose: vi.fn(),
  handle: null as null | ((action: string, payload: unknown) => Promise<unknown>),
}));

function makeDetail(overrides: Partial<LivePageDetail> = {}): LivePageDetail {
  return {
    id: 'page-1', project_id: null, title: 'Production health', slug: 'production-health',
    current_revision_id: 'rev-1', data_revision: 2,
    created_at: '2026-08-26T10:00:00Z', updated_at: '2026-08-26T10:00:00Z',
    last_published_at: '2026-08-26T10:00:00Z', pinned: false, archived: false,
    revision: {
      id: 'rev-1', page_id: 'page-1', revision: 1,
      html: '<main><h1>Production health</h1></main>',
      created_by_agent: 'Ollama', created_at: '2026-08-26T10:00:00Z',
    },
    datasets: [],
    ...overrides,
  };
}

const detail = makeDetail();

vi.mock('../../lib/api', () => ({
  pages: { get: vi.fn(), bindings: vi.fn(), decideGate: vi.fn(), triggerWorkflow: vi.fn() },
  workflows: { listRuns: vi.fn(), getRun: vi.fn() },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: string[]) => args.length ? `${key}:${args.join(',')}` : key }),
}));
vi.mock('../../lib/live-page-sandbox', async importOriginal => ({
  ...await importOriginal<Record<string, unknown>>(),
  createLivePageOpenLinkRelay: vi.fn(() => linkRelay),
  createLivePageActionRelay: vi.fn((_channel: string, handle: (a: string, p: unknown) => Promise<unknown>) => {
    actionRelay.handle = handle;
    return actionRelay;
  }),
}));

import { pages as pagesApi, workflows as workflowsApi } from '../../lib/api';
import { StandaloneLivePage } from '../StandaloneLivePage';

function setHidden(hidden: boolean): void {
  Object.defineProperty(document, 'visibilityState', { value: hidden ? 'hidden' : 'visible', configurable: true });
  document.dispatchEvent(new Event('visibilitychange'));
}

async function advance(ms: number): Promise<void> {
  await act(async () => { await vi.advanceTimersByTimeAsync(ms); });
}

beforeEach(() => {
  linkRelay.connect.mockClear();
  linkRelay.dispose.mockClear();
  vi.mocked(pagesApi.get).mockReset();
  vi.mocked(pagesApi.get).mockResolvedValue(detail);
  vi.mocked(pagesApi.bindings).mockReset();
  vi.mocked(pagesApi.bindings).mockResolvedValue([]);
  vi.mocked(workflowsApi.listRuns).mockReset();
  vi.mocked(workflowsApi.getRun).mockReset();
});

afterEach(() => {
  vi.useRealTimers();
  setHidden(false);
});

describe('StandaloneLivePage', () => {
  it('renders the requested Page full-screen inside the opaque sandbox', async () => {
    const previousTitle = document.title;
    const view = render(<StandaloneLivePage pageId="page-1" />);

    expect(screen.getByRole('status')).toHaveTextContent('pages.standaloneLoading');
    const frame = await screen.findByTestId('standalone-live-page-frame');
    expect(pagesApi.get).toHaveBeenCalledWith('page-1');
    expect(frame).toHaveAttribute('title', 'Production health');
    expect(frame).toHaveAttribute('sandbox', 'allow-scripts');
    expect(frame).not.toHaveAttribute('allow-same-origin');
    expect(frame.getAttribute('srcdoc')).toContain("connect-src 'none'");
    expect(frame.getAttribute('srcdoc')).toContain('<h1>Production health</h1>');
    await waitFor(() => expect(linkRelay.connect).toHaveBeenCalledWith(
      (frame as HTMLIFrameElement).contentWindow,
    ));
    await waitFor(() => expect(document.title).toBe('Production health · Kronn'));

    view.unmount();
    expect(document.title).toBe(previousTitle);
  });

  it('live-refreshes on the active cadence and re-renders new published data', async () => {
    vi.useFakeTimers();
    vi.mocked(pagesApi.get)
      .mockResolvedValueOnce(makeDetail({ data_revision: 2 }))
      .mockResolvedValue(makeDetail({
        data_revision: 3,
        revision: {
          id: 'rev-2', page_id: 'page-1', revision: 2,
          html: '<main><h1>Updated health</h1></main>',
          created_by_agent: 'Ollama', created_at: '2026-08-26T10:05:00Z',
        },
      }));

    render(<StandaloneLivePage pageId="page-1" />);
    await advance(0); // flush the immediate first fetch
    expect(pagesApi.get).toHaveBeenCalledTimes(1);
    let frame = screen.getByTestId('standalone-live-page-frame');
    expect(frame.getAttribute('srcdoc')).toContain('<h1>Production health</h1>');

    await advance(4_000); // next poll at the active interval
    expect(pagesApi.get).toHaveBeenCalledTimes(2);
    frame = screen.getByTestId('standalone-live-page-frame');
    expect(frame.getAttribute('srcdoc')).toContain('<h1>Updated health</h1>');
  });

  it('backs off to the idle cadence once data stops changing', async () => {
    vi.useFakeTimers();
    vi.mocked(pagesApi.get).mockResolvedValue(makeDetail({ data_revision: 2 })); // never changes

    render(<StandaloneLivePage pageId="page-1" />);
    await advance(0);
    expect(pagesApi.get).toHaveBeenCalledTimes(1); // initial (counts as changed: null → 2)

    await advance(4_000); expect(pagesApi.get).toHaveBeenCalledTimes(2); // quiet=1
    await advance(4_000); expect(pagesApi.get).toHaveBeenCalledTimes(3); // quiet=2
    await advance(4_000); expect(pagesApi.get).toHaveBeenCalledTimes(4); // quiet=3 → next is idle

    await advance(4_000); expect(pagesApi.get).toHaveBeenCalledTimes(4); // still waiting the idle heartbeat
    await advance(30_000); expect(pagesApi.get).toHaveBeenCalledTimes(5); // idle heartbeat fires
  });

  it('re-pushes into the iframe only when data changes (idle refresh keeps transient UI)', async () => {
    vi.useFakeTimers();
    vi.mocked(pagesApi.get).mockResolvedValue(makeDetail({ data_revision: 5 }));

    render(<StandaloneLivePage pageId="page-1" />);
    await advance(0);
    const frame = screen.getByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const post = vi.spyOn(frame.contentWindow as Window, 'postMessage');

    // Idle polls with an unchanged data_revision must NOT re-post — otherwise every
    // Page rebuilds its DOM each tick and loses in-progress input (forms, textareas).
    await advance(4_000); await advance(4_000); await advance(4_000); await advance(30_000);
    expect(post).not.toHaveBeenCalled();

    // A genuine change re-posts exactly once.
    vi.mocked(pagesApi.get).mockResolvedValue(makeDetail({ data_revision: 6 }));
    await advance(30_000);
    expect(post).toHaveBeenCalledTimes(1);
  });

  it('pauses polling while the tab is hidden and refreshes on return', async () => {
    vi.useFakeTimers();
    vi.mocked(pagesApi.get).mockResolvedValue(makeDetail({ data_revision: 2 }));

    render(<StandaloneLivePage pageId="page-1" />);
    await advance(0);
    expect(pagesApi.get).toHaveBeenCalledTimes(1);

    setHidden(true);
    await advance(60_000); // no fetches while hidden
    expect(pagesApi.get).toHaveBeenCalledTimes(1);

    setHidden(false); // returning fires an immediate refresh
    await advance(0);
    expect(pagesApi.get).toHaveBeenCalledTimes(2);
  });

  it('keeps the last good render when a poll fails transiently', async () => {
    vi.useFakeTimers();
    vi.mocked(pagesApi.get)
      .mockResolvedValueOnce(makeDetail({ data_revision: 2 }))
      .mockRejectedValue(new Error('network blip'));

    render(<StandaloneLivePage pageId="page-1" />);
    await advance(0);
    expect(screen.getByTestId('standalone-live-page-frame').getAttribute('srcdoc'))
      .toContain('<h1>Production health</h1>');

    await advance(4_000); // this poll rejects
    expect(pagesApi.get).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole('alert')).toBeNull(); // no error surfaced
    expect(screen.getByTestId('standalone-live-page-frame').getAttribute('srcdoc'))
      .toContain('<h1>Production health</h1>'); // last good render stays
  });

  it('mirrors a bound workflow run into a pipeline dataset', async () => {
    vi.useFakeTimers();
    const binding = {
      id: 'b-1', page_id: 'page-1', workflow_id: 'wf-1', dataset: 'pipeline',
      run_selector: 'latest', allowed_gate_steps: [],
      phase_map: [{ name: 'P', steps: [{ step: 'a' }] }],
      meta_map: {},
      created_at: '2026-08-26T10:00:00Z', updated_at: '2026-08-26T10:00:00Z',
    } as unknown as LivePageWorkflowBinding;
    const wfRun = {
      id: 'run-1', workflow_id: 'wf-1', status: 'Running', trigger_context: null,
      step_results: [{ step_name: 'a', status: 'Success', output: 'ok', tokens_used: 0, duration_ms: 1000 }],
      tokens_used: 0, workspace_path: null, started_at: '2026-08-28T12:00:00Z', finished_at: null,
    } as unknown as WorkflowRun;
    vi.mocked(pagesApi.bindings).mockResolvedValue([binding]);
    vi.mocked(workflowsApi.listRuns).mockResolvedValue([wfRun]);
    vi.mocked(workflowsApi.getRun).mockResolvedValue(wfRun);

    render(<StandaloneLivePage pageId="page-1" />);
    await advance(0); // flush the first tick's binding → run → reshape chain

    expect(pagesApi.bindings).toHaveBeenCalledWith('page-1');
    expect(workflowsApi.listRuns).toHaveBeenCalledWith('wf-1', 5);
    expect(workflowsApi.getRun).toHaveBeenCalledWith('wf-1', 'run-1');
    // A non-terminal mirrored run keeps the fast cadence: it re-polls at 4s.
    await advance(4_000);
    expect(vi.mocked(workflowsApi.getRun).mock.calls.length).toBeGreaterThan(1);
  });

  it('brokers a gate decision through pages.decideGate', async () => {
    vi.mocked(pagesApi.decideGate).mockResolvedValue({ run_id: 'r1', new_status: 'Running' } as never);
    render(<StandaloneLivePage pageId="page-1" />);
    await screen.findByTestId('standalone-live-page-frame');
    expect(typeof actionRelay.handle).toBe('function');

    const result = await actionRelay.handle!('gate.decide', {
      dataset: 'pipeline', run_id: 'r1', decision: 'approve', comment: null,
    });

    expect(pagesApi.decideGate).toHaveBeenCalledWith('page-1', {
      dataset: 'pipeline', run_id: 'r1', decision: 'approve', comment: null,
    });
    expect(result).toEqual({ run_id: 'r1', new_status: 'Running' });

    // An unknown action is rejected, and a decision missing identifiers too.
    await expect(actionRelay.handle!('bogus', {})).rejects.toThrow(/Unknown action/);
    await expect(actionRelay.handle!('gate.decide', { decision: 'approve' })).rejects.toThrow(/Missing/);
  });

  it('brokers a workflow trigger through pages.triggerWorkflow', async () => {
    vi.mocked(pagesApi.triggerWorkflow).mockResolvedValue({ run_id: 'run-9' } as never);
    render(<StandaloneLivePage pageId="page-1" />);
    await screen.findByTestId('standalone-live-page-frame');

    const result = await actionRelay.handle!('workflow.trigger', {
      dataset: 'ticket_trigger', variables: { besoin: 'Refonte player' },
    });

    expect(pagesApi.triggerWorkflow).toHaveBeenCalledWith('page-1', {
      dataset: 'ticket_trigger', variables: { besoin: 'Refonte player' },
    });
    expect(result).toEqual({ run_id: 'run-9' });

    // A trigger without a dataset (which binding?) is rejected before any call.
    await expect(actionRelay.handle!('workflow.trigger', {})).rejects.toThrow(/Missing dataset/);
  });

  it('surfaces an error when the very first load fails', async () => {
    vi.mocked(pagesApi.get).mockReset();
    vi.mocked(pagesApi.get).mockRejectedValue(new Error('boom'));

    render(<StandaloneLivePage pageId="page-1" />);
    expect(await screen.findByRole('alert')).toHaveTextContent('pages.standaloneLoadError');
  });
});
