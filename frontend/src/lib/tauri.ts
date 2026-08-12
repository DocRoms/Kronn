interface TauriCoreBridge {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
}

declare global {
  interface Window {
    __TAURI__?: { core?: TauriCoreBridge };
  }
}

export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && typeof window.__TAURI__?.core?.invoke === 'function';
}

export function isTauriAssetLocation(location: Pick<Location, 'protocol' | 'hostname'>): boolean {
  return location.protocol === 'tauri:' || location.hostname === 'tauri.localhost';
}

export function invokeTauri<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke;
  if (!invoke) return Promise.reject(new Error('Tauri runtime is unavailable'));
  return invoke<T>(command, args);
}

/** Resolve the embedded service before React starts rendering. */
export async function getDesktopBackendUrl(): Promise<string | null> {
  if (!isTauriRuntime()) return null;
  return invokeTauri<string>('wait_for_backend');
}

/** A page reload cannot restart the embedded Rust service. */
export async function retryDesktopStartup(): Promise<void> {
  if (isTauriRuntime()) {
    await invokeTauri('restart_app');
    return;
  }
  window.location.reload();
}
