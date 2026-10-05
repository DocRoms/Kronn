import { useEffect, useState } from 'react';
import { safeSetItem } from '../lib/safeStorage';

function readCollapsed(storageKey: string): boolean {
  try {
    return localStorage.getItem(storageKey) === 'true';
  } catch {
    return false;
  }
}

/**
 * Shared collection-sidebar collapse persistence (mirrors Discussions'
 * `kronn:sidebarCollapsed`). Only the desktop rail-collapsed state is
 * durable — a mobile session's transient drawer open/close never writes to
 * storage, so it can't leave a later desktop session starting collapsed.
 */
export function usePersistentSidebarOpen(storageKey: string, isMobile: boolean) {
  const [sidebarOpen, setSidebarOpen] = useState(() => !readCollapsed(storageKey));

  useEffect(() => {
    if (isMobile) return;
    safeSetItem(storageKey, String(!sidebarOpen));
  }, [isMobile, sidebarOpen, storageKey]);

  return [sidebarOpen, setSidebarOpen] as const;
}
