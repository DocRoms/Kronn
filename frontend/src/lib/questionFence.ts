// KT-607 — why a `kronn-question` fence produced no card.
//
// The backend parses the fence strictly and, when it does not fit, writes
// nothing. From the room that reads as silence: the agent posts, no card
// appears, and neither the agent nor the human is told which field was wrong.
// It happened on the first day the notation existed — `"version":"1"`, a
// string where the contract wants the number 1, one character between a
// working card and nothing at all.
//
// This mirrors `backend/src/db/discussion_questions.rs::parse_spec` so the
// card can name the problem. It never decides whether a question is valid —
// the durable row does that — it only explains an absence.

/** The first thing wrong with the fence, as a translation key. */
export interface FenceProblem {
  key: string;
}

const STABLE_KEY = /^[A-Za-z0-9\-_.]{1,100}$/;

function isStableKey(value: unknown): value is string {
  return typeof value === 'string' && STABLE_KEY.test(value);
}

function isFilled(value: unknown, max: number): value is string {
  return typeof value === 'string' && value.trim().length > 0 && [...value].length <= max;
}

function isOptionalText(value: unknown, max: number): boolean {
  return value == null || (typeof value === 'string' && [...value].length <= max);
}

/**
 * `null` when the notation is fine — meaning the absence has another cause,
 * and the card must not blame the author for it.
 */
export function findFenceProblem(source: string | undefined): FenceProblem | null {
  // Nothing handed over is not the same as an empty fence: this explains the
  // notation it was GIVEN, and says nothing about a caller that gave none.
  if (source === undefined) return null;
  if (!source.trim()) return { key: 'disc.question.invalidEmpty' };

  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch {
    return { key: 'disc.question.invalidJson' };
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return { key: 'disc.question.invalidJson' };
  }
  const spec = parsed as Record<string, unknown>;

  // The one that actually happened, and the one a reader would never spot:
  // JSON tells a number and a string apart, and the contract wants the number.
  if (spec.version !== 1) {
    return typeof spec.version === 'string'
      ? { key: 'disc.question.invalidVersionString' }
      : { key: 'disc.question.invalidVersion' };
  }
  if (!isStableKey(spec.key)) return { key: 'disc.question.invalidKey' };
  if (!isFilled(spec.question, 1000)) return { key: 'disc.question.invalidQuestion' };
  if (!isOptionalText(spec.context, 4000)) return { key: 'disc.question.invalidContext' };
  if (spec.task_ref != null && !isFilled(spec.task_ref, 100)) {
    return { key: 'disc.question.invalidTaskRef' };
  }

  const options = spec.options ?? [];
  if (!Array.isArray(options) || options.length > 8) {
    return { key: 'disc.question.invalidOptions' };
  }
  const ids = new Set<string>();
  for (const raw of options) {
    if (typeof raw !== 'object' || raw === null) return { key: 'disc.question.invalidOptions' };
    const option = raw as Record<string, unknown>;
    if (!isStableKey(option.id)) return { key: 'disc.question.invalidOptionId' };
    if (!isFilled(option.label, 250)) return { key: 'disc.question.invalidOptionLabel' };
    if (!isOptionalText(option.description, 1000)) {
      return { key: 'disc.question.invalidOptionDescription' };
    }
    ids.add(option.id);
  }
  if (ids.size !== options.length) return { key: 'disc.question.invalidOptionId' };

  const recommended = spec.recommended_option_ids ?? [];
  if (!Array.isArray(recommended) || recommended.some(id => !ids.has(id as string))) {
    return { key: 'disc.question.invalidRecommended' };
  }
  const multiple = spec.multiple ?? false;
  if (typeof multiple !== 'boolean') return { key: 'disc.question.invalidMultiple' };
  // Recommending two answers to a question that accepts one is not a
  // recommendation, it is an ambiguity.
  if (!multiple && recommended.length > 1) {
    return { key: 'disc.question.invalidRecommendedSingle' };
  }

  return null;
}
