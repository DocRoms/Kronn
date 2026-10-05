import type { ExecScriptFile } from '../../types/generated';

/** One path per line; a path kept from the previous list keeps its approved hash. */
export function parseScriptPaths(text: string, previous: ExecScriptFile[]): ExecScriptFile[] {
  const known = new Map(previous.map(file => [file.path, file.sha256]));
  const seen = new Set<string>();
  const next: ExecScriptFile[] = [];
  for (const line of text.split('\n')) {
    const path = line.trim();
    if (!path || seen.has(path)) continue;
    seen.add(path);
    next.push({ path, sha256: known.get(path) ?? '' });
  }
  return next;
}
