// Heuristics for helping users configure local (Ollama) agent steps.
//
// A local Ollama model is a tool-less HTTP model: it has NO file/repo/worktree
// or MCP access — it only ever sees the prompt text. A workflow step whose
// prompt asks the agent to *read* files therefore silently works blind. We lint
// for that so the wizard can warn and tell the user to inject the content.

const FILE_ACCESS_RE =
  /(?:\b(?:lis|lit|lir|ouvr|consult|read|reading|open|inspect)\w*\b[^.]{0,40}\b(?:fichier|fichiers|file|files|worktree|repo|dépôt|code|diff)\b)|\.kronn\/|git\s+diff|\bworktree\b|<worktreePath>/i;

/** True when a prompt looks like it relies on the agent reading files / the
 *  worktree — impossible for a tool-less Ollama model. */
export function promptNeedsFileAccess(prompt: string | null | undefined): boolean {
  return FILE_ACCESS_RE.test(prompt ?? '');
}
