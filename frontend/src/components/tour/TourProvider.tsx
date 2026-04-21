import { createContext, useContext, useState, useCallback, useEffect, useRef } from 'react';
import { TOUR_STEPS, type Page, type TourStep } from './tourSteps';
import { waitForElement } from './useTourPositioning';

const STORAGE_KEY = 'kronn:tour-completed';
/** Step index where the user left off, persisted so an accidental
 *  refresh / tab close mid-tour picks up exactly where they stopped.
 *  Cleared on `complete()` so a full replay always starts at step 0. */
const STORAGE_KEY_STEP = 'kronn:tour-step';
const AUTO_START_DELAY = 800;

function loadResumeStep(): number {
  try {
    const raw = localStorage.getItem(STORAGE_KEY_STEP);
    if (!raw) return 0;
    const n = parseInt(raw, 10);
    if (!Number.isFinite(n) || n < 0 || n >= TOUR_STEPS.length) return 0;
    return n;
  } catch { return 0; }
}

function saveResumeStep(stepIndex: number) {
  try { localStorage.setItem(STORAGE_KEY_STEP, String(stepIndex)); } catch { /* noop */ }
}

function clearResumeStep() {
  try { localStorage.removeItem(STORAGE_KEY_STEP); } catch { /* noop */ }
}

interface TourContextValue {
  isActive: boolean;
  stepIndex: number;
  totalSteps: number;
  currentStep: TourStep | null;
  waitingForClick: boolean;
  start: (force?: boolean) => void;
  next: () => void;
  prev: () => void;
  skip: () => void;
}

const TourContext = createContext<TourContextValue | null>(null);

export function useTour(): TourContextValue {
  const ctx = useContext(TourContext);
  if (!ctx) throw new Error('useTour must be used within TourProvider');
  return ctx;
}

interface TourProviderProps {
  setPage: (page: Page) => void;
  children: React.ReactNode;
}

export function TourProvider({ setPage, children }: TourProviderProps) {
  const [active, setActive] = useState(false);
  const [stepIndex, setStepIndex] = useState(0);
  const [waitingForClick, setWaitingForClick] = useState(false);
  const navigatingRef = useRef(false);
  const clickListenerRef = useRef<(() => void) | null>(null);
  // Use a ref to always have fresh stepIndex in async callbacks
  const stepIndexRef = useRef(stepIndex);
  stepIndexRef.current = stepIndex;

  const currentStep = active ? TOUR_STEPS[stepIndex] ?? null : null;

  const cleanupClickListener = useCallback(() => {
    if (clickListenerRef.current) {
      clickListenerRef.current();
      clickListenerRef.current = null;
    }
    setWaitingForClick(false);
  }, []);

  const complete = useCallback(() => {
    cleanupClickListener();
    const step = TOUR_STEPS[stepIndexRef.current];
    if (active && step?.afterStep) step.afterStep();
    setActive(false);
    setStepIndex(0);
    localStorage.setItem(STORAGE_KEY, 'true');
    // Mid-tour progress no longer needed — next launch starts fresh.
    clearResumeStep();
  }, [active, cleanupClickListener]);

  // Core navigation — called for every step transition
  const navigateToStep = useCallback(async (targetIndex: number) => {
    if (targetIndex < 0 || targetIndex >= TOUR_STEPS.length) return;
    if (navigatingRef.current) return;
    navigatingRef.current = true;
    cleanupClickListener();

    const fromStep = TOUR_STEPS[stepIndexRef.current];
    const toStep = TOUR_STEPS[targetIndex];

    // Cleanup previous step
    if (fromStep?.afterStep) fromStep.afterStep();

    // Page navigation
    if (toStep.page !== fromStep?.page) {
      setPage(toStep.page);
      await new Promise(r => setTimeout(r, 300));
    }

    // Pre-step action
    if (toStep.beforeStep) {
      toStep.beforeStep();
      await new Promise(r => setTimeout(r, 200));
    }

    // Wait for selector
    if (toStep.selector) {
      const el = await waitForElement(toStep.selector, 2000);

      // Setup waitForClick listener
      if (toStep.waitForClick && el) {
        setWaitingForClick(true);
        setStepIndex(targetIndex);
        navigatingRef.current = false;

        const onUserClick = () => {
          el.removeEventListener('click', onUserClick);
          clickListenerRef.current = null;
          setWaitingForClick(false);
          // Let the click's side effect happen (modal opens, etc.)
          // then advance to next step
          setTimeout(() => {
            const nextIdx = targetIndex + 1;
            if (nextIdx < TOUR_STEPS.length) {
              navigateToStep(nextIdx);
            } else {
              complete();
            }
          }, 400);
        };

        el.addEventListener('click', onUserClick);
        clickListenerRef.current = () => el.removeEventListener('click', onUserClick);
        return; // Don't fall through to the final setStepIndex/unlock below
      }
    }

    setStepIndex(targetIndex);
    saveResumeStep(targetIndex);
    navigatingRef.current = false;
  }, [setPage, cleanupClickListener, complete]);

  const next = useCallback(() => {
    if (waitingForClick) return;
    const nextIdx = stepIndexRef.current + 1;
    if (nextIdx >= TOUR_STEPS.length) {
      complete();
    } else {
      navigateToStep(nextIdx);
    }
  }, [navigateToStep, complete, waitingForClick]);

  const prev = useCallback(() => {
    if (waitingForClick) return;
    const prevIdx = stepIndexRef.current - 1;
    if (prevIdx >= 0) navigateToStep(prevIdx);
  }, [navigateToStep, waitingForClick]);

  const start = useCallback((force = false) => {
    if (!force && localStorage.getItem(STORAGE_KEY)) return;
    cleanupClickListener();
    navigatingRef.current = false;
    // A manual replay (force=true, e.g. the "?" help button) always
    // restarts at step 0 — the user asked for a fresh run. An auto-
    // resume after a refresh picks up where the user left off.
    const resumeStep = force ? 0 : loadResumeStep();
    setStepIndex(0);
    saveResumeStep(resumeStep);
    setActive(true);
    setPage(TOUR_STEPS[0].page);
    // If resuming mid-tour, drive the full navigation pipeline so the
    // target step's `beforeStep` hook (and any page switch) runs —
    // otherwise a refresh inside the profiles sub-flow would land on
    // a collapsed accordion and an invisible selector.
    if (resumeStep > 0) {
      setTimeout(() => { navigateToStep(resumeStep); }, 50);
    }
  }, [setPage, cleanupClickListener, navigateToStep]);

  // Auto-launch on first visit, OR resume a tour the user abandoned
  // before completing it (saved step > 0 and no completion flag).
  useEffect(() => {
    if (localStorage.getItem(STORAGE_KEY)) return;
    const timer = setTimeout(() => start(), AUTO_START_DELAY);
    return () => clearTimeout(timer);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Keyboard navigation
  useEffect(() => {
    if (!active) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') { complete(); e.preventDefault(); }
      if (e.key === 'ArrowRight' && !waitingForClick) { next(); e.preventDefault(); }
      if (e.key === 'ArrowLeft' && !waitingForClick) { prev(); e.preventDefault(); }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [active, next, prev, complete, waitingForClick]);

  useEffect(() => cleanupClickListener, [cleanupClickListener]);

  const value: TourContextValue = {
    isActive: active,
    stepIndex,
    totalSteps: TOUR_STEPS.length,
    currentStep,
    waitingForClick,
    start,
    next,
    prev,
    skip: complete,
  };

  return (
    <TourContext.Provider value={value}>
      {children}
    </TourContext.Provider>
  );
}
