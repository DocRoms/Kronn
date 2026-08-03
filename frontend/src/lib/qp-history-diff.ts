export interface DiffRow {
  prev: string;
  next: string;
  kind: 'same' | 'changed' | 'added' | 'removed';
}

/** Side-by-side line diff used by the Quick Prompt history drawer. */
export function diffLines(prev: string, next: string): DiffRow[] {
  const previousLines = prev.split('\n');
  const nextLines = next.split('\n');
  const max = Math.max(previousLines.length, nextLines.length);
  const rows: DiffRow[] = [];

  for (let index = 0; index < max; index += 1) {
    const previous = previousLines[index] ?? '';
    const nextLine = nextLines[index] ?? '';
    let kind: DiffRow['kind'];
    if (previous === nextLine) kind = 'same';
    else if (previous === '') kind = 'added';
    else if (nextLine === '') kind = 'removed';
    else kind = 'changed';
    rows.push({ prev: previous, next: nextLine, kind });
  }

  return rows;
}
