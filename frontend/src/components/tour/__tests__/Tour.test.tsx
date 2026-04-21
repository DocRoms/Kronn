import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { TourProvider, useTour } from '../TourProvider';
import { TourOverlay } from '../TourOverlay';
import { TOUR_STEPS } from '../tourSteps';

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

vi.mock('../../../hooks/useMediaQuery', () => ({
  useIsMobile: () => false,
}));

const STORAGE_KEY = 'kronn:tour-completed';
const setPage = vi.fn();

function TestConsumer() {
  const tour = useTour();
  return (
    <div>
      <span data-testid="active">{String(tour.isActive)}</span>
      <span data-testid="step">{tour.stepIndex}</span>
      <span data-testid="total">{tour.totalSteps}</span>
      <button data-testid="start" onClick={() => tour.start(true)}>Start</button>
      <button data-testid="next" onClick={tour.next}>Next</button>
      <button data-testid="prev" onClick={tour.prev}>Prev</button>
      <button data-testid="skip" onClick={tour.skip}>Skip</button>
    </div>
  );
}

function renderTour() {
  return render(
    <TourProvider setPage={setPage}>
      <TestConsumer />
      <TourOverlay />
    </TourProvider>
  );
}

beforeEach(() => {
  localStorage.clear();
  setPage.mockClear();
});

describe('Guided Tour', () => {
  it('auto-launches on first visit (no localStorage flag)', async () => {
    vi.useFakeTimers();
    renderTour();
    expect(screen.getByTestId('active').textContent).toBe('false');
    await act(async () => { vi.advanceTimersByTime(1000); });
    expect(screen.getByTestId('active').textContent).toBe('true');
    expect(screen.getByTestId('step').textContent).toBe('0');
    vi.useRealTimers();
  });

  it('does NOT auto-launch when tour already completed', async () => {
    localStorage.setItem(STORAGE_KEY, 'true');
    vi.useFakeTimers();
    renderTour();
    await act(async () => { vi.advanceTimersByTime(1000); });
    expect(screen.getByTestId('active').textContent).toBe('false');
    vi.useRealTimers();
  });

  it('start(force=true) launches even if completed', () => {
    localStorage.setItem(STORAGE_KEY, 'true');
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(screen.getByTestId('active').textContent).toBe('true');
  });

  it('navigates forward via ArrowRight keyboard shortcut', async () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(screen.getByTestId('step').textContent).toBe('0');

    // Step 0 (welcome) has selector: null — no DOM wait needed.
    // Step 1 has a selector that won't exist in test DOM, but the
    // 2s MutationObserver timeout will resolve to null and the step
    // advances anyway. We wait for that.
    await act(async () => {
      fireEvent.keyDown(document, { key: 'ArrowRight' });
      await new Promise(r => setTimeout(r, 2500));
    });
    expect(Number(screen.getByTestId('step').textContent)).toBeGreaterThanOrEqual(1);
  });

  it('skip persists completion to localStorage', () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(screen.getByTestId('active').textContent).toBe('true');

    fireEvent.click(screen.getByTestId('skip'));
    expect(screen.getByTestId('active').textContent).toBe('false');
    expect(localStorage.getItem(STORAGE_KEY)).toBe('true');
  });

  it('Escape key closes the tour', () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(screen.getByTestId('active').textContent).toBe('true');

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.getByTestId('active').textContent).toBe('false');
  });

  it('renders tooltip with step title when active', () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    // First step = welcome (centered, no selector)
    expect(screen.getByText('tour.welcome.title')).toBeInTheDocument();
    expect(screen.getByText('tour.welcome.desc')).toBeInTheDocument();
    expect(screen.getByText(`1 / ${TOUR_STEPS.length}`)).toBeInTheDocument();
  });

  it('start calls setPage with the first step page', () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(setPage).toHaveBeenCalledWith(TOUR_STEPS[0].page);
  });

  it('tour steps include pages beyond projects', () => {
    // Structural check: the step definitions span multiple pages
    const pages = new Set(TOUR_STEPS.map(s => s.page));
    expect(pages.size).toBeGreaterThanOrEqual(4);
    expect(pages.has('projects')).toBe(true);
    expect(pages.has('mcps')).toBe(true);
    expect(pages.has('discussions')).toBe(true);
    expect(pages.has('settings')).toBe(true);
  });

  it('totalSteps matches TOUR_STEPS length', () => {
    renderTour();
    expect(Number(screen.getByTestId('total').textContent)).toBe(TOUR_STEPS.length);
    expect(TOUR_STEPS.length).toBeGreaterThanOrEqual(10);
  });

  // ─── Step persistence / resume ─────────────────────────────────────

  it('persists the current step to localStorage when advancing', async () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(screen.getByTestId('step').textContent).toBe('0');
    // Step 0 → 1: advancing runs navigateToStep which awaits
    // waitForElement up to 2s when the selector isn't in the DOM
    // (the test harness doesn't render the real pages).
    await act(async () => {
      fireEvent.keyDown(document, { key: 'ArrowRight' });
      await new Promise(r => setTimeout(r, 2500));
    });
    expect(localStorage.getItem('kronn:tour-step')).toBe('1');
  });

  it('clears the saved step when the tour is completed (skip)', () => {
    localStorage.setItem('kronn:tour-step', '3');
    renderTour();
    fireEvent.click(screen.getByTestId('skip'));
    expect(localStorage.getItem('kronn:tour-step')).toBeNull();
    // And the completion flag is set.
    expect(localStorage.getItem(STORAGE_KEY)).toBe('true');
  });

  it('auto-resumes from a saved step when starting the tour', async () => {
    localStorage.setItem('kronn:tour-step', '3');
    renderTour();
    // Explicit non-forced start — resumeStep is read from localStorage.
    const tourRef = { start: null as null | ((force?: boolean) => void) };
    const GrabStart = () => {
      const tour = useTour();
      tourRef.start = tour.start;
      return null;
    };
    render(
      <TourProvider setPage={setPage}>
        <TestConsumer />
        <GrabStart />
      </TourProvider>
    );
    act(() => { tourRef.start?.(false); });
    // Wait for navigateToStep to settle (50ms kickoff + 300ms page wait
    // + 2s waitForElement timeout).
    await act(async () => { await new Promise(r => setTimeout(r, 2500)); });
    expect(localStorage.getItem('kronn:tour-step')).toBe('3');
  });

  it('start(force=true) always restarts at step 0 regardless of saved step', () => {
    localStorage.setItem(STORAGE_KEY, 'true');
    localStorage.setItem('kronn:tour-step', '5');
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(Number(screen.getByTestId('step').textContent)).toBe(0);
    expect(localStorage.getItem('kronn:tour-step')).toBe('0');
  });
});
