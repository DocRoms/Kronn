import type { WorkflowRun } from '../types/generated';

/** Run state keys the document audit writes, one per step (backend
 *  `workflows/runner.rs`, `DOCS_POSSIBLE_SECRETS_STATE_PREFIX`, KT-1796). */
export const DOCS_POSSIBLE_SECRETS_STATE_PREFIX = '__kronn.docs_possible_secrets.';
/** The records a run adopted from its child runs, kept apart from its own
 *  (`DOCS_CHILD_POSSIBLE_SECRETS_STATE_PREFIX`). */
export const DOCS_CHILD_POSSIBLE_SECRETS_STATE_PREFIX = '__kronn.docs_child_possible_secrets.';
const PREFIXES = [DOCS_POSSIBLE_SECRETS_STATE_PREFIX, DOCS_CHILD_POSSIBLE_SECRETS_STATE_PREFIX];

export interface PossibleSecretDocument {
  path: string;
  lines: number[];
  moreLines: number;
}

export interface StepPossibleSecrets {
  stepName: string;
  documents: PossibleSecretDocument[];
  moreDocuments: number;
  /** The record is there but cannot be read: still said, never dropped. */
  unreadable: boolean;
}

function parseReport(stepName: string, raw: string): StepPossibleSecrets {
  const unreadable: StepPossibleSecrets = { stepName, documents: [], moreDocuments: 0, unreadable: true };
  try {
    const parsed = JSON.parse(raw) as { documents?: unknown; more_documents?: unknown };
    if (!Array.isArray(parsed.documents)) return unreadable;
    const documents = parsed.documents.flatMap((entry): PossibleSecretDocument[] => {
      const doc = entry as { path?: unknown; lines?: unknown; more_lines?: unknown };
      if (typeof doc.path !== 'string' || !Array.isArray(doc.lines)) return [];
      return [{
        path: doc.path,
        lines: doc.lines.filter((line): line is number => typeof line === 'number'),
        moreLines: typeof doc.more_lines === 'number' ? doc.more_lines : 0,
      }];
    });
    const moreDocuments = typeof parsed.more_documents === 'number' ? parsed.more_documents : 0;
    // Only a count left (the listed documents were cleaned) is still a record.
    if (documents.length === 0 && moreDocuments === 0) return unreadable;
    return { stepName, documents, moreDocuments, unreadable: false };
  } catch {
    return unreadable;
  }
}

/** The possible secrets the document audit reported, per step, in key order.
 *  They failed nothing: the reader decides. */
export function runPossibleSecrets(run: WorkflowRun): StepPossibleSecrets[] {
  return Object.entries(run.state ?? {})
    .flatMap(([key, raw]): [string, string][] => {
      const prefix = PREFIXES.find(known => key.startsWith(known));
      return prefix ? [[key.slice(prefix.length), raw]] : [];
    })
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([stepName, raw]) => parseReport(stepName, raw));
}
