// Mirrors `backend/core/versions.rs` test cases — keep them in sync on
// each release. When the Rust comparator's behavior changes, this file
// must change too (the agent freshness pill and the backend `rtk version`
// endpoint share the rule).
//
// Coverage rationale: strict patch bump, minor/major bump, equal, ahead,
// pre-release suffix, build metadata, unparsable, zero-pad. Same eight
// cases as the Rust side.

import { describe, it, expect } from 'vitest';
import { isUpdateAvailable } from '../version';

describe('isUpdateAvailable', () => {
  it('detects a strict patch bump', () => {
    expect(isUpdateAvailable('1.2.3', '1.2.4')).toBe(true);
    expect(isUpdateAvailable('1.2', '1.2.1')).toBe(true);
  });

  it('detects minor and major bumps', () => {
    expect(isUpdateAvailable('1.2.99', '1.3.0')).toBe(true);
    expect(isUpdateAvailable('1.99.99', '2.0.0')).toBe(true);
  });

  it('treats equal versions as up to date', () => {
    expect(isUpdateAvailable('1.2.3', '1.2.3')).toBe(false);
    expect(isUpdateAvailable('v1.2.3', '1.2.3')).toBe(false);
    expect(isUpdateAvailable('1.2.3', 'v1.2.3')).toBe(false);
  });

  it('does not nag a user pinned to a future release', () => {
    expect(isUpdateAvailable('2.0.0', '1.99.99')).toBe(false);
  });

  it('ignores pre-release suffixes', () => {
    expect(isUpdateAvailable('0.37.2-rc1', '0.37.2')).toBe(false);
    expect(isUpdateAvailable('0.37.1-rc1', '0.37.2')).toBe(true);
  });

  it('ignores build metadata suffixes', () => {
    expect(isUpdateAvailable('1.2.3+sha.abc', '1.2.3')).toBe(false);
  });

  it('returns false on unparsable input rather than nagging', () => {
    expect(isUpdateAvailable('dev', '1.2.3')).toBe(false);
    expect(isUpdateAvailable('1.2.3', 'not-a-version')).toBe(false);
    expect(isUpdateAvailable('', '1.2.3')).toBe(false);
    expect(isUpdateAvailable(null, '1.2.3')).toBe(false);
    expect(isUpdateAvailable('1.2.3', undefined)).toBe(false);
  });

  it('treats 1.2 as equal to 1.2.0', () => {
    expect(isUpdateAvailable('1.2', '1.2.0')).toBe(false);
    expect(isUpdateAvailable('1.2', '1.2.1')).toBe(true);
  });
});
