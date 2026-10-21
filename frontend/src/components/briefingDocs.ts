// KT-999 — pure helpers of the briefing's existing-documentation question.
import type { BriefingDetectedDoc, BriefingDocLocation, BriefingDocState } from '../types/generated';

export const BRIEFING_DOC_STATES: BriefingDocState[] = ['recent', 'old', 'obsolete', 'partial', 'reference'];
export const DOC_NOTE_MAX = 200;
export const DOC_PATH_MAX = 200;

export interface DocRow extends BriefingDetectedDoc {
  included: boolean;
  state: BriefingDocState;
  note: string;
  source_of_truth: boolean;
}

export const toRow = (doc: BriefingDetectedDoc): DocRow => ({
  ...doc, included: true, state: 'unknown', note: '', source_of_truth: false,
});

/** The kept rows as answers; one source of truth per kind, never an obsolete one. */
export function rowsToLocations(rows: DocRow[]): BriefingDocLocation[] {
  return rows
    .filter(r => r.included)
    .map(r => ({
      path: r.path,
      kind: r.kind,
      state: r.state,
      note: r.note.trim(),
      source_of_truth: r.source_of_truth && r.state !== 'obsolete',
      last_commit: null,
    }));
}

/** Marks `path` as the source of truth of its kind, clearing the others of that kind. */
export function setSourceOfTruth(rows: DocRow[], path: string, on: boolean): DocRow[] {
  const kind = rows.find(r => r.path === path)?.kind;
  return rows.map(r => {
    if (r.path === path) return { ...r, source_of_truth: on };
    if (on && r.kind === kind) return { ...r, source_of_truth: false };
    return r;
  });
}

/** The saved locations, `null` without a saved briefing, `'loading'` until it is known. */
export type SavedDocs = BriefingDocLocation[] | null | 'loading';

/**
 * KT-1196 — rows for a re-opened briefing: the saved locations first, in their
 * saved order and with their answers; a location detected since is offered
 * unchecked, so saving unchanged writes the same section.
 */
export function mergeSavedDocs(found: BriefingDetectedDoc[], saved: BriefingDocLocation[] | null): DocRow[] {
  if (saved === null) return found.map(toRow);
  const kept: DocRow[] = saved.map(l => {
    const detected = found.find(d => d.path === l.path);
    return {
      path: l.path,
      kind: l.kind,
      is_dir: detected?.is_dir ?? false,
      last_commit: detected?.last_commit ?? l.last_commit ?? null,
      included: true,
      state: l.state,
      note: l.note,
      source_of_truth: l.source_of_truth,
    };
  });
  const fresh = found
    .filter(d => !saved.some(l => l.path === d.path))
    .map(d => ({ ...toRow(d), included: false }));
  return [...kept, ...fresh];
}
