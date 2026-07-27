/** One file's slice of a `git show` patch. */
export interface PatchFile {
  /** Path as the reader should see it: the post-image side, or `old → new` on a
   *  rename. Never empty — an unparseable header degrades to the raw line. */
  path: string;
  /** The file's own portion of the patch, header included. */
  body: string;
  added: number;
  removed: number;
  /** Git says "Binary files differ" instead of hunks. */
  binary: boolean;
}

/** `diff --git a/<old> b/<new>` — quoted paths appear when they contain spaces. */
const HEADER = /^diff --git (?:"?a\/(.*?)"?) (?:"?b\/(.*?)"?)$/;

/**
 * Split a multi-file patch on its `diff --git` boundaries.
 *
 * KT-87 — the commit tab used to render the whole patch at once: measured at
 * 196 590 px tall and ~11 000 DOM nodes on a root commit. Slicing per file lets
 * the viewer mount one file at a time, and lets it syntax-highlight with that
 * file's real language instead of picking one for the entire commit.
 *
 * Pure and total: anything before the first header (a merge's commit message, or
 * garbage) is kept as a leading pseudo-file rather than dropped, and a truncated
 * tail stays attached to the last file. Never throws.
 */
export function splitPatchByFile(patch: string): PatchFile[] {
  if (!patch.trim()) return [];
  const files: PatchFile[] = [];
  let current: PatchFile | null = null;

  const push = () => { if (current) files.push(current); };

  for (const line of patch.split('\n')) {
    const header = HEADER.exec(line);
    if (header) {
      push();
      const [, oldPath, newPath] = header;
      current = {
        path: oldPath === newPath ? newPath : `${oldPath} → ${newPath}`,
        body: line,
        added: 0,
        removed: 0,
        binary: false,
      };
      continue;
    }
    if (!current) {
      // Preamble (merge header, `--root` notice…). Keeping it visible beats
      // silently swallowing part of what git printed.
      current = { path: '…', body: line, added: 0, removed: 0, binary: false };
      continue;
    }
    current.body += `\n${line}`;
    if (line.startsWith('Binary files ') || line.startsWith('GIT binary patch')) {
      current.binary = true;
    } else if (line.startsWith('+') && !line.startsWith('+++')) {
      current.added += 1;
    } else if (line.startsWith('-') && !line.startsWith('---')) {
      current.removed += 1;
    }
  }
  push();
  return files;
}
