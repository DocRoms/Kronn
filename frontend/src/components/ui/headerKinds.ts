// KT-1884 — the six kinds of header and the one heading level each renders.
// Kept out of the component files so they export components only (fast
// refresh). Where each kind goes: docs/conventions/header-kinds.md.
import type { ReactNode } from 'react';

export type HeaderKind = 'list' | 'detail' | 'tab' | 'section' | 'drawer' | 'card';

/** The heading level of each kind, fixed: a caller cannot change it. */
export const HEADER_LEVELS: Readonly<Record<HeaderKind, 1 | 2 | 3>> = {
  // A level is set by where the kind sits, so a page read heading by heading
  // never skips one: list (1) > detail, drawer, card (2) > tab, section (3).
  /** The page header of a list page (and of Settings): the page's one h1. */
  list: 1,
  /** The selected object of a list, shown beside or instead of the list. */
  detail: 2,
  /** One tab of a detail, under its tab bar. */
  tab: 3,
  /** A group of content inside a detail, a tab or a drawer. */
  section: 3,
  /** A side panel or a drawer: it names the layer. */
  drawer: 2,
  /** One card of a grid or a list, most often right under the page's h1: it
   *  names the card, beside its status. An h3 there would skip a level. */
  card: 2,
};

/** Whether a header slot holds something to draw. `null`, `undefined`,
 *  `false`, `true` and `''` draw nothing in React, so they are no slot at
 *  all: the usual `status={shared && <StatusBadge … />}` leaves no empty
 *  wrapper and no gap. Every slot of the frame goes through it. */
export function hasSlot(node: ReactNode): boolean {
  return node != null && typeof node !== 'boolean' && node !== '';
}

/** The kinds in the order the gallery and the docs list them. */
export const HEADER_KINDS: readonly HeaderKind[] = ['list', 'detail', 'tab', 'section', 'drawer', 'card'];
