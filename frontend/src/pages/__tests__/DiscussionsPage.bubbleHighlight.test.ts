// KT-1344 — the search-current warning ring belongs to ONE bubble, the row the
// search jumped to. A leftover unscoped `.disc-msg-bubble` rule with the same
// declarations once ringed every bubble of every discussion.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const css = readFileSync(resolve(process.cwd(), 'src/pages/DiscussionsPage.css'), 'utf8');

function rulesStyling(selectorPart: string): { selector: string; body: string }[] {
  const rules: { selector: string; body: string }[] = [];
  const pattern = /([^{}]+)\{([^{}]*)\}/g;
  for (const match of css.matchAll(pattern)) {
    const selector = match[1].trim();
    if (selector.split(',').some(part => part.includes(selectorPart))) {
      rules.push({ selector, body: match[2] });
    }
  }
  return rules;
}

describe('discussion bubble search highlight', () => {
  it('never rings a bubble with the warning colour unless it is the search-current one', () => {
    const offenders = rulesStyling('.disc-msg-bubble')
      .filter(rule => /--kr-warning-rgb/.test(rule.body))
      .filter(rule => !rule.selector.includes('[data-search-current="true"]'))
      // A role/variant-scoped rule may use the warning tone for what the
      // message IS; only a bare bubble is a leak.
      .filter(rule => !/\[data-(role|variant|editing)/.test(rule.selector));
    expect(offenders).toEqual([]);
  });

  it('keeps the scoped search-current ring', () => {
    const scoped = rulesStyling('.disc-msg-row[data-search-current="true"] .disc-msg-bubble');
    expect(scoped).toHaveLength(1);
    expect(scoped[0].body).toContain('--kr-warning-rgb');
  });
});
