const TOUR_PROGRESS_STORAGE_KEY = 'kronn:tour-progress:v1';
const LEGACY_COMPLETED_STORAGE_KEY = 'kronn:tour-completed';
const LEGACY_STEP_STORAGE_KEY = 'kronn:tour-step';
export const TOUR_PROGRESS_EVENT = 'kronn:tour-progress-changed';

interface StoredTourProgress {
  schemaVersion: 1;
  completedStepIds: string[];
  currentStepId: string | null;
  hasStarted: boolean;
  skippedStepIds: string[];
}

export interface TourProgress {
  completedStepIds: string[];
  completedCount: number;
  currentStepId: string | null;
  hasStarted: boolean;
  isComplete: boolean;
  resumeStepIndex: number;
  skippedStepIds: string[];
  totalSteps: number;
}

function uniqueKnownIds(ids: unknown, stepIds: string[]): string[] {
  if (!Array.isArray(ids)) return [];
  const known = new Set(stepIds);
  return [...new Set(ids.filter((id): id is string => typeof id === 'string' && known.has(id)))];
}

function normalizeProgress(
  value: Partial<StoredTourProgress>,
  stepIds: string[],
): StoredTourProgress {
  const completedStepIds = uniqueKnownIds(value.completedStepIds, stepIds);
  const completed = new Set(completedStepIds);
  const skippedStepIds = uniqueKnownIds(value.skippedStepIds, stepIds)
    .filter(id => !completed.has(id));
  const currentStepId = typeof value.currentStepId === 'string'
    && stepIds.includes(value.currentStepId)
    && !completedStepIds.includes(value.currentStepId)
    ? value.currentStepId
    : null;

  return {
    schemaVersion: 1,
    completedStepIds,
    currentStepId,
    hasStarted: typeof value.hasStarted === 'boolean'
      ? value.hasStarted
      : completedStepIds.length > 0 || currentStepId !== null,
    skippedStepIds,
  };
}

function migrateLegacyProgress(
  stepIds: string[],
  storage: Pick<Storage, 'getItem'>,
): StoredTourProgress {
  // A user who completed the previous tour should not be forced through the
  // replacement immediately. Persisting the current IDs (instead of keeping a
  // boolean forever) still lets a genuinely new step reactivate the CTA later.
  if (storage.getItem(LEGACY_COMPLETED_STORAGE_KEY) === 'true') {
    return {
      schemaVersion: 1,
      completedStepIds: [...stepIds],
      currentStepId: null,
      hasStarted: true,
      skippedStepIds: [],
    };
  }

  const legacyIndex = Number.parseInt(storage.getItem(LEGACY_STEP_STORAGE_KEY) ?? '', 10);
  if (Number.isFinite(legacyIndex) && legacyIndex >= 0 && legacyIndex < stepIds.length) {
    return {
      schemaVersion: 1,
      completedStepIds: stepIds.slice(0, legacyIndex),
      currentStepId: stepIds[legacyIndex],
      hasStarted: true,
      skippedStepIds: [],
    };
  }

  return {
    schemaVersion: 1,
    completedStepIds: [],
    currentStepId: stepIds[0] ?? null,
    hasStarted: false,
    skippedStepIds: [],
  };
}

function toPublicProgress(progress: StoredTourProgress, stepIds: string[]): TourProgress {
  const completed = new Set(progress.completedStepIds);
  const firstIncompleteIndex = stepIds.findIndex(id => !completed.has(id));
  const currentIndex = progress.currentStepId
    ? stepIds.indexOf(progress.currentStepId)
    : -1;
  const isComplete = stepIds.length > 0 && firstIncompleteIndex === -1;

  return {
    completedStepIds: [...progress.completedStepIds],
    completedCount: progress.completedStepIds.length,
    currentStepId: progress.currentStepId,
    hasStarted: progress.hasStarted,
    isComplete,
    resumeStepIndex: isComplete
      ? 0
      : currentIndex >= 0
        ? currentIndex
        : Math.max(0, firstIncompleteIndex),
    skippedStepIds: [...progress.skippedStepIds],
    totalSteps: stepIds.length,
  };
}

export function loadTourProgress(
  stepIds: string[],
  storage: Pick<Storage, 'getItem' | 'setItem'> = localStorage,
): TourProgress {
  let progress: StoredTourProgress | null = null;

  try {
    const raw = storage.getItem(TOUR_PROGRESS_STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<StoredTourProgress>;
      if (parsed.schemaVersion === 1) {
        progress = normalizeProgress(parsed, stepIds);
      }
    }
  } catch {
    // A corrupt or unavailable storage entry must never block the application.
  }

  if (!progress) {
    progress = migrateLegacyProgress(stepIds, storage);
  }

  try {
    storage.setItem(TOUR_PROGRESS_STORAGE_KEY, JSON.stringify(progress));
  } catch {
    // Private browsing and hardened WebViews may reject localStorage writes.
  }

  return toPublicProgress(progress, stepIds);
}

export function saveTourProgress(
  stepIds: string[],
  completedStepIds: string[],
  currentStepId: string | null,
  hasStarted = true,
  skippedStepIds: string[] = [],
  storage: Pick<Storage, 'setItem'> = localStorage,
): TourProgress {
  const progress = normalizeProgress({
    schemaVersion: 1,
    completedStepIds,
    currentStepId,
    hasStarted,
    skippedStepIds,
  }, stepIds);

  try {
    storage.setItem(TOUR_PROGRESS_STORAGE_KEY, JSON.stringify(progress));
  } catch {
    // Keep the in-memory state usable even when persistence is unavailable.
  }
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new Event(TOUR_PROGRESS_EVENT));
  }

  return toPublicProgress(progress, stepIds);
}

export function clearTourProgress(
  storage: Pick<Storage, 'removeItem'> = localStorage,
): void {
  try {
    storage.removeItem(TOUR_PROGRESS_STORAGE_KEY);
    storage.removeItem(LEGACY_COMPLETED_STORAGE_KEY);
    storage.removeItem(LEGACY_STEP_STORAGE_KEY);
  } catch {
    // No-op: replay still works for the current page session.
  }
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new Event(TOUR_PROGRESS_EVENT));
  }
}

export const TOUR_PROGRESS_KEYS = {
  current: TOUR_PROGRESS_STORAGE_KEY,
  legacyCompleted: LEGACY_COMPLETED_STORAGE_KEY,
  legacyStep: LEGACY_STEP_STORAGE_KEY,
} as const;
