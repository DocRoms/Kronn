/**
 * KT-540 — the in-discussion progress placeholder.
 *
 * There is deliberately no second live system: a media job publishes a shared
 * run the moment it is QUEUED, and this panel already lists a discussion's runs
 * without filtering by kind. So the placeholder is the shared card, and what
 * needs proving is that a media run really reaches it — and that it says
 * "queued" rather than pretending to measure progress nobody measured.
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
  it('lists a queued media generation without any kind filter', async () => {
    runsList.mockResolvedValue([mediaRun()]);
    render(<DiscussionAttachedRuns discussionId="disc-1" />);

    await waitFor(() => expect(runsList).toHaveBeenCalled());
    // The panel must not ask for a subset: media appears because nothing
    // excludes it, which is why no second live system was needed.
    const params = runsList.mock.calls[0][0] as Record<string, unknown>;
    expect(params.kind).toBeUndefined();
    expect(params.discussionId).toBe('disc-1');

    await waitFor(() => expect(screen.getByTestId('run-status-card')).toBeTruthy());
    expect(screen.getByTestId('run-status-card').getAttribute('data-kind')).toBe('media');
  });

  it('shows a queued generation as queued, not as measured progress', async () => {
    runsList.mockResolvedValue([mediaRun()]);
    render(<DiscussionAttachedRuns discussionId="disc-1" />);
    await waitFor(() => expect(screen.getByTestId('run-status-card')).toBeTruthy());

    expect(screen.getByTestId('run-status-card').getAttribute('data-status')).toBe('queued');
    // A ~100 s generation must be visible while it waits, but nothing may
    // pretend to know how far along it is.
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  it('keeps media alongside other kinds in the same panel', async () => {
    runsList.mockResolvedValue([
      mediaRun(),
      mediaRun({ id: 'wf-1', kind: 'workflow', status: 'running', result: null }),
    ]);
    render(<DiscussionAttachedRuns discussionId="disc-1" />);

    await waitFor(() => expect(screen.getAllByTestId('run-status-card')).toHaveLength(2));
    const kinds = screen
      .getAllByTestId('run-status-card')
      .map(card => card.getAttribute('data-kind'));
    expect(kinds).toContain('media');
    expect(kinds).toContain('workflow');
  });
});
