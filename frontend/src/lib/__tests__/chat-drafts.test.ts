import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  saveDraft,
  loadDraft,
  clearDraft,
  purgeExpiredDrafts,
  CHAT_DRAFT_CONFIG,
} from '../chat-drafts';

describe('chat-drafts', () => {
  beforeEach(() => {
    localStorage.clear();
    // Frozen clock so "savedAt" is deterministic.
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-04-15T09:00:00Z'));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  // ─── save / load round-trip ───────────────────────────────────────────

  it('saves a draft and reloads the same text', () => {
    saveDraft('d1', 'hello world');
    const got = loadDraft('d1');
    expect(got).not.toBeNull();
    expect(got!.text).toBe('hello world');
    expect(got!.savedAt).toBe('2026-04-15T09:00:00.000Z');
  });

  it('isolates drafts per discussion id', () => {
    saveDraft('d1', 'one');
    saveDraft('d2', 'two');
    expect(loadDraft('d1')!.text).toBe('one');
    expect(loadDraft('d2')!.text).toBe('two');
  });

  it('saving empty text removes the stored entry (no zero-length drafts)', () => {
    saveDraft('d1', 'text');
    saveDraft('d1', '');
    expect(loadDraft('d1')).toBeNull();
    expect(localStorage.getItem(CHAT_DRAFT_CONFIG.KEY_PREFIX + 'd1')).toBeNull();
  });

  it('clearDraft removes the entry', () => {
    saveDraft('d1', 'sent');
    clearDraft('d1');
    expect(loadDraft('d1')).toBeNull();
  });

  it('unknown discussion id returns null', () => {
    expect(loadDraft('never-saved')).toBeNull();
  });

  it('ignores empty discussion ids safely', () => {
    expect(() => saveDraft('', 'x')).not.toThrow();
    expect(() => clearDraft('')).not.toThrow();
    expect(loadDraft('')).toBeNull();
  });

  // ─── expiration (7-day TTL) ───────────────────────────────────────────

  it('drafts older than 7 days are treated as absent', () => {
    saveDraft('d1', 'old content');
    // Jump 8 days forward.
    const future = new Date('2026-04-23T09:00:00Z');
    expect(loadDraft('d1', future)).toBeNull();
  });

  it('expired drafts are removed from storage as a side-effect of load', () => {
    saveDraft('d1', 'old');
    const future = new Date('2026-04-23T09:00:00Z');
    loadDraft('d1', future);
    expect(localStorage.getItem(CHAT_DRAFT_CONFIG.KEY_PREFIX + 'd1')).toBeNull();
  });

  it('drafts still valid at the 7-day edge boundary are preserved', () => {
    saveDraft('d1', 'edge');
    // Exactly at the TTL — still loadable (MAX_AGE_MS is inclusive upper bound).
    const edge = new Date('2026-04-15T09:00:00Z').getTime() + CHAT_DRAFT_CONFIG.MAX_AGE_MS;
    expect(loadDraft('d1', new Date(edge))).not.toBeNull();
    // One ms past the boundary → gone.
    expect(loadDraft('d1', new Date(edge + 1))).toBeNull();
  });

  // ─── corruption / forward-compat ──────────────────────────────────────

  it('malformed JSON is ignored and returned as null', () => {
    localStorage.setItem(CHAT_DRAFT_CONFIG.KEY_PREFIX + 'd1', '{not valid json');
    expect(loadDraft('d1')).toBeNull();
  });

  it('unknown schema version is ignored (forward-compat)', () => {
    localStorage.setItem(
      CHAT_DRAFT_CONFIG.KEY_PREFIX + 'd1',
      JSON.stringify({ v: 99, text: 'future', savedAt: new Date().toISOString() }),
    );
    expect(loadDraft('d1')).toBeNull();
  });

  it('missing required fields are rejected', () => {
    localStorage.setItem(
      CHAT_DRAFT_CONFIG.KEY_PREFIX + 'd1',
      JSON.stringify({ v: 1, savedAt: new Date().toISOString() }), // text missing
    );
    expect(loadDraft('d1')).toBeNull();
  });

  // ─── purgeExpiredDrafts ───────────────────────────────────────────────

  it('purgeExpiredDrafts removes expired drafts', () => {
    saveDraft('d1', 'fresh');
    vi.setSystemTime(new Date('2026-04-01T09:00:00Z'));
    saveDraft('d-old', 'stale');
    vi.setSystemTime(new Date('2026-04-15T09:00:00Z')); // 14 days later

    purgeExpiredDrafts();

    expect(loadDraft('d1')).not.toBeNull();
    expect(localStorage.getItem(CHAT_DRAFT_CONFIG.KEY_PREFIX + 'd-old')).toBeNull();
  });

  it('purgeExpiredDrafts removes drafts for discussions no longer alive', () => {
    saveDraft('alive', 'keep');
    saveDraft('deleted', 'drop');
    purgeExpiredDrafts(new Set(['alive']));
    expect(loadDraft('alive')).not.toBeNull();
    expect(loadDraft('deleted')).toBeNull();
  });

  it('purgeExpiredDrafts does not touch non-draft keys', () => {
    localStorage.setItem('kronn:ttsEnabled', 'true');
    localStorage.setItem('unrelated:key', 'hello');
    saveDraft('d1', 'keep');
    purgeExpiredDrafts();
    expect(localStorage.getItem('kronn:ttsEnabled')).toBe('true');
    expect(localStorage.getItem('unrelated:key')).toBe('hello');
    expect(loadDraft('d1')).not.toBeNull();
  });

  it('purgeExpiredDrafts handles malformed entries by removing them', () => {
    localStorage.setItem(CHAT_DRAFT_CONFIG.KEY_PREFIX + 'broken', 'not json');
    saveDraft('d1', 'good');
    purgeExpiredDrafts();
    expect(localStorage.getItem(CHAT_DRAFT_CONFIG.KEY_PREFIX + 'broken')).toBeNull();
    expect(loadDraft('d1')).not.toBeNull();
  });
});
