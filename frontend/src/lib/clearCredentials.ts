// KT-1594 — how a credential found in clear in config.toml is named on screen.
// The backend sends names only, never values.
import type { ClearCredentialView } from '../types/generated';

type T = (key: string, ...args: (string | number)[]) => string;

/** `Anthropic`, `anthropic`, or the generic kind when the entry has no name. */
export function clearCredentialLabel(c: ClearCredentialView, t: T): string {
  if (c.kind === 'auth_token') return t('settings.recovery.authToken');
  return c.name || c.provider || t('settings.recovery.providerKey');
}
