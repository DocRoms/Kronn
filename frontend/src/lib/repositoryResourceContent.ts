import type { RepositoryResourceComparison, RepositoryResourceFileContent } from '../types/generated';
import type { SyncState } from './repositoryResourceRows';

/** What a resource's sheet can show: the file as the repository holds it, as
 *  Kronn holds it, or what differs between the two. */
export type ContentMode = 'repository' | 'kronn' | 'diff';

export const CONTENT_MODES: ContentMode[] = ['repository', 'kronn', 'diff'];

/** Why a mode has nothing to show, worded by the sheet in its own language. */
export type ModeReason = 'absent_repository' | 'absent_kronn' | 'identical';

export interface ContentModes {
  /** The mode the sheet opens on. */
  preselected: ContentMode;
  /** The modes that have nothing to show, with why. */
  disabled: Partial<Record<ContentMode, ModeReason>>;
  /** The side that alone moved since the two were aligned, when the two
   *  differ: the sheet says so beside the diff. Two sides that both moved are
   *  a conflict, and neither is called newer. */
  newer: 'repository' | 'kronn' | null;
}

/** Where a side's text is, when it has one. */
const hasSide = (file: RepositoryResourceFileContent, side: 'repository' | 'kronn'): boolean => (
  file[side] !== undefined
);

/** Which modes a comparison can show and which it opens on.
 *
 *  - only in the repository → Repository; only in Kronn → Kronn;
 *  - the same on both sides → Kronn, and Diff is off ("identical");
 *  - different on both sides → Diff, and when one side alone moved
 *    (`repository_newer` / `kronn_newer`, from the dates and baseline the
 *    listing already reports) the sheet says which.
 *
 *  Everything is read from what the backend sent, so a mode is never offered
 *  for content that is not there. */
export function contentModes(state: SyncState, comparison: RepositoryResourceComparison): ContentModes {
  const files = comparison.files ?? [];
  const inRepository = files.some(file => hasSide(file, 'repository'));
  const inKronn = files.some(file => hasSide(file, 'kronn'));
  const differs = (comparison.file_diffs ?? []).length > 0
    || files.some(file => hasSide(file, 'repository') && hasSide(file, 'kronn') && file.repository !== file.kronn);

  const disabled: ContentModes['disabled'] = {};
  if (!inRepository) disabled.repository = 'absent_repository';
  if (!inKronn) disabled.kronn = 'absent_kronn';
  if (!inRepository) disabled.diff = 'absent_repository';
  else if (!inKronn) disabled.diff = 'absent_kronn';
  else if (!differs) disabled.diff = 'identical';

  let preselected: ContentMode = 'kronn';
  if (inRepository && !inKronn) preselected = 'repository';
  else if (inRepository && inKronn && differs) preselected = 'diff';

  let newer: ContentModes['newer'] = null;
  if (differs) {
    if (state === 'repository_newer') newer = 'repository';
    else if (state === 'kronn_newer') newer = 'kronn';
  }
  return { preselected, disabled, newer };
}

/** The files that have a text on `side`, in the order the backend gave them
 *  (the main file first). */
export const filesOn = (
  comparison: RepositoryResourceComparison,
  side: 'repository' | 'kronn',
): RepositoryResourceFileContent[] => (comparison.files ?? []).filter(file => hasSide(file, side));

/** A skill's file split into its header and its body. `header` is the text
 *  between the two `---` lines (null when the file has none); `body` is the
 *  rest, which is what "Rendered" turns into Markdown. */
export function splitFrontMatter(text: string): { header: string | null; body: string } {
  const match = /^---\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/.exec(text);
  if (!match) return { header: null, body: text };
  return { header: match[1], body: text.slice(match[0].length).replace(/^\r?\n/, '') };
}

/** Whether a file is shown as Markdown: a skill's `SKILL.md`. Everything else
 *  (YAML or JSON definitions, prompts, an artifact's page) is shown as text. */
export const isMarkdownContent = (kind: string, path: string): boolean => (
  kind === 'skill' && /\.md$/i.test(path)
);
