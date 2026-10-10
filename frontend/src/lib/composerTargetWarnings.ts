import type { TargetReachability } from './api';
import type { ResolvedCatalogTier } from './modelCatalogSelection';

export type ComposerTargetWarning =
  | { kind: 'unreachable'; reason: string; checkedAt: string | null }
  | { kind: 'refused'; model: string; reason: string | null; since: string | null };

/**
 * What a mentioned agent would hit if the message left now: an endpoint the
 * last probe could not reach first, then a model the catalogue refuses.
 */
export function composerTargetWarning(
  reachability: TargetReachability | undefined,
  resolved: ResolvedCatalogTier | null | undefined,
): ComposerTargetWarning | null {
  const connectivity = reachability?.connectivity;
  if (connectivity?.state === 'unreachable') {
    return {
      kind: 'unreachable',
      reason: connectivity.unreachable_reason ?? 'connect',
      checkedAt: connectivity.checked_at ?? null,
    };
  }
  if (resolved?.unavailable) {
    return {
      kind: 'refused',
      model: resolved.configured || resolved.entry?.model_id || '',
      reason: resolved.entry?.unavailable_reason ?? null,
      // The catalogue's own date: a refusal remembered for days says so.
      since: resolved.entry?.last_checked_at ?? null,
    };
  }
  return null;
}

/** `YYYY-MM-DD HH:MM` in the viewer's time zone; the raw value if unparsable. */
export function formatWarningDate(value: string | null): string {
  if (!value) return '';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
