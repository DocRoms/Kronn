// KT-1886 — the variants of the dense-data components (CopyButton, Table,
// KeyValueList, StatTile, ProgressBar), apart from the components so a file
// of components exports components only (fast refresh).

/** How long a CopyButton shows its check (or its failure) after a click. */
export const COPY_FEEDBACK_MS = 1500;

/** icon: an IconButton (a dense row, beside a value); text: a Button with
 *  its label written (a toolbar, a dialog footer). */
export type CopyButtonForm = 'icon' | 'text';

/** compact: --kr-type-sm (12 px) and tight padding; normal: --kr-type-md. */
export type TableDensity = 'compact' | 'normal';
export const TABLE_DENSITIES: readonly TableDensity[] = ['compact', 'normal'];

/** The bounded heights of a table's scroll area. */
export type TableMaxHeight = 'sm' | 'md' | 'lg';
export const TABLE_MAX_HEIGHTS: readonly TableMaxHeight[] = ['sm', 'md', 'lg'];

/** A narrow container: scroll keeps the columns and scrolls sideways; cards
 *  turns each row into a card, every value preceded by its column header. */
export type TableNarrow = 'scroll' | 'cards';
export const TABLE_NARROW_FORMS: readonly TableNarrow[] = ['scroll', 'cards'];

/** The tint of a row: danger (failed), warning (to look at), muted (done,
 *  archived, out of scope). The row still says it in words. */
export type TableRowTone = 'danger' | 'warning' | 'muted';
export const TABLE_ROW_TONES: readonly TableRowTone[] = ['danger', 'warning', 'muted'];

export type TableSortDirection = 'ascending' | 'descending';
export interface TableSort {
  column: string;
  direction: TableSortDirection;
}

/** The sort a click on the header of `column` asks for: ascending first, then
 *  the other way on the same column. */
export function nextSort(current: TableSort | null | undefined, column: string): TableSort {
  if (current?.column === column && current.direction === 'ascending') return { column, direction: 'descending' };
  return { column, direction: 'ascending' };
}

/** grid: labels in a max-content column, values beside them; inline: one row
 *  per item, the label on the left and the value on the right (tabular). */
export type KeyValueLayout = 'grid' | 'inline';
export const KEY_VALUE_LAYOUTS: readonly KeyValueLayout[] = ['grid', 'inline'];

/** md: value at --kr-type-xl; lg: value at --kr-type-2xl. */
export type StatTileSize = 'md' | 'lg';
export const STAT_TILE_SIZES: readonly StatTileSize[] = ['md', 'lg'];
export type StatDeltaDirection = 'up' | 'down' | 'flat';
/** Good, bad or neither: the colour of the arrow. Never the only cue: the
 *  direction is written in the tile's accessible text. */
export type StatDeltaTone = 'success' | 'danger' | 'warning' | 'neutral';

export type ProgressTone = 'accent' | 'success' | 'warning' | 'danger' | 'info';
export const PROGRESS_TONES: readonly ProgressTone[] = ['accent', 'success', 'warning', 'danger', 'info'];
/** sm: a thin line under a label; md: the default. */
export type ProgressSize = 'sm' | 'md';
export const PROGRESS_SIZES: readonly ProgressSize[] = ['sm', 'md'];
