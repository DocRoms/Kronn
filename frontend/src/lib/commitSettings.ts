// KT-1750 — the commit identity and sign-off of Kronn's commits, as the two forms
// that set them hold it: Settings > Workers (every project) and the project card
// (one project, overriding the first). The identity is both fields or none.
import type { ProjectCommitSettings, SignoffSetting } from '../types/generated';

export const SIGNOFF_SETTINGS: SignoffSetting[] = ['auto', 'always', 'never'];

/** What a form field holds for a stored value. */
export function commitFields(settings: ProjectCommitSettings | undefined): {
  name: string;
  email: string;
  signoff: SignoffSetting;
} {
  return {
    name: settings?.name ?? '',
    email: settings?.email ?? '',
    signoff: settings?.signoff ?? 'auto',
  };
}

/** A name without an e-mail, or the reverse: the backend refuses it. */
export function isHalfIdentity(name: string, email: string): boolean {
  return (name.trim() === '') !== (email.trim() === '');
}

/** Whether the fields hold what `settings` stores. */
export function sameCommitSettings(
  fields: { name: string; email: string; signoff: SignoffSetting },
  settings: ProjectCommitSettings | undefined,
): boolean {
  const stored = commitFields(settings);
  return fields.name.trim() === stored.name
    && fields.email.trim() === stored.email
    && fields.signoff === stored.signoff;
}

/**
 * The settings to store for the fields: trimmed, an empty field as unset, and
 * `undefined` for nothing at all (the backend stores no key then).
 */
export function commitSettingsOf(fields: {
  name: string;
  email: string;
  signoff: SignoffSetting;
}): ProjectCommitSettings | undefined {
  const name = fields.name.trim();
  const email = fields.email.trim();
  if (name === '' && email === '' && fields.signoff === 'auto') return undefined;
  return {
    ...(name !== '' ? { name } : {}),
    ...(email !== '' ? { email } : {}),
    signoff: fields.signoff,
  };
}
