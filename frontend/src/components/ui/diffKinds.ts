// KT-2004 — the variants of DiffViewer, apart from the component so a file of
// components exports components only (fast refresh). The model it renders is
// in lib/diffModel.ts.

/** unified: one column, old and new lines one after the other; split: the
 *  old version on the left, the new one on the right. */
export type DiffLayout = 'unified' | 'split';
export const DIFF_LAYOUTS: readonly DiffLayout[] = ['unified', 'split'];

/** The bounded heights of the files' scroll area. */
export type DiffMaxHeight = 'sm' | 'md' | 'lg';
export const DIFF_MAX_HEIGHTS: readonly DiffMaxHeight[] = ['sm', 'md', 'lg'];

/** Under this width of its own container (in rem, --kr-bp-sm at the default
 *  text size), side by side shows as unified. */
export const DIFF_SPLIT_MIN_REM = 40;

/** A file with more lines than this is shown without syntax highlighting. */
export const DIFF_HIGHLIGHT_LIMIT = 2000;

/** The most lines a selection holds (a comment range). */
export const DIFF_MAX_SELECTION = 50;

/** The marker of a line, written as text: the colour never says it alone. */
export const DIFF_MARKERS = { added: '+', removed: '−', context: ' ' } as const;
