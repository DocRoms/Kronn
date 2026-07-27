const REPLY_DRAFT_KEY_PREFIX = 'kronn:reply-draft:';
const MAX_REPLY_DRAFT_AGE_MS = 7 * 24 * 60 * 60 * 1000;

export interface ReplyDraft {
  messageId: string;
  pendingClientMessageId?: string;
  savedAt: string;
}

function storageKey(discussionId: string): string {
  return `${REPLY_DRAFT_KEY_PREFIX}${discussionId}`;
}

export function saveReplyDraft(
  discussionId: string,
  messageId: string,
  pendingClientMessageId?: string,
): void {
  if (!discussionId || !messageId) return;
  try {
    localStorage.setItem(storageKey(discussionId), JSON.stringify({
      messageId,
      pendingClientMessageId,
      savedAt: new Date().toISOString(),
    } satisfies ReplyDraft));
  } catch {
    // localStorage unavailable or full: the in-memory reply state still works.
  }
}

export function loadReplyDraft(
  discussionId: string,
  now: Date = new Date(),
): ReplyDraft | null {
  if (!discussionId) return null;
  try {
    const raw = localStorage.getItem(storageKey(discussionId));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<ReplyDraft> | null;
    if (!parsed || typeof parsed !== 'object') return null;
    if (typeof parsed.messageId !== 'string' || !parsed.messageId) return null;
    if (typeof parsed.savedAt !== 'string') return null;
    if (parsed.pendingClientMessageId !== undefined
      && typeof parsed.pendingClientMessageId !== 'string') return null;
    const savedMs = Date.parse(parsed.savedAt);
    if (Number.isNaN(savedMs) || now.getTime() - savedMs > MAX_REPLY_DRAFT_AGE_MS) {
      localStorage.removeItem(storageKey(discussionId));
      return null;
    }
    return {
      messageId: parsed.messageId,
      pendingClientMessageId: parsed.pendingClientMessageId,
      savedAt: parsed.savedAt,
    };
  } catch {
    return null;
  }
}

/**
 * Clear the reply selection. When `pendingClientMessageId` is supplied, only
 * clear the matching in-flight send so a newer reply selection is preserved.
 */
export function clearReplyDraft(
  discussionId: string,
  pendingClientMessageId?: string,
): boolean {
  if (!discussionId) return false;
  try {
    if (pendingClientMessageId) {
      const current = loadReplyDraft(discussionId);
      if (current?.pendingClientMessageId !== pendingClientMessageId) return false;
    }
    localStorage.removeItem(storageKey(discussionId));
    return true;
  } catch {
    return false;
  }
}

export const CHAT_REPLY_DRAFT_CONFIG = {
  KEY_PREFIX: REPLY_DRAFT_KEY_PREFIX,
  MAX_AGE_MS: MAX_REPLY_DRAFT_AGE_MS,
};
