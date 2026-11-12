// KT-1490 — the activation card a room message carries. Kept out of the
// component module so that module exports only components.

/** The marker of the System message that carries the room's activation card. */
export const ACTIVATION_CARD_PREFIX = '[kronn-activate:';

export interface ActivationCardPayload {
  quick_exec_id: string;
  name: string;
  program: string;
}

/** The card a room message holds, or `null` for anything else (never throws). */
export function parseActivationCard(content: string): ActivationCardPayload | null {
  const text = content.trim();
  if (!text.startsWith(ACTIVATION_CARD_PREFIX) || !text.endsWith(']')) return null;
  try {
    const value = JSON.parse(text.slice(ACTIVATION_CARD_PREFIX.length, -1)) as Partial<ActivationCardPayload>;
    if (typeof value.quick_exec_id !== 'string' || typeof value.name !== 'string' || typeof value.program !== 'string') {
      return null;
    }
    return { quick_exec_id: value.quick_exec_id, name: value.name, program: value.program };
  } catch {
    return null;
  }
}

/** KT-1695 — a room shows one card per Quick Exec: a line saved again adds a
 * card, and the card reads the Quick Exec live, so only the latest one stays.
 * Returns the ids of the earlier cards of the same Quick Exec. */
export function supersededActivationCards(
  messages: ReadonlyArray<{ id: string; role: string; content: string }>,
): Set<string> {
  const latest = new Map<string, string>();
  const cards: Array<{ id: string; quickExecId: string }> = [];
  for (const message of messages) {
    if (message.role !== 'System') continue;
    const card = parseActivationCard(message.content);
    if (!card) continue;
    latest.set(card.quick_exec_id, message.id);
    cards.push({ id: message.id, quickExecId: card.quick_exec_id });
  }
  return new Set(cards.filter(card => latest.get(card.quickExecId) !== card.id).map(card => card.id));
}
