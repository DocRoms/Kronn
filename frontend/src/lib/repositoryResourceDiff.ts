export type DiffLineKind = 'hunk' | 'context' | 'removed' | 'added';

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
}

export interface SideBySideRow {
  kind: 'hunk' | 'context' | 'changed';
  left: string | null;
  right: string | null;
}

/** Parses the backend's unified diff: `-` is the repository side, `+` Kronn's. */
export function parseUnifiedDiff(diff: string): DiffLine[] {
  const lines = diff.split('\n');
  if (lines[lines.length - 1] === '') lines.pop();
  const start = lines[0] === '--- repository' && lines[1] === '+++ Kronn' ? 2 : 0;
  return lines.slice(start).map((line): DiffLine => {
    if (line.startsWith('@@')) return { kind: 'hunk', text: line };
    if (line.startsWith('-')) return { kind: 'removed', text: line.slice(1) };
    if (line.startsWith('+')) return { kind: 'added', text: line.slice(1) };
    return { kind: 'context', text: line.startsWith(' ') ? line.slice(1) : line };
  });
}

/** Lines the repository dropped sit opposite the ones Kronn added. */
export function sideBySide(lines: DiffLine[]): SideBySideRow[] {
  const rows: SideBySideRow[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index];
    if (line.kind === 'hunk') {
      rows.push({ kind: 'hunk', left: line.text, right: line.text });
      index += 1;
    } else if (line.kind === 'context') {
      rows.push({ kind: 'context', left: line.text, right: line.text });
      index += 1;
    } else {
      const removed: string[] = [];
      const added: string[] = [];
      while (index < lines.length && (lines[index].kind === 'removed' || lines[index].kind === 'added')) {
        (lines[index].kind === 'removed' ? removed : added).push(lines[index].text);
        index += 1;
      }
      for (let row = 0; row < Math.max(removed.length, added.length); row += 1) {
        rows.push({ kind: 'changed', left: removed[row] ?? null, right: added[row] ?? null });
      }
    }
  }
  return rows;
}

export function formatFieldValue(value: unknown): string {
  if (value === undefined) return '—';
  return typeof value === 'string' ? value : JSON.stringify(value);
}
