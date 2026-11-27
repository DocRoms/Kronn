// KT-2004 — the diff model that components/ui/DiffViewer renders: files, their
// hunks and their lines with old and new line numbers. Built from a git patch
// by parseUnifiedDiff, or from the positional pairs of qp-history-diff by
// diffFileFromRows. DiffViewer never parses text itself.
import type { DiffRow } from './qp-history-diff';

export type DiffFileStatus = 'added' | 'deleted' | 'modified' | 'renamed' | 'copied';
export const DIFF_FILE_STATUSES: readonly DiffFileStatus[] = ['added', 'deleted', 'modified', 'renamed', 'copied'];

/** context: in both versions; added / removed: in one; changed: the same
 *  position holds another text (`text` the new one, `otherText` the old one);
 *  meta: a line about the diff ("\ No newline at end of file", a mode). */
export type DiffLineKind = 'context' | 'added' | 'removed' | 'changed' | 'meta';

export interface DiffLineModel {
  kind: DiffLineKind;
  oldNumber?: number;
  newNumber?: number;
  /** The new text (the old one for a removed line). */
  text: string;
  /** A changed line's old text. */
  otherText?: string;
}

export interface DiffHunkModel {
  /** The `@@ -a,b +c,d @@ …` line; none for the file's leading meta lines. */
  header?: string;
  lines: DiffLineModel[];
}

export interface DiffFileModel {
  path: string;
  /** The path before a rename or a copy. */
  previousPath?: string;
  status: DiffFileStatus;
  added?: number;
  removed?: number;
  /** A binary file: no line is shown, the viewer says "binary". */
  binary?: boolean;
  /** The lines were cut by the producer: the viewer says "truncated". */
  truncated?: boolean;
  hunks: DiffHunkModel[];
}

/** A range of lines of one file, by their index across its hunks (every
 *  line counts, meta ones too), `start` ≤ `end`. */
export interface DiffSelection {
  path: string;
  start: number;
  end: number;
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;
/** Preamble lines of a file kept as meta lines: they change the file, not its text. */
const MODE_LINE = /^(?:old mode|new mode|new file mode|deleted file mode) /;

/** `a/x` or `"b/x y"` → `x` / `x y`; `/dev/null` → null. */
function pathOf(raw: string): string | null {
  let p = raw.replace(/\t.*$/, '').trim();
  if (p.startsWith('"') && p.endsWith('"')) p = p.slice(1, -1);
  if (p === '/dev/null') return null;
  return p.replace(/^[ab]\//, '');
}

/** Counts each file's added and removed lines (a changed line is one of each). */
function withCounts(file: DiffFileModel): DiffFileModel {
  let added = 0;
  let removed = 0;
  for (const hunk of file.hunks) {
    for (const line of hunk.lines) {
      if (line.kind === 'added' || line.kind === 'changed') added++;
      if (line.kind === 'removed' || line.kind === 'changed') removed++;
    }
  }
  return { ...file, added: file.added ?? added, removed: file.removed ?? removed };
}

/** Builds the diff model of a git patch (`git diff`, `git show`, a plain
 *  `--- / +++` patch): one file per `diff --git` (or `---` header), the old
 *  and new number of every line counted from its hunk header, renames,
 *  copies, added and deleted files, binary files. */
export function parseUnifiedDiff(text: string): DiffFileModel[] {
  const files: DiffFileModel[] = [];
  let file: DiffFileModel | null = null;
  let hunk: DiffHunkModel | null = null;
  let oldNo = 0;
  let newNo = 0;
  let oldLeft = 0;
  let newLeft = 0;

  const startFile = (path: string, previousPath?: string): DiffFileModel => {
    const next: DiffFileModel = { path, status: 'modified', hunks: [] };
    if (previousPath && previousPath !== path) next.previousPath = previousPath;
    files.push(next);
    hunk = null;
    oldLeft = 0;
    newLeft = 0;
    return next;
  };
  const metaHunk = (f: DiffFileModel): DiffHunkModel => {
    const last = f.hunks[f.hunks.length - 1];
    if (last && last.header === undefined) return last;
    const h: DiffHunkModel = { lines: [] };
    f.hunks.push(h);
    return h;
  };

  for (const line of text.split(/\r?\n/)) {
    const inHunk = hunk !== null && (oldLeft > 0 || newLeft > 0);
    if (inHunk && hunk) {
      const h: DiffHunkModel = hunk;
      const sign = line[0];
      if (sign === '+') { h.lines.push({ kind: 'added', newNumber: newNo++, text: line.slice(1) }); newLeft--; continue; }
      if (sign === '-') { h.lines.push({ kind: 'removed', oldNumber: oldNo++, text: line.slice(1) }); oldLeft--; continue; }
      if (sign === ' ' || line === '') {
        h.lines.push({ kind: 'context', oldNumber: oldNo++, newNumber: newNo++, text: line.slice(1) });
        oldLeft--;
        newLeft--;
        continue;
      }
      if (sign === '\\') { h.lines.push({ kind: 'meta', text: line }); continue; }
      // Anything else ends the hunk early (a short count): read it below.
      oldLeft = 0;
      newLeft = 0;
    }
    if (line.startsWith('\\') && hunk) { (hunk as DiffHunkModel).lines.push({ kind: 'meta', text: line }); continue; }

    const git = line.match(/^diff --git (\S+|"[^"]+") (\S+|"[^"]+")$/) ?? line.match(/^diff --git a\/(.*) b\/(.*)$/);
    if (git) {
      const prev = pathOf(git[1]) ?? '';
      const next = pathOf(git[2]) ?? prev;
      file = startFile(next, prev);
      continue;
    }
    if (line.startsWith('--- ')) {
      // A plain patch has no diff --git line: `---` opens the file.
      if (!file || file.hunks.some(h => h.header !== undefined)) file = startFile(pathOf(line.slice(4)) ?? '');
      const prev = pathOf(line.slice(4));
      if (prev === null) file.status = 'added';
      else if (file.status !== 'renamed' && file.status !== 'copied') file.path = prev;
      continue;
    }
    if (line.startsWith('+++ ') && file) {
      const next = pathOf(line.slice(4));
      if (next === null) {
        file.status = 'deleted';
        if (file.previousPath) file.path = file.previousPath;
        delete file.previousPath;
      } else {
        if (file.status === 'modified' && file.path !== next) file.previousPath = file.path;
        file.path = next;
      }
      continue;
    }
    const header = line.match(HUNK);
    if (header && file) {
      hunk = { header: line, lines: [] };
      file.hunks.push(hunk);
      oldNo = Number(header[1]);
      newNo = Number(header[3]);
      oldLeft = header[2] === undefined ? 1 : Number(header[2]);
      newLeft = header[4] === undefined ? 1 : Number(header[4]);
      continue;
    }
    if (!file) continue;
    const rename = line.match(/^(rename|copy) (from|to) (.+)$/);
    if (rename) {
      file.status = rename[1] === 'rename' ? 'renamed' : 'copied';
      if (rename[2] === 'from') file.previousPath = pathOf(rename[3]) ?? undefined;
      else file.path = pathOf(rename[3]) ?? file.path;
      continue;
    }
    if (line.startsWith('Binary files ') || line === 'GIT binary patch') {
      file.binary = true;
      continue;
    }
    if (line.startsWith('new file mode ')) file.status = 'added';
    if (line.startsWith('deleted file mode ')) file.status = 'deleted';
    if (MODE_LINE.test(line)) metaHunk(file).lines.push({ kind: 'meta', text: line });
    // index, similarity and the rest describe the file the header already shows.
  }

  return files.map(f => {
    const counted = withCounts(f);
    if (counted.status === 'added' || counted.status === 'deleted') delete counted.previousPath;
    return counted;
  });
}

/** The diff model of the positional pairs of qp-history-diff: line N of the
 *  old text against line N of the new one, in one hunk. A changed row
 *  carries both texts. */
export function diffFileFromRows(
  rows: readonly DiffRow[],
  file: { path: string; previousPath?: string; status?: DiffFileStatus },
): DiffFileModel {
  const lines: DiffLineModel[] = rows.map((row, i) => {
    const n = i + 1;
    switch (row.kind) {
      case 'same': return { kind: 'context', oldNumber: n, newNumber: n, text: row.next };
      case 'changed': return { kind: 'changed', oldNumber: n, newNumber: n, text: row.next, otherText: row.prev };
      case 'added': return { kind: 'added', newNumber: n, text: row.next };
      case 'removed': return { kind: 'removed', oldNumber: n, text: row.prev };
    }
  });
  const model: DiffFileModel = { path: file.path, status: file.status ?? 'modified', hunks: [{ lines }] };
  if (file.previousPath) model.previousPath = file.previousPath;
  return withCounts(model);
}

/** Every line of a file, in order across its hunks: the index a selection uses. */
export function fileLines(file: DiffFileModel): DiffLineModel[] {
  return file.hunks.flatMap(h => h.lines);
}

/** The new text of a file's shown lines: context, added and the new side of
 *  a changed line, one per line (what its CopyButton copies). */
export function newText(file: DiffFileModel): string {
  return fileLines(file)
    .filter(l => l.kind === 'context' || l.kind === 'added' || l.kind === 'changed')
    .map(l => l.text)
    .join('\n');
}

/** The lines a selection covers in `file` (none when it is another file's). */
export function selectedLines(file: DiffFileModel, selection: DiffSelection | null | undefined): DiffLineModel[] {
  if (!selection || selection.path !== file.path) return [];
  return fileLines(file).slice(selection.start, selection.end + 1);
}
