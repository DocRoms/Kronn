// KT-1883 — the variants of the feedback components, apart from the
// components so a file of components exports components only (fast refresh).

/** info: something to know; warning: something to look at before going on;
 *  danger: something failed or cannot be done; success: something done that
 *  lasts on the page. Each takes its --kr-status-* colour. */
export type NoticeTone = 'info' | 'warning' | 'danger' | 'success';
export const NOTICE_TONES: readonly NoticeTone[] = ['info', 'warning', 'danger', 'success'];

/** firstUse: nothing exists yet, the action creates the first one;
 *  noResult: things exist but the search or the filters hide them all, the
 *  action clears them; nothingSelected: a detail pane waits for a choice in
 *  the list next to it, no action. */
export type EmptyStateKind = 'firstUse' | 'noResult' | 'nothingSelected';
export const EMPTY_STATE_KINDS: readonly EmptyStateKind[] = ['firstUse', 'noResult', 'nothingSelected'];

/** sm: inline, a ring beside a short text (a row, a cell, beside a button);
 *  md: a panel or a page area, the mark over its sentence (the default);
 *  lg: a whole page, the large mark. */
export type LoadingSize = 'sm' | 'md' | 'lg';
export const LOADING_SIZES: readonly LoadingSize[] = ['sm', 'md', 'lg'];

/** list: rows of a list or a table; detail: the header and the body of a
 *  detail pane. */
export type SkeletonKind = 'list' | 'detail';
export const SKELETON_KINDS: readonly SkeletonKind[] = ['list', 'detail'];
