const DEFAULT_PROJECT_KEY = 'kronn:new-discussion:default-project';

export function loadDefaultDiscussionProject(): string {
  try {
    return localStorage.getItem(DEFAULT_PROJECT_KEY) ?? '';
  } catch {
    return '';
  }
}

export function saveDefaultDiscussionProject(projectId: string | null): void {
  try {
    if (projectId) {
      localStorage.setItem(DEFAULT_PROJECT_KEY, projectId);
    } else {
      localStorage.removeItem(DEFAULT_PROJECT_KEY);
    }
  } catch {
    // Storage can be disabled or full. The current form selection still works.
  }
}

export const NEW_DISCUSSION_PREFERENCES = {
  DEFAULT_PROJECT_KEY,
};
