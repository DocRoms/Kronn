/**
 * Per-discussion chat-input drafts (localStorage).
 *
 * Solves the bug where the ChatInput textarea (non-controlled, DOM-ref
 * based for perf) loses its content when the user navigates away from
 * the Discussions tab and comes back — the component remounts, the
 * ref loses its DOM value. Now each keystroke is mirrored (throttled
 * at the caller) to `localStorage['kronn:draft:<discussion_id>']`, and
 * restored on remount.
 *
 * Storage shape (one entry per discussion):
 *   { v: 1, text: string, savedAt: ISO-8601 }
 * The `v` field lets future schema changes coexist with old browsers
 * that still have pre-migration drafts.
 *
 * Expiration: drafts older than `MAX_DRAFT_AGE_MS` (7 days) are ignored
 * on load and dropped by `purgeExpiredDrafts` so we don't leak storage.
 * Dead drafts for deleted discussions are also cleaned by
 * `purgeExpiredDrafts` once the caller supplies the current live ids.
 */

const DRAFT_KEY_PREFIX = 'kronn:draft:';
const MAX_DRAFT_AGE_MS = 7 * 24 * 60 * 60 * 1000; // 7 days
const DRAFT_SCHEMA_VERSION = 1;

export interface DraftRecord {
  text: string;
  savedAt: string; // ISO-8601
}

interface StoredDraft {
  v: number;
  text: string;
  savedAt: string;
}

function storageKey(discussionId: string): string {
  return DRAFT_KEY_PREFIX + discussionId;
}

/**
 * Persist the current draft text for a discussion. Called from the
 * textarea onChange handler, throttled by the caller (typical: 250ms).
 *
 * Passing an empty string removes the entry — no point keeping a
 * zero-length draft around.
 */
export function saveDraft(discussionId: string, text: string): void {
  if (!discussionId) return;
  try {
    if (!text) {
      localStorage.removeItem(storageKey(discussionId));
      return;
    }
    const payload: StoredDraft = {
      v: DRAFT_SCHEMA_VERSION,
      text,
      savedAt: new Date().toISOString(),
    };
    localStorage.setItem(storageKey(discussionId), JSON.stringify(payload));
  } catch {
    // Quota exceeded / disabled storage — drop silently, the draft is
    // still in the DOM for the current session.
  }
}

/**
 * Load the saved draft for a discussion, or null if none / expired /
 * malformed. Expired drafts are removed as a side-effect so repeated
 * navigations to a stale discussion don't keep returning the same
 * outdated text.
 */
export function loadDraft(discussionId: string, now: Date = new Date()): DraftRecord | null {
  if (!discussionId) return null;
  try {
    const raw = localStorage.getItem(storageKey(discussionId));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<StoredDraft> | null;
    if (!parsed || typeof parsed !== 'object') return null;
    if (parsed.v !== DRAFT_SCHEMA_VERSION) return null;
    if (typeof parsed.text !== 'string' || !parsed.text) return null;
    if (typeof parsed.savedAt !== 'string') return null;

    const savedMs = Date.parse(parsed.savedAt);
    if (Number.isNaN(savedMs)) return null;

    if (now.getTime() - savedMs > MAX_DRAFT_AGE_MS) {
      // Fall through to purge; loader returns null so callers don't
      // accidentally restore ancient content.
      localStorage.removeItem(storageKey(discussionId));
      return null;
    }

    return { text: parsed.text, savedAt: parsed.savedAt };
  } catch {
    return null;
  }
}

/** Remove the draft for a discussion. Called right after a successful send. */
export function clearDraft(discussionId: string): void {
  if (!discussionId) return;
  try {
    localStorage.removeItem(storageKey(discussionId));
  } catch {
    // ignore
  }
}

/**
 * Clean up drafts that are expired OR tied to a discussion that no
 * longer exists. Pass the set of currently-alive discussion ids; drafts
 * pointing anywhere else are removed.
 *
 * No-op when `liveIds` is undefined — used in tests where the caller
 * only wants to flush expired entries without checking liveness.
 */
export function purgeExpiredDrafts(liveIds?: ReadonlySet<string>, now: Date = new Date()): void {
  try {
    // Snapshot the keys first — removeItem mutates the iteration target
    // and we'd otherwise skip half the entries on some browsers.
    const keys: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k && k.startsWith(DRAFT_KEY_PREFIX)) keys.push(k);
    }

    for (const key of keys) {
      const discussionId = key.slice(DRAFT_KEY_PREFIX.length);
      let drop = false;

      if (liveIds && !liveIds.has(discussionId)) {
        drop = true;
      } else {
        // Read + check expiry (cheap — one JSON parse per draft).
        const raw = localStorage.getItem(key);
        if (!raw) continue;
        try {
          const parsed = JSON.parse(raw) as Partial<StoredDraft> | null;
          const savedMs = parsed?.savedAt ? Date.parse(parsed.savedAt) : NaN;
          if (Number.isNaN(savedMs) || now.getTime() - savedMs > MAX_DRAFT_AGE_MS) {
            drop = true;
          }
        } catch {
          drop = true; // malformed → take it out
        }
      }

      if (drop) {
        try { localStorage.removeItem(key); } catch { /* ignore */ }
      }
    }
  } catch {
    // localStorage unavailable — nothing to purge.
  }
}

/** Exported constants for tests and for callers that need the TTL. */
export const CHAT_DRAFT_CONFIG = {
  KEY_PREFIX: DRAFT_KEY_PREFIX,
  MAX_AGE_MS: MAX_DRAFT_AGE_MS,
  SCHEMA_VERSION: DRAFT_SCHEMA_VERSION,
};
