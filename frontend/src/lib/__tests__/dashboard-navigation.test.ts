import { afterEach, describe, expect, it } from 'vitest';
import {
  readActiveDiscussionId,
  readDashboardPage,
  writeActiveDiscussionId,
  writeDashboardPage,
} from '../dashboard-navigation';

afterEach(() => {
  sessionStorage.clear();
});

describe('dashboard reload navigation checkpoint', () => {
  it('round-trips the active page and discussion inside the tab session', () => {
    writeDashboardPage('discussions');
    writeActiveDiscussionId('disc-42');

    expect(readDashboardPage()).toBe('discussions');
    expect(readActiveDiscussionId()).toBe('disc-42');
  });

  it('falls back safely when stored navigation was tampered with', () => {
    sessionStorage.setItem('kronn:navigation:page', 'not-a-page');
    sessionStorage.setItem('kronn:navigation:discussion', '   ');

    expect(readDashboardPage()).toBe('projects');
    expect(readActiveDiscussionId()).toBeNull();
  });

  it('removes a stale discussion checkpoint explicitly', () => {
    writeActiveDiscussionId('deleted-disc');
    writeActiveDiscussionId(null);

    expect(readActiveDiscussionId()).toBeNull();
  });
});
