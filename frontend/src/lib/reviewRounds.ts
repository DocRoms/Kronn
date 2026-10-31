// KT-1345 — the review-round limit. « Unlimited » is stored as a sentinel in the
// `max_review_rounds` columns (never 0): the backend's `UNLIMITED_REVIEW_ROUNDS`.

export const UNLIMITED_REVIEW_ROUNDS = 4_294_967_295;

/** Smallest and largest explicit limit the backend accepts. */
export const MIN_REVIEW_ROUNDS = 1;
export const MAX_REVIEW_ROUNDS = 1_000;
/** Out-of-the-box default, mirrored from `DEFAULT_MAX_REVIEW_ROUNDS`. */
export const DEFAULT_REVIEW_ROUNDS = 5;

export function isUnlimitedRounds(limit: number | null | undefined): boolean {
  return limit !== null && limit !== undefined && limit >= UNLIMITED_REVIEW_ROUNDS;
}

/** `3` for a cap, `∞` for the sentinel. */
export function roundsLimitLabel(limit: number): string {
  return isUnlimitedRounds(limit) ? '∞' : String(limit);
}

/** The explicit limit the form can edit, clamped to what the backend accepts. */
export function clampRounds(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_REVIEW_ROUNDS;
  return Math.min(MAX_REVIEW_ROUNDS, Math.max(MIN_REVIEW_ROUNDS, Math.trunc(value)));
}
