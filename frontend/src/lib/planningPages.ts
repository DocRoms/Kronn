import { planning, type PlanningTaskListFilters } from './api';
import type { PlanningTaskListResponse, PlanningTaskSummary } from '../types/generated';

const PAGE_SIZE = 100;
/** A ceiling on the paging loop: 5,000 tasks, then the list says it stopped. */
export const MAX_PLANNING_PAGES = 50;

export interface AllPlanningTasks {
  items: PlanningTaskSummary[];
  total: number;
  project_totals: NonNullable<PlanningTaskListResponse['project_totals']>;
  unassigned_total: number;
  /** The page ceiling was hit: `items` is a prefix of the `total` tasks. */
  truncated: boolean;
}

/**
 * Every page of a task query, so a list and the counts it shows describe the
 * same set instead of "the rows one page held". The counts are those of the
 * first page: the backend computes the grouped ones there only, with the
 * query's own filter.
 */
export async function listAllPlanningTasks(
  filters: Omit<PlanningTaskListFilters, 'cursor' | 'limit'>,
  isCancelled: () => boolean = () => false,
): Promise<AllPlanningTasks | null> {
  const items: PlanningTaskSummary[] = [];
  let counts: Omit<AllPlanningTasks, 'items' | 'truncated'> | null = null;
  let cursor: number | undefined;
  for (let pages = 1; ; pages += 1) {
    const page = await planning.list({ ...filters, limit: PAGE_SIZE, cursor });
    if (isCancelled()) return null;
    items.push(...page.items);
    // The first page always carries the grouped counts; the fallback only
    // satisfies the optional type that later pages share.
    counts ??= {
      total: page.total,
      project_totals: page.project_totals ?? {},
      unassigned_total: page.unassigned_total ?? 0,
    };
    const next = page.next_cursor ?? undefined;
    // A cursor that does not move forward would loop on the same page.
    const exhausted = next === undefined || (cursor !== undefined && next <= cursor);
    if (exhausted) return { items, ...counts, truncated: false };
    if (pages >= MAX_PLANNING_PAGES) return { items, ...counts, truncated: true };
    cursor = next;
  }
}
