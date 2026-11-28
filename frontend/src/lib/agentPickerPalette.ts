// KT-1885 — ids of the agent picker's palette (the composer's @ menu): the
// text field that drives it names the list (`aria-controls`) and the row it
// highlights (`aria-activedescendant`), so a screen reader announces it.

/**
 * The area the composer's @ menu must stay in: the discussion's message list,
 * which starts under the header and the banners above it; else the column
 * holding the composer (it clips what passes its top). `null` outside a
 * discussion: the window bounds the menu.
 */
export function composerMentionBoundary(field: Element | null | undefined): Element | null {
  const column = field?.closest('.disc-messages-col');
  return column?.querySelector('.disc-messages') ?? column ?? null;
}

/** The id of a palette row, from its list's id and its row key. */
export const agentPaletteOptionId = (listId: string, key: string) =>
  `${listId}-option-${key.replace(/[^A-Za-z0-9_-]/g, '_')}`;

/** What the text field driving an open palette carries, or nothing while it is closed. */
export function agentPaletteFieldAria(listId: string, open: boolean, highlightedKey?: string | null) {
  if (!open) return {};
  return {
    'aria-controls': listId,
    'aria-expanded': true,
    ...(highlightedKey ? { 'aria-activedescendant': agentPaletteOptionId(listId, highlightedKey) } : {}),
  } as const;
}
