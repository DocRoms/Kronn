import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  clearReplyDraft,
  loadReplyDraft,
  saveReplyDraft,
} from '../chat-reply-drafts';

describe('chat reply drafts', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-07-28T12:00:00Z'));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('persists the target and pending client message across a reload', () => {
    saveReplyDraft('disc-1', 'source-1', 'client-1');
    expect(loadReplyDraft('disc-1')).toMatchObject({
      messageId: 'source-1',
      pendingClientMessageId: 'client-1',
    });
  });

  it('does not let an older receipt clear a newer reply selection', () => {
    saveReplyDraft('disc-1', 'source-1', 'client-1');
    saveReplyDraft('disc-1', 'source-2');
    expect(clearReplyDraft('disc-1', 'client-1')).toBe(false);
    expect(loadReplyDraft('disc-1')?.messageId).toBe('source-2');
  });

  it('clears only the matching pending send', () => {
    saveReplyDraft('disc-1', 'source-1', 'client-1');
    expect(clearReplyDraft('disc-1', 'client-1')).toBe(true);
    expect(loadReplyDraft('disc-1')).toBeNull();
  });

  it('drops stale reply selections', () => {
    saveReplyDraft('disc-1', 'source-1');
    expect(loadReplyDraft('disc-1', new Date('2026-08-05T12:00:00Z'))).toBeNull();
  });
});
