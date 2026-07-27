import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { TOUR_STEPS } from '../tourSteps';

/**
 * KT-117 — a tour step is a contract with the UI, and nothing enforced it.
 * `usage-header` was renamed to `settings-usage` and the step kept pointing at
 * the old name: the step went silently dead, and until the overlay fix a dead
 * step froze the app behind an invisible backdrop.
 *
 * This walks the real source instead of a hand-maintained list, so renaming an
 * anchor without updating the tour fails here rather than in a user's browser.
 */
const SRC = join(__dirname, '..', '..', '..');

function collectSources(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    if (entry === 'node_modules' || entry.startsWith('.')) continue;
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) collectSources(full, out);
    else if (/\.tsx?$/.test(entry) && !/\.test\.tsx?$/.test(entry)) out.push(full);
  }
  return out;
}

const sources = collectSources(SRC)
  .filter(path => !path.includes(join('components', 'tour')))
  .map(path => readFileSync(path, 'utf8'));

/** Anchors built by template literal, e.g. ``data-tour-id={`nav-${id}`}``. */
function hasDynamicAnchor(id: string): boolean {
  return sources.some(source =>
    [...source.matchAll(/data-tour-id=\{`([^`$]*)\$\{/g)]
      .some(match => id.startsWith(match[1]) && match[1].length > 0),
  );
}

describe('tour anchors exist in the app', () => {
  const anchored = TOUR_STEPS
    .flatMap(step => [
      { id: step.id, selector: step.selector },
      ...(step.secondarySelectors ?? []).map((selector, index) => ({
        id: `${step.id} (secondary ${index + 1})`,
        selector,
      })),
    ])
    .filter((step): step is { id: string; selector: string } => !!step.selector);

  it('every data-tour-id a step targets is present in the source', () => {
    const missing = anchored
      .map(step => ({ step, tourId: /\[data-tour-id="([^"]+)"\]/.exec(step.selector)?.[1] }))
      .filter((entry): entry is { step: { id: string; selector: string }; tourId: string } =>
        !!entry.tourId)
      .filter(({ tourId }) =>
        !sources.some(source => source.includes(`data-tour-id="${tourId}"`))
        && !sources.some(source => source.includes(`'data-tour-id': '${tourId}'`))
        && !hasDynamicAnchor(tourId))
      .map(({ step, tourId }) => `${step.id} → ${tourId}`);

    expect(missing, 'tour steps pointing at an anchor no longer in the UI').toEqual([]);
  });

  it('every class or id selector a step targets is present in the source', () => {
    const missing = anchored
      .filter(step => !step.selector.startsWith('[data-tour-id'))
      .filter(step => {
        // `.foo` → `className="… foo …"`; `#foo` → `id="foo"`.
        const token = step.selector.replace(/^[.#]/, '').split(/[\s>:[]/)[0];
        return !sources.some(source => source.includes(token));
      })
      .map(step => `${step.id} → ${step.selector}`);

    expect(missing, 'tour steps pointing at a class/id no longer in the UI').toEqual([]);
  });

  it('no step depends on data a brand-new user does not have yet', () => {
    // The profiles CHIP only carries its anchor on the first item of a list, so
    // a user with zero profiles — the tour's only audience — had no target.
    // Anchor such steps on the durable container instead.
    const conditional = TOUR_STEPS
      .filter(step => step.selector?.includes('profile-chip'))
      .map(step => step.id);
    expect(conditional, 'anchor on the container, not on a list row').toEqual([]);
  });
});
