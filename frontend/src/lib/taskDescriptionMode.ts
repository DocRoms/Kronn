export type TaskDescriptionMode = 'rendered' | 'raw';

export const TASK_DESCRIPTION_MODE_KEY = 'kronn:planning.descriptionMode';

export function readTaskDescriptionMode(): TaskDescriptionMode {
  try {
    return localStorage.getItem(TASK_DESCRIPTION_MODE_KEY) === 'raw' ? 'raw' : 'rendered';
  } catch {
    return 'rendered';
  }
}

export function writeTaskDescriptionMode(mode: TaskDescriptionMode) {
  try {
    localStorage.setItem(TASK_DESCRIPTION_MODE_KEY, mode);
  } catch {
    // Storage can be blocked; the mode then lasts for this view only.
  }
}
