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
import { act, render, screen, cleanup, waitFor } from '@testing-library/react';

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
  runsApi: { get: (...a: unknown[]) => runsGet(...a) },
  // A visible, still-running card opens a WebSocket; both are read by the
  // shared hook before anything else happens.
  getApiBase: () => '',
  getAuthToken: () => null,
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

  it('states no elapsed time and no freshness on a media bubble', () => {
    // What a reader got on an image was "Duration unavailable / Rehydrated
    // from the server" — two labels that answer nothing about a picture. The
    // duration that means something for a media is the one of the file it
    // produced, and that one lives in the media row.
    const model = sharedRunStatusCardModel(
      run({ schema_version: 1, modality: 'image', phase: 'persisting', width: 1024, height: 1024 }),
    );
    render(<RunStatusCard model={model} />);
    expect(screen.queryByText('run.durationUnavailable')).toBeNull();
    expect(screen.queryByText('run.freshness.live')).toBeNull();
  });

  it('leaves every other kind of run with the meta line it has today', () => {
    // DoD #9: the folding, the duration rule and the price all target media
    // only — a workflow card must render exactly as before.
    const model = sharedRunStatusCardModel({ ...run(null), kind: 'workflow' } as SharedRun);
    render(<RunStatusCard model={model} />);
    expect(screen.getByText('run.durationUnavailable')).toBeInTheDocument();
    expect(screen.getByText('run.freshness.live')).toBeInTheDocument();
  });

  it('times the produced clip, never the produced picture', () => {
    // `media_duration_ms` on an image would be the generation time wearing the
    // clothes of a playback length.
    const model = sharedRunStatusCardModel(
      run({ schema_version: 1, modality: 'image', phase: 'persisting', media_duration_ms: 8000 }),
    );
    render(<RunStatusCard model={model} />);
    expect(screen.getByTestId('run-status-card-media').textContent).not.toContain('8s');
  });

  it('folds the raw projection away on a media run, and only there', () => {
    const media = sharedRunStatusCardModel(run(VIDEO_RESULT));
    const { unmount } = render(<RunStatusCard model={media} />);
    const fold = screen.getByTestId('run-status-card-result-fold');
    // Present and readable on demand, but closed: the JSON is a debugging
    // detail, not the answer the bubble exists to give.
    expect(fold.tagName).toBe('DETAILS');
    expect((fold as HTMLDetailsElement).open).toBe(false);
    expect(fold.textContent).toContain('run.details');
    expect(fold.querySelector('pre')?.textContent).toContain('864');
    unmount();

    const workflow = sharedRunStatusCardModel({ ...run(VIDEO_RESULT), kind: 'workflow' } as SharedRun);
    render(<RunStatusCard model={workflow} />);
    expect(screen.queryByTestId('run-status-card-result-fold')).toBeNull();
    expect(document.querySelector('.run-status-card-result')?.textContent).toContain('864');
  });

  it('closes the card on the price, outside the media row', () => {
    const model = sharedRunStatusCardModel(run(VIDEO_RESULT));
    render(<RunStatusCard model={model} />);
    const cost = screen.getByTestId('run-status-card-media-cost');
    // Bottom right of the bubble: the price is what the eye looks for once the
    // media is there.
    expect(cost.closest('[data-testid="run-status-card-media"]')).toBeNull();
    expect(screen.getByTestId('run-status-card').lastElementChild).toBe(cost);
  });

  it('keeps the run link suppressed across a rehydration', async () => {
    // The bubble owns a better destination (the asset itself). Self-hydration
    // rebuilds the model from the server, href included, so a suppression that
    // only applied to the initial props would come back a second later.
    const rehydrated = run(VIDEO_RESULT);
    runsGet.mockResolvedValue(rehydrated);
    render(<RunStatusCard model={sharedRunStatusCardModel(rehydrated)} runId="job-1" hideRunLink />);

    act(() => MockIntersectionObserver.instances[0].setIntersecting(true));
    await waitFor(() => expect(runsGet).toHaveBeenCalledWith('job-1'));
    // The server projection does carry an href — it is the suppression that
    // has to survive, not the absence of a destination.
    expect(sharedRunStatusCardModel(rehydrated).href).toBeTruthy();
    expect(screen.queryByRole('link')).toBeNull();
  });

  it('shows BYOK instead of a misleading zero', () => {
    const model = sharedRunStatusCardModel(
      run({ schema_version: 1, modality: 'video', phase: 'persisting', cost_usd: 0, is_byok: true }),
    );
    render(<RunStatusCard model={model} />);
    expect(screen.getByTestId('run-status-card-media-cost').textContent).toContain('run.media.byok');
  });

});
