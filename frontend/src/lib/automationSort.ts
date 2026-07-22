import type { QuickApi, QuickPrompt } from '../types/generated';

export type QuickPromptSort = 'name' | 'updated' | 'usage';
export type QuickApiSort = 'name' | 'updated' | 'endpoint';

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
