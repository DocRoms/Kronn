import { useSyncExternalStore } from 'react';
import { getBackendHealth, subscribeBackendHealth, type BackendHealth } from '../lib/backendReachability';

export function useBackendHealth(): BackendHealth {
  return useSyncExternalStore(subscribeBackendHealth, getBackendHealth, getBackendHealth);
}
