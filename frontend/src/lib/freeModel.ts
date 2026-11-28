/**
 * KT-2010 — whether a model is free. The catalogue entry carries no price, so
 * the model's own words are the only fact, and the providers spell it several
 * ways (owner's catalogue, 2026-11-22): OpenRouter's variants end with `:free`
 * (`vendor/model:free`) and its router is `openrouter/free`; OpenCode's free
 * models end with `-free` (`opencode-go/beam-free`); one OpenCode model says
 * it in its display name only ("OpenCode Zen/Union Alpha Free").
 *
 * So the id's last word decides: the trimmed id, case-insensitive, is `free`
 * or ends with `free` right after one of `:`, `-`, `_`, `/`. `freedom-1`,
 * `vendor/freeze`, `carefree` and `model-free-preview` are not free. When the
 * catalogue entry is at hand, a display name whose last word is `free`, with
 * or without parentheses ("… Alpha Free", "… (free)"), makes the model free
 * even when its id does not say so.
 *
 * Read for any agent or connection, not for one named OpenRouter or OpenCode:
 * a proxy that lists the same ids says the same thing. If a provider later
 * states prices in its listing, the fact belongs to the catalogue entry,
 * server-side.
 */
const FREE_ID = /(?:^|[:\-_/])free$/;
const FREE_NAME = /(?:^|[\s:\-_/])\(?free\)?$/;

/** Whether a display name's last word is `free`, bare or in parentheses. */
export function isFreeDisplayName(displayName: string | null | undefined): boolean {
  const name = displayName?.trim().toLowerCase() ?? '';
  return FREE_NAME.test(name);
}

/**
 * Whether `modelId` names a free model: its last word (after `:`, `-`, `_` or
 * `/`, or the whole id) is `free`; or, given the catalogue entry's
 * `displayName`, that name's last word is `free`.
 */
export function isFreeModel(modelId: string | null | undefined, displayName?: string | null): boolean {
  const id = modelId?.trim().toLowerCase() ?? '';
  if (!id) return false;
  return FREE_ID.test(id) || isFreeDisplayName(displayName);
}
