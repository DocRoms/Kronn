// KT-619 — what a message must carry to publish a steering card, and what to
// do when it cannot.
//
// Extracted from the composer's send handler for one reason: it is the only
// place in that handler with an `await` between pressing send and the message
// going out, which makes it the only place a Stop can be followed by a send
// anyway. A rule with a race in it belongs somewhere it can be tested without
// mounting a page.

import type { SendMessageRequest } from '../types/generated';

/** The two fields a publication adds to a send. Absent means "no card". */
export type PublicationFields = Pick<
  SendMessageRequest,
  'publication_grant' | 'publication_proof'
>;

export type PreparedPublication =
  | { outcome: 'send'; fields: PublicationFields }
  /** The caller stopped while the proof was in flight. Send nothing. */
  | { outcome: 'abandon' };

/** The marker that turns a message into a publication request. */
export const IMPORTANT_FENCE = 'kronn-important';

export interface PreparePublicationOptions {
  /** The credential the person typed. Empty means they are not publishing. */
  grant: string;
  /** The exact body about to be sent — the proof is bound to it. */
  content: string;
  discussionId: string;
  /** The send's own abort signal, so a Stop is observed rather than assumed. */
  signal: AbortSignal;
  issueProof: (grant: string, discussionId: string, content: string) => Promise<string>;
  /** Told when the card is lost, so the screen can say so. */
  onRefused: () => void;
}

/**
 * Decide what a send carries.
 *
 * Three rules, and the third is the one worth stating:
 *
 * 1. no fence, or no credential → nothing is asked of the server. A person who
 *    has not filled the field is not publishing, and an ordinary message must
 *    not cost a round-trip;
 * 2. a refused proof loses the CARD, never the MESSAGE. Throwing away text
 *    somebody wrote because a publication right failed is the wrong trade, and
 *    "the message posts, the card does not" is the fail-closed state the
 *    contract describes;
 * 3. a Stop during the round-trip abandons the send. Checked AFTER the await,
 *    against the abort that may have happened while we were away — not the one
 *    that had not happened when we left.
 */
export async function preparePublication(
  options: PreparePublicationOptions,
): Promise<PreparedPublication> {
  const grant = options.grant.trim();
  if (options.signal.aborted) return { outcome: 'abandon' };
  if (!grant || !options.content.includes(IMPORTANT_FENCE)) {
    return { outcome: 'send', fields: {} };
  }
  let fields: PublicationFields = {};
  try {
    fields = {
      publication_grant: grant,
      publication_proof: await options.issueProof(grant, options.discussionId, options.content),
    };
  } catch {
    if (!options.signal.aborted) options.onRefused();
  }
  if (options.signal.aborted) return { outcome: 'abandon' };
  return { outcome: 'send', fields };
}
