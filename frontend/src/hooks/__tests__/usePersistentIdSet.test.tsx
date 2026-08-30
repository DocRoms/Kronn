import { describe, expect, it } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { usePersistentIdSet } from '../usePersistentIdSet';

describe('usePersistentIdSet', () => {
  it('restores favorites through an empty loading state, then prunes only after readiness', () => {
    const key = 'kronn:test:persistent-id-set';
    localStorage.setItem(key, JSON.stringify(['kept', 'stale']));

    const { result, rerender } = renderHook(
      ({ availableIds, ready }) => usePersistentIdSet(key, availableIds, ready),
      { initialProps: { availableIds: [] as string[], ready: false } },
    );

    expect([...result.current.ids].sort()).toEqual(['kept', 'stale']);
    expect(JSON.parse(localStorage.getItem(key) ?? '[]')).toEqual(['kept', 'stale']);

    rerender({ availableIds: ['kept'], ready: true });

    expect([...result.current.ids]).toEqual(['kept']);
    expect(JSON.parse(localStorage.getItem(key) ?? '[]')).toEqual(['kept']);

    act(() => result.current.toggle('kept'));
    expect(JSON.parse(localStorage.getItem(key) ?? '[]')).toEqual([]);
  });
});
