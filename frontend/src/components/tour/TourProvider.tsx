import { createContext, useContext, useState, useCallback, useEffect, useRef } from 'react';
import {
  TOUR_STEPS,
  archiveDemoDiscussion,
  type Page,
  type TourStep,
} from './tourSteps';
import { waitForElement } from './useTourPositioning';
import { loadTourProgress, saveTourProgress } from './tourProgress';

const AUTO_START_DELAY = 800;
const PAGE_TARGET_TIMEOUT_MS = 15_000;
const REQUIRED_TARGET_TIMEOUT_MS = 5_000;
const OPTIONAL_TARGET_TIMEOUT_MS = 2_000;
const TOUR_STEP_IDS = TOUR_STEPS.map(step => step.id);

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
  finish: () => void;
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
  const [progress, setProgress] = useState(() => loadTourProgress(TOUR_STEP_IDS));
  const navigatingRef = useRef(false);
  const clickListenerRef = useRef<(() => void) | null>(null);
  // Pending setTimeout id from a waitForClick advance — must be cleared
  // when the user skips, presses Prev/Next, or goes back. Pre-fix,
  // skipping during the 150 ms post-click delay left a dangling timer
  // that re-armed `setStepIndex` after `complete()` had already reset
  // state, dirtying `kronn:tour-step` localStorage.
  const pendingAdvanceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Use a ref to always have fresh stepIndex in async callbacks
  const stepIndexRef = useRef(stepIndex);
  const progressRef = useRef(progress);

  // The visible card becomes clickable as soon as React commits the state, but
  // the passive effect below may run later under load. Keep the imperative
  // navigation cursor in sync in the same tick as every state write; otherwise
  // a click on the newly visible card can still read the previous index and
  // navigate to the step already on screen, making Next appear to do nothing.
  const commitStepIndex = useCallback((index: number) => {
    stepIndexRef.current = index;
    setStepIndex(index);
  }, []);

  useEffect(() => {
    stepIndexRef.current = stepIndex;
  }, [stepIndex]);

  useEffect(() => {
    progressRef.current = progress;
  }, [progress]);

  const currentStep = active ? TOUR_STEPS[stepIndex] ?? null : null;

  const cleanupClickListener = useCallback(() => {
    if (clickListenerRef.current) {
      clickListenerRef.current();
      clickListenerRef.current = null;
    }
    if (pendingAdvanceRef.current !== null) {
      clearTimeout(pendingAdvanceRef.current);
      pendingAdvanceRef.current = null;
    }
    setWaitingForClick(false);
  }, []);

  const persistProgress = useCallback((
    completedStepIds: string[],
    currentStepId: string | null,
    hasStarted = true,
    skippedStepIds = progressRef.current.skippedStepIds,
  ) => {
    const nextProgress = saveTourProgress(
      TOUR_STEP_IDS,
      completedStepIds,
      currentStepId,
      hasStarted,
      skippedStepIds,
    );
    progressRef.current = nextProgress;
    setProgress(nextProgress);
    return nextProgress;
  }, []);

  const abandon = useCallback(() => {
    if (!active) return;
    cleanupClickListener();
    const step = TOUR_STEPS[stepIndexRef.current];
    if (active && step?.afterStep) step.afterStep();
    setActive(false);
    persistProgress(
      progressRef.current.completedStepIds,
      step?.id ?? progressRef.current.currentStepId,
      true,
      progressRef.current.skippedStepIds,
    );
  }, [active, cleanupClickListener, persistProgress]);

  const finish = useCallback(() => {
    cleanupClickListener();
    const step = TOUR_STEPS[stepIndexRef.current];
    if (active && step?.afterStep) step.afterStep();
    setActive(false);
    commitStepIndex(0);
    const completedStepIds = step
      ? [...new Set([...progressRef.current.completedStepIds, step.id])]
      : progressRef.current.completedStepIds;
    const completed = new Set(completedStepIds);
    const firstIncompleteStepId = TOUR_STEP_IDS.find(id => !completed.has(id)) ?? null;
    persistProgress(
      completedStepIds,
      firstIncompleteStepId,
      true,
      progressRef.current.skippedStepIds,
    );
    void archiveDemoDiscussion();
  }, [active, cleanupClickListener, commitStepIndex, persistProgress]);

  // Core navigation — called for every step transition
  /** `direction` is the way the user is travelling; a skipped step continues
   *  that way instead of always jumping forward. */
  const navigateToStep = useCallback(async function navigateToStepImpl(
    targetIndex: number,
    direction: 1 | -1 = 1,
  ): Promise<void> {
    if (targetIndex < 0 || targetIndex >= TOUR_STEPS.length) return;
    // A visible Next/Prev click is a user command, not a hint. Page switches and
    // lazy mounts can overlap with a fast click under load; dropping that click
    // made the button appear broken. Serialize behind the in-flight transition
    // so every accepted click is eventually applied.
    while (navigatingRef.current) {
      await new Promise(resolve => setTimeout(resolve, 25));
    }
    navigatingRef.current = true;
    try {
      cleanupClickListener();

    const fromStep = TOUR_STEPS[stepIndexRef.current];
    const toStep = TOUR_STEPS[targetIndex];
    let completedStepIds = progressRef.current.completedStepIds;
    let skippedStepIds = progressRef.current.skippedStepIds;

    // Cleanup previous step
    if (fromStep?.afterStep) fromStep.afterStep();
    if (direction > 0 && targetIndex > stepIndexRef.current && fromStep) {
      completedStepIds = [...new Set([...completedStepIds, fromStep.id])];
    }

    // Page navigation
    if (toStep.page !== fromStep?.page) {
      setPage(toStep.page);
      await new Promise(r => setTimeout(r, 300));
    }

    // Pre-step action
    if (toStep.beforeStep) {
      await toStep.beforeStep();
      await new Promise(r => setTimeout(r, 200));
    }

    // Wait for selector
    if (toStep.selector) {
      const el = await waitForElement(
        toStep.selector,
        toStep.optionalWhenMissing
          ? OPTIONAL_TARGET_TIMEOUT_MS
          : toStep.page !== fromStep?.page
            ? PAGE_TARGET_TIMEOUT_MS
            : REQUIRED_TARGET_TIMEOUT_MS,
      );

      // A step whose target never appears has nothing to show. Skip it and keep
      // going in the direction the user was heading, rather than parking on a
      // card that points at nothing — an outdated step should go quiet, not
      // break the tour. The skip happens only AFTER `waitForElement`'s bounded
      // MutationObserver wait, so a target that mounts late on a slow machine is
      // still caught; skipping on the first miss would trade a visible freeze
      // for steps randomly dropped under load.
      if (!el) {
        skippedStepIds = [...new Set([...skippedStepIds, toStep.id])];
        if (direction > 0 && toStep.optionalWhenMissing) {
          completedStepIds = [...new Set([...completedStepIds, toStep.id])];
        }
        if (import.meta.env.DEV) {
          console.warn(
            `[tour] step "${toStep.id}" skipped: no element matches ${toStep.selector}`,
          );
        }
        const skipTo = targetIndex + direction;
        persistProgress(
          completedStepIds,
          TOUR_STEPS[skipTo]?.id ?? null,
          true,
          skippedStepIds,
        );
        navigatingRef.current = false;
        if (skipTo >= 0 && skipTo < TOUR_STEPS.length) {
          await navigateToStepImpl(skipTo, direction);
        } else if (direction > 0) {
          finish();
        } else {
          commitStepIndex(targetIndex);
          persistProgress(completedStepIds, toStep.id, true, skippedStepIds);
        }
        return;
      }
      skippedStepIds = skippedStepIds.filter(id => id !== toStep.id);

      // Setup waitForClick listener
      if (toStep.waitForClick && el) {
        setWaitingForClick(true);
        commitStepIndex(targetIndex);
        persistProgress(completedStepIds, toStep.id, true, skippedStepIds);
        navigatingRef.current = false;

        const onUserClick = () => {
          el.removeEventListener('click', onUserClick);
          clickListenerRef.current = null;
          setWaitingForClick(false);
          // Let the click's side effect happen (modal opens, accordion
          // expands, etc.) then advance. Pre-fix this used a 400 ms
          // timeout — perceptibly laggy after a fast click. 150 ms is
          // enough for the React re-render that opens the accordion to
          // commit, while staying snappy enough that the spotlight
          // motion feels responsive. Track the timer in a ref so a
          // skip / Prev / Next during the wait can cancel it.
          pendingAdvanceRef.current = setTimeout(() => {
            pendingAdvanceRef.current = null;
            const nextIdx = targetIndex + 1;
            if (nextIdx < TOUR_STEPS.length) {
              void navigateToStepImpl(nextIdx);
            } else {
              finish();
            }
          }, 150);
        };

        el.addEventListener('click', onUserClick);
        // Also accept a click on any descendant of the spotlight target
        // if `el` is a wrapper (e.g. the chip-list container) — captures
        // the case where the visible click target is the inner span/icon
        // and the bubble path doesn't reach `el`'s `click` handler if the
        // descendant called preventDefault. Safety net.
        clickListenerRef.current = () => el.removeEventListener('click', onUserClick);
        return; // Don't fall through to the final setStepIndex/unlock below
      }
    }

    commitStepIndex(targetIndex);
    persistProgress(completedStepIds, toStep.id, true, skippedStepIds);
    } finally {
      // Exceptions in page hooks or persistence must never permanently disable
      // the navigation buttons. Every exit path releases the serializer.
      navigatingRef.current = false;
    }
  }, [setPage, cleanupClickListener, commitStepIndex, finish, persistProgress]);

  // Pre-fix: `next` and `prev` bailed out when `waitingForClick === true`,
  // and the TourOverlay also hid the buttons in that state. Effect: on
  // steps 11/12 (waitForClick on profile toggle + first chip), the user
  // had no escape — they were forced to click the spotlighted target,
  // could not back out to the previous step, and could not skip ahead
  // to the next without using "Passer" (which marks the tour complete).
  // Now both helpers clean up the wait state first, so the manual
  // navigation works regardless of whether a click was awaited.
  const next = useCallback(() => {
    cleanupClickListener();
    const nextIdx = stepIndexRef.current + 1;
    if (nextIdx >= TOUR_STEPS.length) {
      finish();
    } else {
      navigateToStep(nextIdx);
    }
  }, [navigateToStep, finish, cleanupClickListener]);

  const prev = useCallback(() => {
    cleanupClickListener();
    const prevIdx = stepIndexRef.current - 1;
    if (prevIdx >= 0) navigateToStep(prevIdx, -1);
  }, [navigateToStep, cleanupClickListener]);

  const start = useCallback((force = false) => {
    const latestProgress = loadTourProgress(TOUR_STEP_IDS);
    if (!force && latestProgress.isComplete) return;
    cleanupClickListener();
    navigatingRef.current = false;
    // A manual replay (force=true, e.g. the "?" help button) always
    // restarts at step 0 — the user asked for a fresh run. An auto-
    // resume after a refresh picks up where the user left off.
    const resumeStep = force ? 0 : latestProgress.resumeStepIndex;
    // Initialize at the resume step directly. Pre-fix this set
    // `stepIndex(0)` first and bumped to the resume step 50 ms later,
    // which produced a visible flash of step 1 before jumping ahead.
    commitStepIndex(resumeStep);
    persistProgress(
      latestProgress.completedStepIds,
      TOUR_STEPS[resumeStep]?.id ?? null,
      true,
      latestProgress.skippedStepIds,
    );
    setActive(true);
    setPage(TOUR_STEPS[resumeStep]?.page ?? TOUR_STEPS[0].page);
    // If resuming mid-tour, drive the full navigation pipeline so the
    // target step's `beforeStep` hook (and any page switch) runs —
    // otherwise a refresh inside the profiles sub-flow would land on
    // a collapsed accordion and an invisible selector.
    if (resumeStep > 0) {
      setTimeout(() => { navigateToStep(resumeStep); }, 50);
    }
  }, [setPage, cleanupClickListener, commitStepIndex, navigateToStep, persistProgress]);

  // The launcher is the real form, but its launch action is intercepted during
  // the demo: click or Ctrl/Cmd+Enter advances to the local seeded conversation
  // instead of creating another discussion or starting an agent.
  useEffect(() => {
    if (!active) return;
    const handleDemoLaunch = () => {
      if (TOUR_STEPS[stepIndexRef.current]?.id === 'disc-form') next();
    };
    window.addEventListener('kronn:tour-demo-launch', handleDemoLaunch);
    return () => window.removeEventListener('kronn:tour-demo-launch', handleDemoLaunch);
  }, [active, next]);

  // Auto-launch only for a genuinely new user. Once a tour has started, an
  // interruption or deliberate close is resumed from the CTA in Configuration
  // instead of unexpectedly taking over the application after every reload.
  useEffect(() => {
    const latestProgress = loadTourProgress(TOUR_STEP_IDS);
    if (latestProgress.hasStarted || latestProgress.isComplete) return;
    const timer = setTimeout(() => start(), AUTO_START_DELAY);
    return () => clearTimeout(timer);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Keyboard navigation. Arrow keys also work during `waitingForClick`
  // now — same rationale as the visible Prev/Next buttons: the user
  // needs an escape hatch when their click on the spotlighted target
  // didn't register (covered descendant, custom event handler, etc.).
  useEffect(() => {
    if (!active) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') { abandon(); e.preventDefault(); }
      const target = e.target instanceof HTMLElement ? e.target : null;
      const isEditing = target?.matches('input, textarea, select, [contenteditable="true"]')
        || Boolean(target?.closest('[contenteditable="true"]'));
      if (!isEditing && e.key === 'ArrowRight') { next(); e.preventDefault(); }
      if (!isEditing && e.key === 'ArrowLeft') { prev(); e.preventDefault(); }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [active, next, prev, abandon]);

  // Interactive coachmarks let the spotlighted control receive the click, but
  // the rest of the application must stay inert: otherwise a stray click on
  // navigation can move the page while the tour still describes the old step.
  useEffect(() => {
    if (!active || !waitingForClick || !currentStep?.selector) return;
    const target = document.querySelector<HTMLElement>(currentStep.selector);
    if (!target) return;
    const tooltipContains = (clicked: Node) => (
      document.querySelector('.tour-tooltip')?.contains(clicked) ?? false
    );
    const blockOutsideClick = (event: MouseEvent) => {
      const clicked = event.target instanceof Node ? event.target : null;
      if (clicked && (target.contains(clicked) || tooltipContains(clicked))) return;
      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation();
    };
    document.addEventListener('click', blockOutsideClick, true);
    return () => document.removeEventListener('click', blockOutsideClick, true);
  }, [active, waitingForClick, currentStep?.id, currentStep?.selector]);

  useEffect(() => () => {
    cleanupClickListener();
    TOUR_STEPS[stepIndexRef.current]?.afterStep?.();
  }, [cleanupClickListener]);

  const value: TourContextValue = {
    isActive: active,
    stepIndex,
    totalSteps: TOUR_STEPS.length,
    currentStep,
    waitingForClick,
    start,
    next,
    prev,
    skip: abandon,
    finish,
  };

  return (
    <TourContext.Provider value={value}>
      {children}
    </TourContext.Provider>
  );
}
