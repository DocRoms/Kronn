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
