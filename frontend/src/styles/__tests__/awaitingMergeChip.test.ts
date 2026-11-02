// KT-1519 — the "integrated into <branch>, awaiting merge" chip is the longest
// chip of a task row. Captured on the Planning sidebar it broke the reference
// (« KT- / 1519 »), slid under the star and menu and overflowed the card; in the
// discussion plan panel it stayed grey. These tests read the real CSS so the
// layout and the warning tone cannot regress silently.

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const css = (...path: string[]) => readFileSync(join(__dirname, '..', '..', ...path), 'utf8');
const PLANNING = css('pages', 'PlanningPage.css');
const PLAN_PANEL = css('components', 'DiscussionPlanPanel.css');

const rule = (source: string, selector: string): string => {
  const start = source.indexOf(`${selector} {`);
  expect(start, `${selector} rule exists`).toBeGreaterThanOrEqual(0);
  return source.slice(start, source.indexOf('}', start));
};

describe('awaiting-merge chip layout and tone', () => {
  it('shrinks the sidebar chip with an ellipsis instead of overflowing the card', () => {
    const chip = rule(PLANNING, '.planning-sidebar-awaiting');
    expect(chip).toMatch(/max-width:\s*100%/);
    expect(chip).toMatch(/overflow:\s*hidden/);
    expect(chip).toMatch(/text-overflow:\s*ellipsis/);
    expect(chip).toMatch(/display:\s*block/);
  });

  it('never wraps the task reference', () => {
    const ref = rule(PLANNING, '.planning-ref');
    expect(ref).toMatch(/white-space:\s*nowrap/);
    expect(ref).toMatch(/flex:\s*0 0 auto/);
  });

  it('uses the warning tone on the planning page chip', () => {
    expect(rule(PLANNING, '.planning-status[data-status="awaiting_merge"]')).toMatch(/color:\s*var\(--kr-warning\)/);
  });

  it('uses the same warning tone in both discussion plan panel views', () => {
    expect(rule(PLAN_PANEL, '.plan-all-task-meta > span[data-status="awaiting_merge"]')).toMatch(/color:\s*var\(--kr-warning\)/);
    expect(rule(PLAN_PANEL, '.plan-task-meta > span[data-status="awaiting_merge"]')).toMatch(/color:\s*var\(--kr-warning\)/);
  });
});
