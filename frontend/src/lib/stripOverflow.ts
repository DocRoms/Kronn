// KT-1936 — the participants strip's overflow, measured on its list.

/** How many children of `list` end past its visible right edge, and whether
 *  it scrolls further right (the fade of ParticipantsStrip). */
export function measureStripOverflow(list: HTMLElement): { hidden: number; fadeEnd: boolean } {
  const edge = list.scrollLeft + list.clientWidth;
  const chips = Array.from(list.children) as HTMLElement[];
  const hidden = chips.filter(chip => chip.offsetLeft + chip.offsetWidth > edge + 1).length;
  return { hidden, fadeEnd: list.scrollWidth - edge > 1 };
}

/** KT-1936 review round 2 — in the room header the whole agent line scrolls
 *  (HeaderScrollLine), not the strip: how many chips of `list` are not fully
 *  inside the visible box of `scroller`, on either side. */
export function measureChipsOutOfView(scroller: HTMLElement, list: HTMLElement): number {
  const view = scroller.getBoundingClientRect();
  const chips = Array.from(list.children) as HTMLElement[];
  return chips.filter(chip => {
    const box = chip.getBoundingClientRect();
    return box.left < view.left - 1 || box.right > view.right + 1;
  }).length;
}

/** Whether the line scrolls further right: its edge fades while it does. */
export function lineFadesEnd(scroller: HTMLElement): boolean {
  return scroller.scrollWidth - scroller.scrollLeft - scroller.clientWidth > 1;
}
