import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  getDesktopBackendUrl,
  invokeTauri,
  isTauriAssetLocation,
  isTauriRuntime,
  retryDesktopStartup,
} from '../tauri';

describe('Tauri runtime bridge', () => {
  afterEach(() => {
    delete window.__TAURI__;
  });

  it('stays disabled in the ordinary web application', async () => {
    expect(isTauriRuntime()).toBe(false);
    await expect(getDesktopBackendUrl()).resolves.toBeNull();
    await expect(invokeTauri('restart_app')).rejects.toThrow('unavailable');
  });

  it('invokes only through the injected desktop bridge', async () => {
    const invoke = vi.fn().mockResolvedValue('http://127.0.0.1:43140');
    window.__TAURI__ = { core: { invoke } };

    expect(isTauriRuntime()).toBe(true);
    await expect(invokeTauri('wait_for_backend')).resolves.toBe('http://127.0.0.1:43140');
    expect(invoke).toHaveBeenCalledWith('wait_for_backend', undefined);
  });

  it('recognizes both Tauri asset origins without matching the backend origin', () => {
    expect(isTauriAssetLocation({ protocol: 'tauri:', hostname: 'localhost' } as Location)).toBe(true);
    expect(isTauriAssetLocation({ protocol: 'http:', hostname: 'tauri.localhost' } as Location)).toBe(true);
    expect(isTauriAssetLocation({ protocol: 'http:', hostname: '127.0.0.1' } as Location)).toBe(false);
  });

  it('surfaces a native startup failure before exposing a dead backend URL', async () => {
    const calls = vi.fn();
    window.__TAURI__ = { core: {
      invoke: async <T,>(command: string, args?: Record<string, unknown>) => {
        calls(command, args);
        if (command === 'wait_for_backend') throw new Error('another Kronn instance owns the data');
        return 'http://127.0.0.1:43140' as T;
      },
    } };

    await expect(getDesktopBackendUrl()).rejects.toThrow('another Kronn instance owns the data');
    expect(calls).toHaveBeenCalledWith('wait_for_backend', undefined);
  });

  it('returns the native backend URL and restarts the process on retry', async () => {
    const calls = vi.fn();
    window.__TAURI__ = { core: {
      invoke: async <T,>(command: string, args?: Record<string, unknown>) => {
        calls(command, args);
        const result = command === 'wait_for_backend'
          ? 'http://127.0.0.1:43140'
          : undefined;
        return result as T;
      },
    } };

    await expect(getDesktopBackendUrl()).resolves.toBe('http://127.0.0.1:43140');
    await retryDesktopStartup();
    expect(calls).toHaveBeenCalledWith('restart_app', undefined);
  });
});
