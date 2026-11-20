import type { NotCarried } from '../types/generated';

/** What an exported file names and does not hold (KT-1843), read defensively. */
export function notCarriedIn(content: string): NotCarried[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(content);
  } catch {
    return [];
  }
  const list = (parsed as { not_carried?: unknown } | null)?.not_carried;
  if (!Array.isArray(list)) return [];
  return list.filter((item): item is NotCarried =>
    typeof item === 'object' && item !== null
    && typeof (item as NotCarried).kind === 'string'
    && typeof (item as NotCarried).id === 'string'
    && typeof (item as NotCarried).named_in === 'string');
}
