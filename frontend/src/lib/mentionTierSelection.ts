import type { ResolvedCatalogTier } from './modelCatalogSelection';
import type { ModelTier } from '../types/generated';

export const MENTION_TIER_CHOICES: ModelTier[] = ['economy', 'default', 'reasoning'];

/** A key gesture may skip unavailable tiers, never select one or wrap. */
export function nextMentionTierIndex(
  current: number | null, effectiveTier: ModelTier, step: 1 | -1,
  resolve: (tier: ModelTier) => ResolvedCatalogTier,
): number | null {
  const from = current ?? Math.max(0, MENTION_TIER_CHOICES.indexOf(effectiveTier));
  for (let index = from + step; index >= 0 && index < MENTION_TIER_CHOICES.length; index += step) {
    if (!resolve(MENTION_TIER_CHOICES[index]).unavailable) return index;
  }
  return resolve(MENTION_TIER_CHOICES[from]).unavailable ? null : current ?? from;
}
