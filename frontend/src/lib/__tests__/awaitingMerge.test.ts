import { describe, expect, it } from 'vitest';
import { awaitingMergeTitle, shortBranch } from '../awaitingMerge';

describe('awaiting merge label', () => {
  it('keeps a short branch whole', () => {
    expect(shortBranch('autocode/kt-1495')).toBe('autocode/kt-1495');
  });

  it('cuts a long branch from the front, keeping the part that tells branches apart', () => {
    const short = shortBranch('autocode/campaign/kt-1519-tasks-are-marked-done-too-early');
    expect(short.length).toBeLessThanOrEqual(24);
    expect(short.startsWith('…')).toBe(true);
    expect(short.endsWith('done-too-early')).toBe(true);
  });

  it('names the full branch and the abbreviated commit on the title', () => {
    expect(awaitingMergeTitle({ branch: 'autocode/kt-1495', sha: '0123456789abcdef' })).toBe(
      'autocode/kt-1495 @ 01234567',
    );
  });
});
