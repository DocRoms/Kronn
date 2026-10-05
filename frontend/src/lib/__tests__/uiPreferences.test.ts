import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../api', () => ({ config: {} }));

import { safeSetItem } from '../safeStorage';
import {
  createUiPreferencesSync,
  HYDRATED_MARKER,
  isSyncedKey,
  type UiPreferencesTransport,
} from '../uiPreferences';

function transport(remote: Record<string, string> = {}) {
  const t = {
    get: vi.fn<UiPreferencesTransport['get']>(async () => ({ ...remote })),
    put: vi.fn<UiPreferencesTransport['put']>(async () => undefined),
  };
  return t;
}

beforeEach(() => {
  localStorage.clear();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('isSyncedKey', () => {
  it('syncs visible preferences and never drafts, outboxes or secrets', () => {
    expect(isSyncedKey('kronn:theme', false)).toBe(true);
    expect(isSyncedKey('kronn:collection-favorites:projects', false)).toBe(true);
    for (const key of [
      'kronn:draft:d1',
      'kronn:reply-draft:d1',
      'kronn:message-outbox:d1',
      'kronn:setup-status',
      'kronn:secretInputRevealed',
      'kronn:ui-locale',
      HYDRATED_MARKER,
    ]) {
      expect(isSyncedKey(key, true), key).toBe(false);
    }
  });

  it('keeps the web session id per desktop install only', () => {
    expect(isSyncedKey('kronn:webSessionId', true)).toBe(true);
    expect(isSyncedKey('kronn:webSessionId', false)).toBe(false);
  });
});

describe('boot hydration', () => {
  it('fills a fresh origin from the server before the first render', async () => {
    const t = transport({ 'kronn:theme': 'light', 'kronn:tour-progress:v1': '{"hasStarted":true}', 'kronn:draft:x': 'never' });
    const sync = createUiPreferencesSync({ transport: t, isDesktop: () => false });

    await sync.boot();

    expect(localStorage.getItem('kronn:theme')).toBe('light');
    expect(localStorage.getItem('kronn:tour-progress:v1')).toBe('{"hasStarted":true}');
    expect(localStorage.getItem('kronn:draft:x')).toBeNull();
    expect(localStorage.getItem(HYDRATED_MARKER)).toBe('1');
  });

  it('lets a fresh origin take the server copy over defaults written early', async () => {
    localStorage.setItem('kronn:sidebarCollapsed', 'false');
    const sync = createUiPreferencesSync({ transport: transport({ 'kronn:sidebarCollapsed': 'true' }), isDesktop: () => false });
    await sync.boot();
    expect(localStorage.getItem('kronn:sidebarCollapsed')).toBe('true');
  });

  it('only fills missing keys on an origin with its own history', async () => {
    localStorage.setItem(HYDRATED_MARKER, '1');
    localStorage.setItem('kronn:theme', 'dark');
    const sync = createUiPreferencesSync({
      transport: transport({ 'kronn:theme': 'light', 'kronn:layoutDensity': 'compact' }),
      isDesktop: () => false,
    });

    await sync.boot();
    await vi.waitFor(() => expect(localStorage.getItem('kronn:layoutDensity')).toBe('compact'));
    expect(localStorage.getItem('kronn:theme')).toBe('dark');
  });

  it('keeps localStorage alone when the server is unavailable, and never overwrites it blindly', async () => {
    localStorage.setItem('kronn:theme', 'dark');
    const t = transport();
    t.get.mockRejectedValue(new Error('backend down'));
    const sync = createUiPreferencesSync({ transport: t, isDesktop: () => false });

    await expect(sync.boot()).resolves.toBeUndefined();
    await expect(sync.flush()).resolves.toBeUndefined();

    expect(localStorage.getItem('kronn:theme')).toBe('dark');
    expect(localStorage.getItem(HYDRATED_MARKER)).toBeNull();
    expect(t.put).not.toHaveBeenCalled();
  });

  it('does not hold the first render past its timeout when the server hangs', async () => {
    vi.useFakeTimers();
    const t = transport();
    t.get.mockReturnValue(new Promise(() => {}));
    const sync = createUiPreferencesSync({ transport: t, isDesktop: () => false });

    const booted = sync.boot(500);
    await vi.advanceTimersByTimeAsync(500);
    await expect(booted).resolves.toBeUndefined();
  });

  it('survives a storage that throws on every access', async () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('blocked'); });
    const t = transport({ 'kronn:theme': 'light' });
    const sync = createUiPreferencesSync({ transport: t, isDesktop: () => false });
    await expect(sync.boot()).resolves.toBeUndefined();
    await expect(sync.flush()).resolves.toBeUndefined();
    expect(t.put).not.toHaveBeenCalled();
  });
});

describe('write-through', () => {
  it('pushes a debounced snapshot of synced keys only', async () => {
    vi.useFakeTimers();
    const t = transport({ 'kronn:theme': 'light' });
    const sync = createUiPreferencesSync({ transport: t, isDesktop: () => false });
    await sync.boot();
    const stop = sync.start();
    try {
      safeSetItem('kronn:draft:d1', 'unsent text');
      safeSetItem('kronn:theme', 'dark');
      safeSetItem('kronn:layoutDensity', 'compact');
      await vi.advanceTimersByTimeAsync(450);

      expect(t.put).toHaveBeenCalledTimes(1);
      expect(t.put).toHaveBeenCalledWith({ 'kronn:theme': 'dark', 'kronn:layoutDensity': 'compact' });
    } finally {
      stop();
    }
  });

  it('catches direct localStorage writers at the periodic check and skips unchanged snapshots', async () => {
    vi.useFakeTimers();
    const t = transport({ 'kronn:theme': 'light' });
    const sync = createUiPreferencesSync({ transport: t, isDesktop: () => false });
    await sync.boot();
    const stop = sync.start();
    try {
      await vi.advanceTimersByTimeAsync(5_000);
      expect(t.put).not.toHaveBeenCalled();
      localStorage.setItem('kronn:planningCollapsedSections', '["done"]');
      await vi.advanceTimersByTimeAsync(5_000);
      expect(t.put).toHaveBeenCalledWith({ 'kronn:theme': 'light', 'kronn:planningCollapsedSections': '["done"]' });
    } finally {
      stop();
    }
  });

  it('retries after a failed write and never throws', async () => {
    const t = transport();
    t.put.mockRejectedValueOnce(new Error('503'));
    const sync = createUiPreferencesSync({ transport: t, isDesktop: () => false });
    await sync.boot();
    localStorage.setItem('kronn:theme', 'dark');

    await expect(sync.flush()).resolves.toBeUndefined();
    await sync.flush();
    expect(t.put).toHaveBeenCalledTimes(2);
    await sync.flush();
    expect(t.put).toHaveBeenCalledTimes(2);
  });

  it('carries over keys this client does not manage, so a browser never drops the desktop id', async () => {
    const t = transport({ 'kronn:webSessionId': 'desktop-id', 'kronn:future-key': 'x' });
    const sync = createUiPreferencesSync({ transport: t, isDesktop: () => false });
    localStorage.setItem('kronn:webSessionId', 'browser-id');
    localStorage.setItem(HYDRATED_MARKER, '1');
    await sync.boot();
    localStorage.setItem('kronn:theme', 'dark');

    await sync.flush();

    expect(localStorage.getItem('kronn:webSessionId')).toBe('browser-id');
    expect(t.put).toHaveBeenCalledWith({
      'kronn:webSessionId': 'desktop-id',
      'kronn:future-key': 'x',
      'kronn:theme': 'dark',
    });
  });

  it('restores the desktop participant id on a new desktop origin and records a first one', async () => {
    const restored = createUiPreferencesSync({ transport: transport({ 'kronn:webSessionId': 'desktop-id' }), isDesktop: () => true });
    await restored.boot();
    expect(localStorage.getItem('kronn:webSessionId')).toBe('desktop-id');

    localStorage.clear();
    const t = transport();
    const first = createUiPreferencesSync({ transport: t, isDesktop: () => true });
    await first.boot();
    localStorage.setItem('kronn:webSessionId', 'new-id');
    await first.flush();
    expect(t.put).toHaveBeenCalledWith({ 'kronn:webSessionId': 'new-id' });
  });
});
