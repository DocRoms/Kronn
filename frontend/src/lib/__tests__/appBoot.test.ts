import { describe, expect, it, vi } from 'vitest';
import {
  cacheSetupStatus,
  clearCachedSetupStatus,
  readCachedSetupStatus,
} from '../appBoot';
import type { SetupStatus } from '../../types/generated';

const completeStatus: SetupStatus = {
  is_first_run: false,
  current_step: 'Complete',
  agents_detected: [],
  scan_paths_set: true,
  scan_paths_explored: ['/repos'],
  repos_detected: [],
  default_scan_path: '/repos',
};

describe('setup status boot cache', () => {
  it('round-trips the last server-confirmed status', () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: vi.fn((key: string) => values.get(key) ?? null),
      setItem: vi.fn((key: string, value: string) => { values.set(key, value); }),
      removeItem: vi.fn((key: string) => { values.delete(key); }),
    };

    cacheSetupStatus(completeStatus, storage);
    expect(readCachedSetupStatus(storage)).toEqual(completeStatus);
    clearCachedSetupStatus(storage);
    expect(readCachedSetupStatus(storage)).toBeNull();
  });

  it('ignores malformed or incomplete cached values', () => {
    expect(readCachedSetupStatus({ getItem: () => '{bad json' })).toBeNull();
    expect(readCachedSetupStatus({ getItem: () => JSON.stringify({ is_first_run: false }) })).toBeNull();
  });
});
