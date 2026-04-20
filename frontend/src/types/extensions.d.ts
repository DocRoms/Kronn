// Type extensions for fields added in newer backend versions.
// These augment auto-generated types until `make typegen` is re-run.

// Types not yet in generated.ts (added until next `make typegen`)
export interface DiscoveredKey {
  provider: string;
  source: string;
  suggested_name: string;
  already_exists: boolean;
}

export interface DiscoverKeysResponse {
  discovered: DiscoveredKey[];
  imported_count: number;
}

// ── Test mode (worktree swap-in-main-repo UX) ─────────────────────────────
//
// The enter endpoint returns a tagged-union envelope: either the happy path
// ("ok") with branch-swap details, or a structured preflight blocker
// ("blocked") the UI maps to the right modal via `kind`.

export interface TestModeEnterSuccess {
  status: 'ok';
  previous_branch: string;
  tested_branch: string;
  stashed: boolean;
  was_detached: boolean;
}

/**
 * Kinds the UI is expected to react to. Keep in sync with the Rust match
 * arms in `test_mode_enter`. Unknown kinds degrade to the generic error
 * toast + the server's `message`.
 */
export type TestModeBlockerKind =
  | 'WorktreeDirty'
  | 'MainDirty'
  | 'Detached'
  | 'AlreadyInTestMode'
  | 'NoBranch'
  | 'NoProject';

export interface TestModeBlocker {
  status: 'blocked';
  kind: TestModeBlockerKind | string;
  message: string;
  details?: {
    files?: Array<{ path: string; status: string }>;
    current_branch?: string;
    [k: string]: unknown;
  } | null;
}

export type TestModeEnterResult = TestModeEnterSuccess | TestModeBlocker;

export interface TestModeExitResponse {
  restored_branch: string;
  unstashed: boolean;
  worktree_restored: boolean;
}
