/**
 * KT-540 — media adapter on the shared RunStatusCard.
 *
 * One kind for both modalities, read from `result.modality`. The properties
 * worth pinning are the ones that would silently lie: a fabricated progress
 * bar, a zero cost on a job that has not been billed, or a geometry taken from
 * the request instead of the produced file.
 *
 * Targeted WebSocket rehydration is covered in `RunStatusCard.test.tsx`, which
 * owns the real WebSocket harness — the scoping code is shared by every kind,
 * so duplicating it here with a mocked hook would test the mock, not the card.
 */
import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';

// The card suspends loading while off-screen, and jsdom never fires a real
// IntersectionObserver — so visibility is flipped deterministically here,
// exactly as the existing RunStatusCard suite does.
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

const runsGet = vi.fn();

vi.mock('../../lib/api', () => ({
  runs: { get: (...a: unknown[]) => runsGet(...a) },
}));

const { RunStatusCard } = await import('../RunStatusCard');
import { sharedRunStatusCardModel } from '../../lib/runStatusCardModel';
import type { SharedRun } from '../../types/generated';


function run(result: unknown, id = 'job-1'): SharedRun {
  return {
    id,
    kind: 'media',
    source_id: 'conn-1',
    project_id: null,
    discussion_id: 'disc-1',
    status: 'running',
    started_at: null,
    finished_at: null,
    duration_ms: null,
    result,
    diagnostic: null,
    created_at: '2026-09-01T00:00:00Z',
    updated_at: '2026-09-01T00:00:00Z',
  } as SharedRun;
}

const VIDEO_RESULT = {
  schema_version: 1,
  modality: 'video',
  phase: 'polling',
  width: 864,
  height: 496,
  media_duration_ms: 5042,
  cost_usd: 0.0708932,
  is_byok: false,
};

let originalObserver: typeof IntersectionObserver | undefined;

beforeEach(() => {
  MockIntersectionObserver.instances = [];
  originalObserver = globalThis.IntersectionObserver;
  (globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver =
    MockIntersectionObserver;
});

afterEach(() => {
  cleanup();
  runsGet.mockReset();
  if (originalObserver) globalThis.IntersectionObserver = originalObserver;
});

describe('RunStatusCard — media', () => {
  it('renders a video run with the geometry of the produced file', () => {
    const model = sharedRunStatusCardModel(run(VIDEO_RESULT));
    render(<RunStatusCard model={model} />);

    const media = screen.getByTestId('run-status-card-media');
    expect(media.getAttribute('data-modality')).toBe('video');
    // 864x496 is what a "480p 16:9" request actually produced.
    expect(screen.getByTestId('run-status-card-media-size').textContent).toBe('864×496');
    expect(media.textContent).toContain('5s');
    expect(screen.getByTestId('run-status-card-media-cost').textContent).toContain('0.0709');
  });

  it('renders an image run through the same single kind', () => {
    const model = sharedRunStatusCardModel(
      run({ schema_version: 1, modality: 'image', phase: 'persisting', width: 1024, height: 1024 }),
    );
    render(<RunStatusCard model={model} />);
    // Same card, same kind: only the modality differs.
    expect(screen.getByTestId('run-status-card').getAttribute('data-kind')).toBe('media');
    expect(screen.getByTestId('run-status-card-media').getAttribute('data-modality')).toBe('image');
    expect(screen.getByTestId('run-status-card-media-size').textContent).toBe('1024×1024');
  });

  it('never fabricates a progress bar', () => {
    // The provider does not measure progress, so the backend omits it. A bar
    // here would look authoritative while being fiction.
    const model = sharedRunStatusCardModel(run(VIDEO_RESULT));
    expect(model.progress).toBeNull();
    render(<RunStatusCard model={model} />);
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  it('shows no cost and no size while nothing has been measured', () => {
    const model = sharedRunStatusCardModel(
      run({ schema_version: 1, modality: 'video', phase: 'submitting' }),
    );
    render(<RunStatusCard model={model} />);
    expect(screen.getByTestId('run-status-card-media')).toBeTruthy();
    // Absent, not zero: nothing has been billed or produced yet.
    expect(screen.queryByTestId('run-status-card-media-cost')).toBeNull();
    expect(screen.queryByTestId('run-status-card-media-size')).toBeNull();
  });

  it('declines a projection whose schema version it does not understand', () => {
    // The reader advertises version 1; interpreting another one would risk
    // misreporting a real generation whose fields changed meaning.
    for (const version of [undefined, 0, 2, '1', null]) {
      cleanup();
      const model = sharedRunStatusCardModel(
        run({ schema_version: version, modality: 'video', phase: 'polling', width: 864, height: 496 }),
      );
      render(<RunStatusCard model={model} />);
      expect(screen.getByTestId('run-status-card')).toBeTruthy();
      expect(screen.queryByTestId('run-status-card-media')).toBeNull();
    }
  });

  it('degrades to no media details on an absent or malformed result', () => {
    for (const bad of [null, undefined, 42, 'nope', {}, { modality: 'audio' }, { schema_version: 1 }]) {
      cleanup();
      const model = sharedRunStatusCardModel(run(bad));
      render(<RunStatusCard model={model} />);
      // The card still renders; only the media block is withheld.
      expect(screen.getByTestId('run-status-card')).toBeTruthy();
      expect(screen.queryByTestId('run-status-card-media')).toBeNull();
    }
  });

  it('shows BYOK instead of a misleading zero', () => {
    const model = sharedRunStatusCardModel(
      run({ schema_version: 1, modality: 'video', phase: 'persisting', cost_usd: 0, is_byok: true }),
    );
    render(<RunStatusCard model={model} />);
    expect(screen.getByTestId('run-status-card-media-cost').textContent).toContain('run.media.byok');
  });

});
