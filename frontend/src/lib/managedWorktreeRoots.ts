// KT-1748 — Settings > Workers: folders whose worktrees Kronn manages. One path per
// line in the form; the server re-validates (`validate_managed_worktree_roots`), this
// only spares a round trip for the obvious mistakes.

/** Most paths one list may hold (the server's `MAX_MANAGED_WORKTREE_ROOTS`). */
export const MAX_MANAGED_WORKTREE_ROOTS = 32;

/** The paths typed in the field: trimmed, blank lines dropped. */
export function parseManagedRoots(text: string): string[] {
  return text
    .split('\n')
    .map(line => line.trim())
    .filter(line => line.length > 0);
}

function isAbsolutePath(path: string): boolean {
  return path.startsWith('/') || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith('\\\\');
}

function isFilesystemRoot(path: string): boolean {
  return /^\/+$/.test(path) || /^[A-Za-z]:[\\/]?$/.test(path);
}

/** Whether the list is one the server will accept. */
export function validManagedRoots(roots: string[]): boolean {
  return roots.length <= MAX_MANAGED_WORKTREE_ROOTS
    && roots.every(root => isAbsolutePath(root)
      && !isFilesystemRoot(root)
      && !root.split(/[\\/]/).includes('..'));
}
