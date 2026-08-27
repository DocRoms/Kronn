// Heuristics for helping users configure local (Ollama) agent steps.
//
// An HTTP model has no host shell or arbitrary MCP bridge. A project-bound
// workflow does receive Kronn's bounded workspace tools; an unbound workflow
// does not. This heuristic lets the wizard warn only in that latter case.

const FILE_ACCESS_RE =
  /(?:\b(?:lis|lit|lir|ouvr|consult|read|reading|open|inspect)\w*\b[^.]{0,40}\b(?:fichier|fichiers|file|files|worktree|repo|dépôt|code|diff)\b)|\.kronn\/|git\s+diff|\bworktree\b|<worktreePath>/i;

/** True when a prompt looks like it relies on reading files / the worktree. */
export function promptNeedsFileAccess(prompt: string | null | undefined): boolean {
  return FILE_ACCESS_RE.test(prompt ?? '');
}

/** Warn only when the prompt needs a workspace and the workflow cannot bind
 * one. Project-bound HTTP agents receive Kronn's bounded workspace tools. */
export function promptNeedsUnboundWorkspace(
  prompt: string | null | undefined,
  projectId: string | null | undefined,
): boolean {
  return !projectId && promptNeedsFileAccess(prompt);
}
