import { afterEach, describe, expect, it } from 'vitest';
import { readActiveDiscussionId, writeActiveDiscussionId } from '../dashboard-navigation';

afterEach(() => {
  sessionStorage.clear();
});

describe('open discussion reload checkpoint', () => {
  it('round-trips the open discussion inside the tab session', () => {
    writeActiveDiscussionId('disc-42');

    expect(readActiveDiscussionId()).toBe('disc-42');
  });

  it('starts without a discussion', () => {
    expect(readActiveDiscussionId()).toBeNull();
  });

  it('falls back safely when the stored discussion was tampered with', () => {
    sessionStorage.setItem('kronn:navigation:discussion', '   ');

    expect(readActiveDiscussionId()).toBeNull();
  });

  it('removes a stale discussion checkpoint explicitly', () => {
    writeActiveDiscussionId('deleted-disc');
    writeActiveDiscussionId(null);

    expect(readActiveDiscussionId()).toBeNull();
  });

  it('never keeps a page: the address is what restores it', () => {
    writeActiveDiscussionId('disc-42');

    expect(sessionStorage.getItem('kronn:navigation:page')).toBeNull();
  });
});
