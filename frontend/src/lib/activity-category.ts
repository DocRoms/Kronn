/**
 * What an agent's tool call is, as the server reports it: a fixed category,
 * never the tool's name or its target.
 */
import { BookOpen, Search, Pencil, Terminal, Globe, Plug, Sparkles, Brain, Wrench, type LucideIcon } from 'lucide-react';
import type { ActivityCategory } from '../types/generated';

export const ACTIVITY_CATEGORIES: readonly ActivityCategory[] = [
  'Read', 'Search', 'Edit', 'Execute', 'Web', 'Mcp', 'Kronn', 'Think', 'Other',
];

export const ACTIVITY_CATEGORY_ICONS: Record<ActivityCategory, LucideIcon> = {
  Read: BookOpen, Search, Edit: Pencil, Execute: Terminal, Web: Globe, Mcp: Plug,
  Kronn: Sparkles, Think: Brain, Other: Wrench,
};

/** A category from the server; anything else (an older server's raw tool name) reads as Other. */
export function asActivityCategory(value: string | null | undefined): ActivityCategory {
  return ACTIVITY_CATEGORIES.includes(value as ActivityCategory) ? (value as ActivityCategory) : 'Other';
}

export function activityCategoryLabel(
  t: (key: string, ...args: (string | number)[]) => string,
  value: string | null | undefined,
): string {
  return t(`auditTimeline.activity.category.${asActivityCategory(value)}`);
}
