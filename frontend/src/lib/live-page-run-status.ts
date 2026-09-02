import type { RunStatus } from '../types/generated';

/**
 * The single canonical set of terminal `RunStatus` values, shared by the Live
 * Page pipeline fold and the binding mirror. A new terminal status is added
 * here and nowhere else — both `TERMINAL_FAILURE` (in `live-page-pipeline`) and
 * the mirror's non-terminal / active check derive from this one set.
 */
export const TERMINAL_RUN: ReadonlySet<RunStatus> = new Set<RunStatus>([
  'Success', 'Partial', 'Failed', 'Cancelled', 'StoppedByGuard', 'Interrupted',
]);
