import type { ValidationBlock, ValidationState } from '../types/generated';

/**
 * KT-1122 — what a validation discussion's banner offers, decided from the
 * server's verdict alone (never from the title or the text of a message).
 *
 *  - `superseded`: this discussion is not the validation of the latest run;
 *    it cannot validate anything, and says where the current one is.
 *  - `validate`: finished, and the server would accept "mark valid".
 *  - `blocked`: finished, but the server would refuse: show why.
 *  - `none`: nothing to offer yet (unfinished, unknown, or already validated).
 */
export type ValidationBanner =
  | { kind: 'none' }
  | { kind: 'superseded'; currentDiscussionId: string | null }
  | { kind: 'blocked'; block: ValidationBlock | null }
  | { kind: 'validate' };

export function validationBanner(
  state: ValidationState | null,
  discussionId: string,
  auditStatus: string | undefined,
): ValidationBanner {
  if (!state || auditStatus !== 'Audited') return { kind: 'none' };
  if (state.discussion_id !== discussionId) {
    return { kind: 'superseded', currentDiscussionId: state.discussion_id };
  }
  if (!state.finished) return { kind: 'none' };
  if (!state.can_validate) return { kind: 'blocked', block: state.blocked };
  return { kind: 'validate' };
}
