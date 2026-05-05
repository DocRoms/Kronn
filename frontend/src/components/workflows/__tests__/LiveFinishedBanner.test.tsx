// Regression guards for the live-run finished banner colour logic.
//
// The banner is what the operator sees the instant a workflow's SSE
// stream emits its terminal `run_done` event. The 0.7.0 fix introduced
// a third colour state for `WaitingApproval` runs — before, every
// non-Success run painted red, so a paused-on-Gate run looked like a
// failure. The logic is small but load-bearing UX, hence these tests.

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { LiveFinishedBanner } from '../WorkflowDetail';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

describe('LiveFinishedBanner — colour mapping', () => {
  it('paints green + Check + "wf.runDone — Success" for Success runs', () => {
    const { container } = render(
      <LiveFinishedBanner status="Success" stepsExecuted={3} t={t} />
    );
    const banner = container.querySelector('.wf-live-finished');
    expect(banner).not.toBeNull();
    expect(banner!.getAttribute('data-status')).toBe('success');
    expect(screen.getByText(/wf\.runDone:Success/)).toBeInTheDocument();
    expect(screen.getByText(/wf\.stepsExecuted:3/)).toBeInTheDocument();
  });

  it('paints amber + Hand + "wf.runWaiting" for WaitingApproval runs (NOT red)', () => {
    // Critical regression: pre-0.7.0 this was red + "Run terminé".
    const { container } = render(
      <LiveFinishedBanner status="WaitingApproval" stepsExecuted={2} t={t} />
    );
    const banner = container.querySelector('.wf-live-finished');
    expect(banner!.getAttribute('data-status')).toBe('waiting');
    expect(screen.getByText('wf.runWaiting')).toBeInTheDocument();
    // Must NOT show the generic "run done" label, or the Success/Failed
    // pill — those would imply the run is over rather than paused.
    expect(screen.queryByText(/wf\.runDone/)).not.toBeInTheDocument();
  });

  it('paints red + X for Failed runs', () => {
    const { container } = render(
      <LiveFinishedBanner status="Failed" stepsExecuted={1} t={t} />
    );
    expect(container.querySelector('.wf-live-finished')!.getAttribute('data-status')).toBe('failed');
    expect(screen.getByText(/wf\.runDone:Failed/)).toBeInTheDocument();
  });

  it('paints red for Cancelled and StoppedByGuard (default failed bucket)', () => {
    const cancelled = render(
      <LiveFinishedBanner status="Cancelled" stepsExecuted={0} t={t} />
    );
    expect(cancelled.container.querySelector('.wf-live-finished')!.getAttribute('data-status')).toBe('failed');
    cancelled.unmount();

    const guard = render(
      <LiveFinishedBanner status="StoppedByGuard" stepsExecuted={0} t={t} />
    );
    expect(guard.container.querySelector('.wf-live-finished')!.getAttribute('data-status')).toBe('failed');
  });

  it('handles a null status defensively (renders the failed bucket)', () => {
    // Should not crash even on degenerate state.
    const { container } = render(
      <LiveFinishedBanner status={null} stepsExecuted={0} t={t} />
    );
    expect(container.querySelector('.wf-live-finished')!.getAttribute('data-status')).toBe('failed');
  });
});
