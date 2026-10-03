import type { QuickApi, QuickPrompt } from '../types/generated';
import { AUTOMATION_KIND_FILTERS, type AutomationKind } from './automationFilters';

export type QuickPromptSort = 'name' | 'updated' | 'usage';
export type QuickApiSort = 'name' | 'updated' | 'endpoint';
/** Sort of the sidebar of the global Automation page (every kind mixed). The
 *  ⋯ menu offers `name`, `updated` and `opened`; `kind` orders by type. */
export type AutomationSort = 'name' | 'updated' | 'kind' | 'opened';

const collator = new Intl.Collator(undefined, {
  sensitivity: 'base',
  numeric: true,
});

const byName = (a: { name: string }, b: { name: string }) =>
  collator.compare(a.name, b.name);

export function sortQuickPrompts(
  prompts: QuickPrompt[],
  sort: QuickPromptSort,
  usageById: Readonly<Record<string, number>>,
  reversed = false,
): QuickPrompt[] {
  return [...prompts].sort((a, b) => {
    let result: number;
    if (sort === 'updated') {
      result = b.updated_at.localeCompare(a.updated_at) || byName(a, b);
    } else if (sort === 'usage') {
      result = (usageById[b.id] ?? 0) - (usageById[a.id] ?? 0) || byName(a, b);
    } else {
      result = byName(a, b);
    }
    return reversed ? -result : result;
  });
}

export function sortQuickApis(
  apis: QuickApi[],
  sort: QuickApiSort,
  reversed = false,
): QuickApi[] {
  return [...apis].sort((a, b) => {
    let result: number;
    if (sort === 'updated') {
      result = b.updated_at.localeCompare(a.updated_at) || byName(a, b);
    } else if (sort === 'endpoint') {
      result = collator.compare(a.api_plugin_slug, b.api_plugin_slug)
        || collator.compare(a.api_endpoint_path, b.api_endpoint_path)
        || byName(a, b);
    } else {
      result = byName(a, b);
    }
    return reversed ? -result : result;
  });
}

export interface SortableAutomation {
  kind: AutomationKind;
  name: string;
  pinned: boolean;
  updatedAt: string;
  /** Epoch ms of the last opening from the sidebar; none means "never". */
  lastOpenedAt?: number | null;
}

/**
 * Orders the automations of the global page. Favorites stay first whatever the
 * criterion, as they always were; the criterion and its direction order each
 * side of that line, so "reverse" never sinks a favorite below the rest.
 *
 * `opened` puts the most recently opened first and what was never opened
 * after it, by name. The "Recent" chip sorts that way without the favorites
 * line (`pinnedFirst: false`): it is a history, not a library.
 */
export function sortAutomationResources<T extends SortableAutomation>(
  resources: readonly T[],
  sort: AutomationSort,
  reversed = false,
  { pinnedFirst = true }: { pinnedFirst?: boolean } = {},
): T[] {
  return [...resources].sort((a, b) => {
    let result: number;
    if (sort === 'updated') {
      result = b.updatedAt.localeCompare(a.updatedAt) || byName(a, b);
    } else if (sort === 'kind') {
      result = AUTOMATION_KIND_FILTERS.indexOf(a.kind) - AUTOMATION_KIND_FILTERS.indexOf(b.kind) || byName(a, b);
    } else if (sort === 'opened') {
      // A real timestamp is always above 0, which stands for "never opened".
      result = (b.lastOpenedAt ?? 0) - (a.lastOpenedAt ?? 0) || byName(a, b);
    } else {
      result = byName(a, b);
    }
    return (pinnedFirst ? Number(b.pinned) - Number(a.pinned) : 0) || (reversed ? -result : result);
  });
}
