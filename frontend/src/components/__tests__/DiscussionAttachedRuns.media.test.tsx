/**
 * KT-549 — media runs left this panel.
 *
 * KT-540 first showed a media generation here (no kind filter, since nothing
 * excluded it). KT-549 gave every launch its own anchor message and a live
 * placeholder rendered inline in the transcript at that exact position
 * (`InlineMediaJob`) — so a media run showing up here TOO would duplicate the
 * same status in two places on screen. This panel now filters it out
 * client-side; other kinds are unaffected.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';

const runsList = vi.fn();
const runsGet = vi.fn();

vi.mock('../../lib/api', () => ({
  runs: { list: (...a: unknown[]) => runsList(...a), get: (...a: unknown[]) => runsGet(...a) },
  runsApi: { list: (...a: unknown[]) => runsList(...a), get: (...a: unknown[]) => runsGet(...a) },
  getApiBase: () => 'http://localhost',
  getAuthToken: () => null,
}));

class MockIntersectionObserver {
  static instances: MockIntersectionObserver[] = [];
  callback: (entries: Array<{ isIntersecting: boolean }>) => void;
  constructor(callback: (entries: Array<{ isIntersecting: boolean }>) => void) {
    this.callback = callback;
    MockIntersectionObserver.instances.push(this);
  }
  observe() {}
  disconnect() {}
  setIntersecting(value: boolean) { this.callback([{ isIntersecting: value }]); }
}

const { DiscussionAttachedRuns } = await import('../DiscussionAttachedRuns');
import type { SharedRun } from '../../types/generated';

function mediaRun(overrides: Partial<SharedRun> = {}): SharedRun {
  return {
    id: 'media-run-1',
    kind: 'media',
    source_id: 'conn-1',
    project_id: null,
    discussion_id: 'disc-1',
    status: 'queued',
    started_at: null,
    finished_at: null,
    duration_ms: null,
    result: { schema_version: 1, modality: 'video', phase: 'submitting' },
    diagnostic: null,
    created_at: '2026-09-01T00:00:00Z',
    updated_at: '2026-09-01T00:00:00Z',
    ...overrides,
  } as SharedRun;
}

let originalObserver: typeof IntersectionObserver | undefined;

beforeEach(() => {
  MockIntersectionObserver.instances = [];
  originalObserver = globalThis.IntersectionObserver;
  (globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver =
    MockIntersectionObserver;
  runsList.mockReset();
  runsGet.mockReset();
});

afterEach(() => {
  cleanup();
  if (originalObserver) globalThis.IntersectionObserver = originalObserver;
});

describe('DiscussionAttachedRuns — media', () => {
  it('excludes a queued media generation: it has its own inline placeholder now', async () => {
    runsList.mockResolvedValue([mediaRun()]);
    render(<DiscussionAttachedRuns discussionId="disc-1" />);

    await waitFor(() => expect(runsList).toHaveBeenCalled());
    // Still no server-side kind filter — the exclusion happens client-side so
    // the same list endpoint keeps serving every other consumer unfiltered.
    const params = runsList.mock.calls[0][0] as Record<string, unknown>;
    expect(params.kind).toBeUndefined();
    expect(params.discussionId).toBe('disc-1');

    // Nothing to show: a media-only result renders no card and no panel.
    expect(screen.queryByTestId('run-status-card')).toBeNull();
    expect(screen.queryByTestId('disc-attached-runs')).toBeNull();
  });

  it('keeps other kinds while excluding media from the same panel', async () => {
    runsList.mockResolvedValue([
      mediaRun(),
      mediaRun({ id: 'wf-1', kind: 'workflow', status: 'running', result: null }),
    ]);
    render(<DiscussionAttachedRuns discussionId="disc-1" />);

    await waitFor(() => expect(screen.getAllByTestId('run-status-card')).toHaveLength(1));
    const kinds = screen
      .getAllByTestId('run-status-card')
      .map(card => card.getAttribute('data-kind'));
    expect(kinds).not.toContain('media');
    expect(kinds).toContain('workflow');
  });
});
