import { safeRemoveItem, safeSetItem } from './safeStorage';

const DEFAULT_PROJECT_KEY = 'kronn:new-discussion:default-project';

export function loadDefaultDiscussionProject(): string {
  try {
    return localStorage.getItem(DEFAULT_PROJECT_KEY) ?? '';
  } catch {
    return '';
  }
}

export function saveDefaultDiscussionProject(projectId: string | null): void {
  // Storage can be disabled or full; the current form selection still works.
  if (projectId) safeSetItem(DEFAULT_PROJECT_KEY, projectId);
  else safeRemoveItem(DEFAULT_PROJECT_KEY);
}

export const NEW_DISCUSSION_PREFERENCES = {
  DEFAULT_PROJECT_KEY,
};
