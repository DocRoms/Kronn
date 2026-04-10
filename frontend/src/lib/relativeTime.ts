/**
 * Format a date as a compact relative / absolute string for the discussion
 * sidebar. We use native `Intl.RelativeTimeFormat` + `Intl.DateTimeFormat`
 * so there's no new runtime dependency, and the output follows the user's
 * active Kronn language (FR/EN/ES).
 *
 * Rules:
 * - < 60s → "à l'instant" / "just now" / "ahora"
 * - < 60m → "il y a 5 min" / "5m ago" / "hace 5 min"
 * - < 24h → "il y a 3 h"  / "3h ago" / "hace 3 h"
 * - < 7d  → "hier" / "yesterday" / "ayer" when exactly 1 day,
 *           otherwise "il y a 4 j" / "4d ago" / "hace 4 d"
 * - ≥ 7d  → short date "5 avr." / "Apr 5" / "5 abr."
 *
 * The returned string is short enough to fit in a single meta row
 * (`12 msg · ClaudeCode · il y a 5 min`).
 */
export function formatRelativeTime(iso: string, lang: string = 'fr'): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  const now = Date.now();
  const diffMs = now - d.getTime();
  const diffSec = Math.round(diffMs / 1000);
  const diffMin = Math.round(diffSec / 60);
  const diffHour = Math.round(diffMin / 60);
  const diffDay = Math.round(diffHour / 24);

  // Normalize lang to a BCP 47 tag Intl accepts
  const locale = lang === 'en' ? 'en' : lang === 'es' ? 'es' : 'fr';

  // Future dates (clock skew, etc.) → treat as "just now"
  if (diffSec < 60) {
    if (locale === 'en') return 'just now';
    if (locale === 'es') return 'ahora';
    return "à l'instant";
  }

  if (diffMin < 60) {
    if (locale === 'en') return `${diffMin}m ago`;
    if (locale === 'es') return `hace ${diffMin} min`;
    return `il y a ${diffMin} min`;
  }

  if (diffHour < 24) {
    if (locale === 'en') return `${diffHour}h ago`;
    if (locale === 'es') return `hace ${diffHour} h`;
    return `il y a ${diffHour} h`;
  }

  if (diffDay < 7) {
    if (diffDay === 1) {
      if (locale === 'en') return 'yesterday';
      if (locale === 'es') return 'ayer';
      return 'hier';
    }
    if (locale === 'en') return `${diffDay}d ago`;
    if (locale === 'es') return `hace ${diffDay} d`;
    return `il y a ${diffDay} j`;
  }

  // ≥ 7 days → short absolute date. Include year only if different from current.
  const nowD = new Date(now);
  const sameYear = nowD.getFullYear() === d.getFullYear();
  try {
    return new Intl.DateTimeFormat(locale, {
      day: 'numeric',
      month: 'short',
      ...(sameYear ? {} : { year: 'numeric' }),
    }).format(d);
  } catch {
    return d.toISOString().slice(0, 10);
  }
}
