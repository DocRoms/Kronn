/**
 * Reload/HMR checkpoint of the open discussion.
 *
 * The page and the open discussion have addresses (`routes.ts`); this only
 * remembers which discussion to reopen from the bare `/discussions`.
 * `sessionStorage` is intentional: a full Vite reload in the current tab keeps
 * the user's place, while a genuinely new app/browser session starts on the
 * discussion list.
 *
 * An address does not go through here: `/discussions/<id>` opens that
 * discussion in any tab and takes precedence over this checkpoint — it is what
 * the reader asked for, while the checkpoint only says where the previous
 * visit left off. The checkpoint serves the bare `/discussions`.
 */

const DISCUSSION_KEY = 'kronn:navigation:discussion';

function storage(): Storage | null {
  try {
    return typeof sessionStorage === 'undefined' ? null : sessionStorage;
  } catch {
    return null;
  }
}

export function readActiveDiscussionId(): string | null {
  try {
    const value = storage()?.getItem(DISCUSSION_KEY)?.trim();
    return value || null;
  } catch {
    return null;
  }
}

export function writeActiveDiscussionId(discussionId: string | null): void {
  try {
    const target = storage();
    if (!target) return;
    if (discussionId) {
      target.setItem(DISCUSSION_KEY, discussionId);
    } else {
      target.removeItem(DISCUSSION_KEY);
    }
  } catch {
    // The in-memory selection remains usable when storage is unavailable.
  }
}
