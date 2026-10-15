// KT-1199 — « 1-9, 12 » from a list of step numbers, for the repository's
// audit run: « étapes 1-9 faites, 10-16 à reprendre ».
export function stepRanges(indexes: readonly number[]): string {
  const sorted = [...new Set(indexes)].sort((a, b) => a - b);
  const parts: string[] = [];
  for (let i = 0; i < sorted.length; i++) {
    let end = i;
    while (end + 1 < sorted.length && sorted[end + 1] === sorted[end] + 1) end++;
    parts.push(end > i ? `${sorted[i]}-${sorted[end]}` : `${sorted[i]}`);
    i = end;
  }
  return parts.join(', ');
}
