/** Compact token counts for tight surfaces — KT-254.
 *
 *  Its own module rather than living beside the badge, so a header, a tooltip and
 *  a panel all shorten the same way; two formatters would drift and make the same
 *  figure look like two.
 */

/** 1 234 567 → "1.2M". Never rounds a real number down to "0": a zero in a cost
 *  slot reads as free, which is the exact misreading this release is about. */
export function compactTokens(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}k`;
  if (value > 0) return '<1k';
  return '0';
}
