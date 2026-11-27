import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { TourProvider, useTour } from '../TourProvider';
import { TourOverlay } from '../TourOverlay';
import { TOUR_STEPS } from '../tourSteps';
import { loadTourProgress, TOUR_PROGRESS_KEYS } from '../tourProgress';
import { discussions as discussionsApi } from '../../../lib/api';

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

vi.mock('../../../hooks/useMediaQuery', () => ({
  useIsMobile: () => false,
}));

vi.mock('../../../lib/api', () => ({
  discussions: {
    ensureTourDemo: vi.fn().mockResolvedValue({
      discussion_id: 'tour-demo',
      created: true,
      prompt: 'Build a demo page',
    }),
    update: vi.fn().mockResolvedValue(undefined),
  },
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
      <button data-testid="resume" onClick={() => tour.start(false)}>Resume</button>
      <button data-testid="next" onClick={tour.next}>Next</button>
      <button data-testid="prev" onClick={tour.prev}>Prev</button>
      <button data-testid="skip" onClick={tour.skip}>Skip</button>
      <button data-testid="finish" onClick={tour.finish}>Finish</button>
      <textarea data-testid="tour-editable" defaultValue="editable" />
    </div>
  );
}

/**
 * Stand-ins for the elements the steps point at. Without them every step is
 * skipped (a step with no target has nothing to show), so navigation tests would
 * only ever observe the tour running to its end — they used to pass because the
 * provider displayed steps that pointed at nothing.
 */
function TourAnchors() {
  const selectors = [...new Set(TOUR_STEPS.map(s => s.selector).filter(Boolean))] as string[];
  return (
    <div>
      {selectors.map(selector => {
        const tourId = /\[data-tour-id="([^"]+)"\]/.exec(selector)?.[1];
        if (tourId) return <div key={selector} data-tour-id={tourId} />;
        if (selector.startsWith('#')) return <div key={selector} id={selector.slice(1)} />;
        return <div key={selector} className={selector.replace(/^\./, '')} />;
      })}
    </div>
  );
}

function renderTour({ withAnchors = true } = {}) {
  return render(
    <TourProvider setPage={setPage}>
      <TestConsumer />
      {withAnchors && <TourAnchors />}
      <TourOverlay />
    </TourProvider>
  );
}

beforeEach(() => {
  localStorage.clear();
  setPage.mockClear();
  vi.mocked(discussionsApi.ensureTourDemo).mockClear();
  vi.mocked(discussionsApi.update).mockClear();
});

afterEach(() => {
  // A failed timer-driven assertion must not leave fake timers installed and
  // turn every following test into an unrelated timeout cascade.
  vi.useRealTimers();
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

  it('skips a step whose target no longer exists instead of stalling on it', async () => {
    // KT-117 — an outdated step used to be displayed anyway: no spotlight, so no
    // darkness, while the full-screen backdrop still swallowed every click. The
    // app looked untouched and ignored the mouse, which reads as "frozen, reload
    // to escape". A step with nothing to show must go quiet and let the tour
    // continue. Rendered WITHOUT anchors so every targeted step is missing.
    vi.useFakeTimers();
    renderTour({ withAnchors: false });
    fireEvent.click(screen.getByTestId('start'));
    expect(screen.getByTestId('active').textContent).toBe('true');

    // Each skipped step first exhausts its own bounded wait, so give the walk
    // room: the point is that it walks, not that it walks instantly.
    await act(async () => {
      fireEvent.keyDown(document, { key: 'ArrowRight' });
      await vi.advanceTimersByTimeAsync(600_000);
    });

    // Every targeted step in between is missing, so the tour walks over all of
    // them and lands on the final centered step instead of parking on an empty
    // card. It is still active — the last step has something to say.
    expect(screen.getByTestId('step').textContent).toBe(String(TOUR_STEPS.length - 1));
    expect(screen.getByTestId('active').textContent).toBe('true');
    vi.useRealTimers();
  });

  it('never leaves an undimmed backdrop swallowing clicks with no spotlight', () => {
    // Defence in depth for the transient window before a target is measured:
    // the darkness comes from the spotlight, so no spotlight must mean a dimmed
    // backdrop — never an invisible one that still blocks the page.
    renderTour();
    fireEvent.click(screen.getByTestId('start'));

    const backdrop = document.querySelector('.tour-backdrop');
    expect(backdrop).not.toBeNull();
    expect(document.querySelector('.tour-spotlight')).toBeNull();
    expect(backdrop).toHaveAttribute('data-dimmed', 'true');
    expect(screen.getByText('tour.skip')).toBeInTheDocument();
  });

  it('skip preserves an interrupted tour for the Settings resume CTA', () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(screen.getByTestId('active').textContent).toBe('true');

    fireEvent.click(screen.getByTestId('skip'));
    expect(screen.getByTestId('active').textContent).toBe('false');
    expect(loadTourProgress(TOUR_STEPS.map(step => step.id))).toMatchObject({
      currentStepId: TOUR_STEPS[0].id,
      hasStarted: true,
      isComplete: false,
    });
  });

  it('Escape key closes the tour', () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(screen.getByTestId('active').textContent).toBe('true');

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.getByTestId('active').textContent).toBe('false');
  });

  it('does not hijack arrow keys from an editable field', () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    const editable = screen.getByTestId('tour-editable');
    editable.focus();

    fireEvent.keyDown(editable, { key: 'ArrowRight' });

    expect(screen.getByTestId('step').textContent).toBe('0');
    expect(editable).toHaveFocus();
  });

  it('focuses the dialog and restores the previous focus on close', async () => {
    renderTour();
    const start = screen.getByTestId('start');
    start.focus();
    fireEvent.click(start);

    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(screen.getByRole('dialog')).toHaveFocus();

    fireEvent.click(screen.getByTestId('skip'));
    expect(start).toHaveFocus();
  });

  it('finish never fabricates completion for steps that were not shown', () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    fireEvent.click(screen.getByTestId('finish'));

    expect(loadTourProgress(TOUR_STEPS.map(step => step.id))).toMatchObject({
      completedCount: 1,
      currentStepId: TOUR_STEPS[1].id,
      isComplete: false,
    });
  });

  it('does not create a demo discussion before the discussion act', async () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    await act(async () => { await Promise.resolve(); });
    expect(discussionsApi.ensureTourDemo).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTestId('finish'));
    await act(async () => { await Promise.resolve(); });
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
    expect(setPage).toHaveBeenCalledWith(TOUR_STEPS[0].page, undefined);
  });

  // KT-1923 — Settings has five sub-pages and `/config` opens the first: the
  // agents step names its section, so the tour opens the sub-page that holds it.
  it('opens Settings on the sub-page of the section a step points at', () => {
    const agentsStep = TOUR_STEPS.find(step => step.id === 'agents-config');
    expect(agentsStep?.page).toBe('settings');
    expect(agentsStep?.settingsSection).toBe('settings-agent-config');
    expect(TOUR_STEPS.filter(step => step.page === 'settings').every(step => step.settingsSection)).toBe(true);
  });

  // KT-1923 — the agents step opens a sub-page drawn on its first visit, which
  // is still moving when its header appears. Shown at that moment, the step
  // measured a passing position and the spotlight stood 184 to 195 px from its
  // target (guided-tour-viewport.spec.ts). After a page switch the step now
  // waits for its target to hold still, then the spotlight sits on it.
  it('after a page switch, shows the step once its target holds still, with the spotlight on it', async () => {
    vi.useFakeTimers();
    const at = TOUR_STEPS.findIndex(step => step.id === 'agents-config');
    const from = at - 1;
    expect(TOUR_STEPS[from].page).not.toBe('settings');
    localStorage.setItem(TOUR_PROGRESS_KEYS.current, JSON.stringify({
      schemaVersion: 1,
      completedStepIds: TOUR_STEPS.slice(0, from).map(step => step.id),
      currentStepId: TOUR_STEPS[from].id,
      hasStarted: true,
      skippedStepIds: [],
    }));
    renderTour();
    fireEvent.click(screen.getByTestId('resume'));
    await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
    expect(screen.getByTestId('step').textContent).toBe(String(from));

    // The header keeps sliding up while the sub-page is drawn and scrolled.
    const target = document.querySelector<HTMLElement>('[data-tour-id="settings-agents"]')!;
    const settledTop = 120;
    let moving = true;
    let top = 600;
    vi.spyOn(target, 'getBoundingClientRect').mockImplementation(() => {
      if (moving) top = Math.max(settledTop + 1, top - 7); else top = settledTop;
      return { top, left: 40, width: 600, height: 48, bottom: top + 48, right: 640, x: 40, y: top, toJSON: () => ({}) } as DOMRect;
    });
    fireEvent.click(screen.getByTestId('next'));

    // Still moving: the previous step stays on screen.
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(setPage).toHaveBeenLastCalledWith('settings', 'settings-agent-config');
    expect(screen.getByTestId('step').textContent).toBe(String(from));

    // Held still: the step shows, and its spotlight sits on the target.
    moving = false;
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(screen.getByTestId('step').textContent).toBe(String(at));
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    const spotlight = document.querySelector<HTMLElement>('.tour-spotlight');
    expect(spotlight?.style.top).toBe(`${settledTop - 8}px`);
    expect(spotlight?.style.left).toBe('32px');
    vi.useRealTimers();
  });

  // KT-2017 — on a scrolled Settings sub-page the band of section links sticks
  // under the top bar. The agents step's header could stand under it, in the
  // viewport yet covered, and the spotlight sat on the band. The page gives
  // its tour targets the scroll margin of what covers them: a target under it
  // is brought into view below it.
  it('brings a target the page keeps covered at the top into view below the cover', async () => {
    vi.useFakeTimers();
    const at = TOUR_STEPS.findIndex(step => step.id === 'agents-config');
    const from = at - 1;
    localStorage.setItem(TOUR_PROGRESS_KEYS.current, JSON.stringify({
      schemaVersion: 1,
      completedStepIds: TOUR_STEPS.slice(0, from).map(step => step.id),
      currentStepId: TOUR_STEPS[from].id,
      hasStarted: true,
      skippedStepIds: [],
    }));
    renderTour();
    fireEvent.click(screen.getByTestId('resume'));
    await act(async () => { await vi.advanceTimersByTimeAsync(3000); });

    const target = document.querySelector<HTMLElement>('[data-tour-id="settings-agents"]')!;
    target.style.setProperty('scroll-margin-top', '111px');
    let top = 70; // under the top bar (51 px) and the band (44 px)
    vi.spyOn(target, 'getBoundingClientRect').mockImplementation(() => (
      { top, left: 40, width: 600, height: 48, bottom: top + 48, right: 640, x: 40, y: top, toJSON: () => ({}) } as DOMRect
    ));
    const scrollIntoView = vi.fn(() => { top = 420; });
    target.scrollIntoView = scrollIntoView;
    fireEvent.click(screen.getByTestId('next'));
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(screen.getByTestId('step').textContent).toBe(String(at));
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });

    expect(scrollIntoView).toHaveBeenCalledWith({ behavior: 'auto', block: 'center' });
    expect(document.querySelector<HTMLElement>('.tour-spotlight')?.style.top).toBe(`${420 - 8}px`);
    vi.useRealTimers();
  });

  it('leaves a target in place when nothing covers it there', async () => {
    vi.useFakeTimers();
    const at = TOUR_STEPS.findIndex(step => step.id === 'agents-config');
    const from = at - 1;
    localStorage.setItem(TOUR_PROGRESS_KEYS.current, JSON.stringify({
      schemaVersion: 1,
      completedStepIds: TOUR_STEPS.slice(0, from).map(step => step.id),
      currentStepId: TOUR_STEPS[from].id,
      hasStarted: true,
      skippedStepIds: [],
    }));
    renderTour();
    fireEvent.click(screen.getByTestId('resume'));
    await act(async () => { await vi.advanceTimersByTimeAsync(3000); });

    const target = document.querySelector<HTMLElement>('[data-tour-id="settings-agents"]')!;
    target.style.setProperty('scroll-margin-top', '111px');
    vi.spyOn(target, 'getBoundingClientRect').mockImplementation(() => (
      { top: 160, left: 40, width: 600, height: 48, bottom: 208, right: 640, x: 40, y: 160, toJSON: () => ({}) } as DOMRect
    ));
    const scrollIntoView = vi.fn();
    target.scrollIntoView = scrollIntoView;
    fireEvent.click(screen.getByTestId('next'));
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(screen.getByTestId('step').textContent).toBe(String(at));
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });

    expect(scrollIntoView).not.toHaveBeenCalled();
    vi.useRealTimers();
  });

  it('tour steps include pages beyond projects', () => {
    // Structural check: the step definitions span multiple pages
    const pages = new Set(TOUR_STEPS.map(s => s.page));
    expect(pages.size).toBeGreaterThanOrEqual(4);
    expect(pages.has('projects')).toBe(true);
    expect(pages.has('mcps')).toBe(true);
    expect(pages.has('discussions')).toBe(true);
    expect(pages.has('settings')).toBe(true);
    // Planning arrived after v2 of the tour and was invisible to newcomers.
    expect(pages.has('planning')).toBe(true);
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
    // waitForElement up to 5s when the selector isn't in the DOM
    // (the test harness doesn't render the real pages).
    await act(async () => {
      fireEvent.keyDown(document, { key: 'ArrowRight' });
      await new Promise(r => setTimeout(r, 2500));
    });
    expect(loadTourProgress(TOUR_STEPS.map(step => step.id))).toMatchObject({
      completedStepIds: [TOUR_STEPS[0].id],
      currentStepId: TOUR_STEPS[1].id,
      resumeStepIndex: 1,
    });
  });

  it('keeps the saved step when the tour is interrupted', async () => {
    localStorage.setItem('kronn:tour-step', '3');
    renderTour();
    fireEvent.click(screen.getByTestId('resume'));
    await act(async () => { await new Promise(r => setTimeout(r, 2500)); });
    fireEvent.click(screen.getByTestId('skip'));
    expect(loadTourProgress(TOUR_STEPS.map(step => step.id))).toMatchObject({
      currentStepId: TOUR_STEPS[3].id,
      isComplete: false,
    });
  });

  it('auto-resumes from a saved step when starting the tour', async () => {
    localStorage.setItem('kronn:tour-step', '3');
    renderTour();
    // Explicit non-forced start — resumeStep is read from localStorage.
    fireEvent.click(screen.getByTestId('resume'));
    // Wait for navigateToStep to settle (50ms kickoff + 300ms page wait
    // + 5s waitForElement timeout).
    await act(async () => { await new Promise(r => setTimeout(r, 2500)); });
    expect(loadTourProgress(TOUR_STEPS.map(step => step.id)).resumeStepIndex).toBe(3);
  });

  it('start(force=true) always restarts at step 0 regardless of saved step', () => {
    localStorage.setItem(STORAGE_KEY, 'true');
    localStorage.setItem('kronn:tour-step', '5');
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(Number(screen.getByTestId('step').textContent)).toBe(0);
    // Replaying does not erase the already-earned completion state. If the
    // user closes a voluntary replay, the Settings CTA stays completed.
    expect(loadTourProgress(TOUR_STEPS.map(step => step.id)).isComplete).toBe(true);
  });

  // ─── Manual nav still works during waitForClick (steps 11/12 fix) ──
  it('Next manually advances even when waitingForClick is true', async () => {
    // Pre-fix: on waitForClick steps (11/12 = profile toggle / first
    // chip), `next` bailed out and the button was hidden, so the user
    // was forced to either click the spotlight target or skip the whole
    // tour. Now `next` cancels the click listener and advances.
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(screen.getByTestId('step').textContent).toBe('0');

    // Force the provider into a waitForClick-ish state by advancing past
    // a couple of selector-less steps and then asserting `next` advances
    // the counter without requiring a real DOM click on a missing target.
    await act(async () => {
      fireEvent.keyDown(document, { key: 'ArrowRight' });
      await new Promise(r => setTimeout(r, 2500));
    });
    const beforeStep = Number(screen.getByTestId('step').textContent);
    await act(async () => {
      fireEvent.click(screen.getByTestId('next'));
      await new Promise(r => setTimeout(r, 2500));
    });
    expect(Number(screen.getByTestId('step').textContent)).toBeGreaterThan(beforeStep);
  });

  it('Prev manually goes back even when waitingForClick is true', async () => {
    renderTour();
    fireEvent.click(screen.getByTestId('start'));

    // Move forward two steps so a Prev is meaningful.
    await act(async () => {
      fireEvent.keyDown(document, { key: 'ArrowRight' });
      await new Promise(r => setTimeout(r, 2500));
    });
    await act(async () => {
      fireEvent.keyDown(document, { key: 'ArrowRight' });
      await new Promise(r => setTimeout(r, 2500));
    });
    const before = Number(screen.getByTestId('step').textContent);

    await act(async () => {
      fireEvent.click(screen.getByTestId('prev'));
      await new Promise(r => setTimeout(r, 2500));
    });
    expect(Number(screen.getByTestId('step').textContent)).toBe(before - 1);
  });

  // ─── Backdrop click no longer dismisses the tour ────────────────────
  it('clicking the dark backdrop does NOT dismiss the tour or mark it completed', () => {
    // Pre-fix the backdrop's onClick was `skip` — a stray click on the
    // dim area outside the tooltip permanently marked the tour as done
    // (kronn:tour-completed = "true") so the user could only get it back
    // via the "?" help button. Now the backdrop is non-interactive: only
    // the explicit "Passer / Finir" buttons + the Escape shortcut count
    // as intentional dismissals.
    renderTour();
    fireEvent.click(screen.getByTestId('start'));
    expect(screen.getByTestId('active').textContent).toBe('true');

    const backdrop = document.querySelector('.tour-backdrop') as HTMLElement;
    expect(backdrop).not.toBeNull();
    fireEvent.click(backdrop);

    // Tour stays active, completion flag stays unset.
    expect(screen.getByTestId('active').textContent).toBe('true');
    expect(localStorage.getItem(STORAGE_KEY)).toBeNull();
    expect(localStorage.getItem(TOUR_PROGRESS_KEYS.current)).not.toBeNull();
  });
});
