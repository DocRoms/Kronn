import { createContext } from 'react';

/** KT-1936 review round 2 — what a child of HeaderScrollLine reaches: the
 *  element that scrolls (to measure what is in view) and the pinned end slot
 *  (where the participants strip puts its « +N » counter, outside what
 *  scrolls). */
export interface HeaderScrollLineSlots {
  scroller: HTMLElement | null;
  end: HTMLElement | null;
}

export const HeaderScrollLineContext = createContext<HeaderScrollLineSlots | null>(null);
