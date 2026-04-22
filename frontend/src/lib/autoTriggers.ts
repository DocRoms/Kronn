import type { AutoTriggers, Skill } from '../types/generated';

/** Fallback locale when the skill declares patterns for one language but
 *  the current discussion is in another. English is the lingua franca
 *  of Kronn's skill library — nearly every skill has an `en` bucket, so
 *  it's the safest fallback for unknown locales. */
const FALLBACK_LOCALE = 'en';

/** Flatten a skill's `auto_triggers` into the single list that applies
 *  to the active locale: `common` patterns always count, plus the
 *  locale-specific bucket (falling back to `en` when the current locale
 *  has no entries). A skill without any triggers yields an empty list. */
export function selectTriggers(
  triggers: AutoTriggers | null | undefined,
  locale: string,
): string[] {
  if (!triggers) return [];
  const common = triggers.common ?? [];
  const byLocale = triggers.locales ?? {};
  const localized = byLocale[locale] ?? byLocale[FALLBACK_LOCALE] ?? [];
  return [...common, ...localized];
}

/** Pre-compile each pattern once so we don't rebuild regexes per message.
 *  Invalid regexes are silently dropped (logged in dev) — a bad trigger
 *  in one skill must not break the whole matcher. The `iu` flags are
 *  fixed: case-insensitive + full Unicode so French/Spanish accents +
 *  emoji word-boundaries work. */
function compile(patterns: string[]): RegExp[] {
  const out: RegExp[] = [];
  for (const p of patterns) {
    try {
      out.push(new RegExp(p, 'iu'));
    } catch (e) {
      if (typeof console !== 'undefined') {
        console.warn('[auto-triggers] invalid regex skipped:', p, e);
      }
    }
  }
  return out;
}

/** Return skills whose triggers match the message AND that are not yet
 *  active on the discussion AND whose auto-activation has not been
 *  disabled by the operator (Settings > Skills > ⚡ toggle). Order
 *  matches `skills` input. */
export function detectTriggeredSkills(
  message: string,
  skills: Skill[],
  activeSkillIds: readonly string[],
  locale: string,
  disabledSkillIds: ReadonlySet<string> = new Set(),
): Skill[] {
  const active = new Set(activeSkillIds);
  return skills.filter(skill => {
    if (active.has(skill.id)) return false;
    if (disabledSkillIds.has(skill.id)) return false;
    const patterns = selectTriggers(skill.auto_triggers, locale);
    if (patterns.length === 0) return false;
    const regexes = compile(patterns);
    return regexes.some(r => r.test(message));
  });
}
