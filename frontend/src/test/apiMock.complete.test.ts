// Completeness test for the shared API mock.
//
// Imports the REAL `lib/api.ts` and asserts that every top-level export
// (namespaces like `discussions`, `workflows`, ...) is also present in
// the default mock built by `buildApiMock()`.
//
// When you add a new namespace to `lib/api.ts`, this test fails — the
// error tells you which one is missing from `buildApiMock` so the shared
// mock stays in sync with reality.

import { describe, it, expect } from 'vitest';
import * as realApi from '../lib/api';
import { API_NAMESPACES, API_TOP_LEVEL_FNS, buildApiMock } from './apiMock';

describe('apiMock completeness', () => {
  it('buildApiMock covers every namespace exported by lib/api.ts', () => {
    const mock = buildApiMock();
    const realNamespaces = Object.keys(realApi).filter((key) => {
      const value = (realApi as Record<string, unknown>)[key];
      return typeof value === 'object' && value !== null;
    });

    for (const ns of realNamespaces) {
      expect(mock, `Missing namespace in buildApiMock: ${ns}`).toHaveProperty(ns);
    }

    // Also assert the declared list matches reality — catches cases where
    // a namespace was removed from api.ts but the mock still advertises it.
    const declared = new Set<string>(API_NAMESPACES);
    for (const ns of realNamespaces) {
      expect(declared, `API_NAMESPACES is stale — does not list "${ns}"`).toContain(ns);
    }
  });

  it('buildApiMock covers every flat top-level function exported by lib/api.ts', () => {
    const mock = buildApiMock();
    const flatFns = Object.keys(realApi).filter((key) => {
      return typeof (realApi as Record<string, unknown>)[key] === 'function';
    });

    for (const fn of flatFns) {
      expect(mock, `Missing flat export in buildApiMock: ${fn}`).toHaveProperty(fn);
      expect(API_TOP_LEVEL_FNS, `API_TOP_LEVEL_FNS missing "${fn}"`).toContain(fn as never);
    }
  });

  it('overrides deep-merge per namespace without erasing sibling methods', () => {
    // Regression: shallow overrides on `discussions.list` used to wipe out
    // `discussions.dismissPartial`, making tests that indirectly call it
    // break silently.
    const mock = buildApiMock({
      discussions: { list: (async () => [{ id: 'custom' }]) as never },
    });
    expect(mock.discussions.list).toBeDefined();
    expect(mock.discussions.dismissPartial).toBeDefined();
    expect(mock.discussions.stop).toBeDefined();
  });
});
