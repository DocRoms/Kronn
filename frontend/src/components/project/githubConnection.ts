import type { GithubConnectionState, GithubScope } from '../../types/generated';
import { safeGetItem, safeSetItem } from '../../lib/safeStorage';

type T = (key: string, ...args: (string | number)[]) => string;

/** Synced through the server ui-preferences store (see uiPreferences.ts). */
export const GITHUB_NOTICE_DISMISSED_KEY = 'kronn:githubUpgradeNoticeDismissed';

export function githubStateTone(state: GithubConnectionState): 'success' | 'info' | 'muted' {
  if (state === 'connected_gh_login' || state === 'connected_stored_token') return 'success';
  if (state === 'available_but_off') return 'info';
  return 'muted';
}

export function isGithubConnected(state: GithubConnectionState): boolean {
  return state === 'connected_gh_login' || state === 'connected_stored_token';
}

const MAX_REPOSITORIES_SHOWN = 3;

/** One line saying what the token can reach, or why that is unknown. */
export function githubScopeSummary(scope: GithubScope | null | undefined, t: T): string {
  if (!scope) return t('github.scope.notChecked');
  if (!scope.verified) return t('github.scope.notVerified', scope.reason ?? '?');
  if (scope.token_kind === 'fine_grained' || scope.repositories.length > 0) {
    const shown = scope.repositories.slice(0, MAX_REPOSITORIES_SHOWN).join(', ');
    const more = scope.repositories.length > MAX_REPOSITORIES_SHOWN ? '…' : '';
    return scope.repositories_truncated
      ? t('github.scope.repositoriesMore', scope.repositories.length, `${shown}${more}`)
      : t('github.scope.repositories', scope.repositories.length, `${shown}${more}`);
  }
  return scope.scopes.length > 0
    ? t('github.scope.scopes', scope.scopes.join(', '))
    : t('github.scope.noScopes');
}

function readDismissed(): string[] {
  try {
    const parsed: unknown = JSON.parse(safeGetItem(GITHUB_NOTICE_DISMISSED_KEY) ?? '[]');
    return Array.isArray(parsed) ? parsed.filter((id): id is string => typeof id === 'string') : [];
  } catch {
    return [];
  }
}

export function isGithubNoticeDismissed(projectId: string): boolean {
  return readDismissed().includes(projectId);
}

export function dismissGithubNotice(projectId: string): void {
  const ids = readDismissed();
  if (!ids.includes(projectId)) safeSetItem(GITHUB_NOTICE_DISMISSED_KEY, JSON.stringify([...ids, projectId]));
}
