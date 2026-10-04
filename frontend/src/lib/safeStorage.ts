/** localStorage access that never throws (disabled, full or blocked storage). */
type WriteListener = (key: string) => void;

let writeListener: WriteListener | null = null;

/** One listener (the server-side preference mirror) hears successful writes. */
export function setStorageWriteListener(listener: WriteListener | null): void {
  writeListener = listener;
}

function notify(key: string): void {
  try {
    writeListener?.(key);
  } catch {
    // A failing mirror never breaks the write that triggered it.
  }
}

export function safeGetItem(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function safeSetItem(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Preferences stay usable in memory for the session.
    return;
  }
  notify(key);
}

export function safeRemoveItem(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    return;
  }
  notify(key);
}
