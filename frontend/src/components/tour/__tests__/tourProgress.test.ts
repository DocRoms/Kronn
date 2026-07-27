import { beforeEach, describe, expect, it } from 'vitest';
import {
  clearTourProgress,
  loadTourProgress,
  saveTourProgress,
  TOUR_PROGRESS_KEYS,
} from '../tourProgress';

const STEPS = ['welcome', 'projects', 'discussion', 'automation'];

beforeEach(() => {
  localStorage.clear();
});

describe('guided-tour progress', () => {
  it('starts on the first incomplete step', () => {
    expect(loadTourProgress(STEPS)).toMatchObject({
      completedStepIds: [],
      completedCount: 0,
      currentStepId: 'welcome',
      hasStarted: false,
      isComplete: false,
      resumeStepIndex: 0,
      skippedStepIds: [],
      totalSteps: 4,
    });
  });

  it('resumes the interrupted step after reload', () => {
    saveTourProgress(STEPS, ['welcome'], 'projects');

    expect(loadTourProgress(STEPS)).toMatchObject({
      completedStepIds: ['welcome'],
      currentStepId: 'projects',
      hasStarted: true,
      resumeStepIndex: 1,
    });
  });

  it('reactivates on the first new step after a completed version', () => {
    saveTourProgress(STEPS, STEPS, null);
    const expandedSteps = [...STEPS.slice(0, 2), 'new-capability', ...STEPS.slice(2)];

    expect(loadTourProgress(expandedSteps)).toMatchObject({
      completedCount: STEPS.length,
      isComplete: false,
      resumeStepIndex: 2,
      totalSteps: expandedSteps.length,
    });
  });

  it('drops removed and duplicate IDs without losing known progress', () => {
    localStorage.setItem(TOUR_PROGRESS_KEYS.current, JSON.stringify({
      schemaVersion: 1,
      completedStepIds: ['welcome', 'removed', 'welcome'],
      currentStepId: 'removed',
      skippedStepIds: ['removed', 'discussion', 'discussion', 'welcome'],
    }));

    expect(loadTourProgress(STEPS)).toMatchObject({
      completedStepIds: ['welcome'],
      currentStepId: null,
      hasStarted: true,
      resumeStepIndex: 1,
      skippedStepIds: ['discussion'],
    });
  });

  it('keeps skipped steps separate from actual completion', () => {
    saveTourProgress(STEPS, ['welcome'], 'discussion', true, ['projects']);

    expect(loadTourProgress(STEPS)).toMatchObject({
      completedStepIds: ['welcome'],
      skippedStepIds: ['projects'],
      completedCount: 1,
      isComplete: false,
      resumeStepIndex: 2,
    });
  });

  it('migrates the legacy completion flag to stable step IDs', () => {
    localStorage.setItem(TOUR_PROGRESS_KEYS.legacyCompleted, 'true');

    expect(loadTourProgress(STEPS)).toMatchObject({
      completedStepIds: STEPS,
      completedCount: STEPS.length,
      currentStepId: null,
      isComplete: true,
    });
  });

  it('migrates the legacy index as completed predecessors plus current step', () => {
    localStorage.setItem(TOUR_PROGRESS_KEYS.legacyStep, '2');

    expect(loadTourProgress(STEPS)).toMatchObject({
      completedStepIds: ['welcome', 'projects'],
      currentStepId: 'discussion',
      resumeStepIndex: 2,
    });
  });

  it('survives corrupt storage and clears current plus legacy keys', () => {
    localStorage.setItem(TOUR_PROGRESS_KEYS.current, '{invalid');
    localStorage.setItem(TOUR_PROGRESS_KEYS.legacyStep, '1');
    expect(loadTourProgress(STEPS).resumeStepIndex).toBe(1);

    clearTourProgress();
    expect(localStorage.getItem(TOUR_PROGRESS_KEYS.current)).toBeNull();
    expect(localStorage.getItem(TOUR_PROGRESS_KEYS.legacyCompleted)).toBeNull();
    expect(localStorage.getItem(TOUR_PROGRESS_KEYS.legacyStep)).toBeNull();
  });
});
