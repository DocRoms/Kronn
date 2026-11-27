// KT-1885 review — the picker's stylesheet: tier choices whose text wraps
// rather than being cut and never under 24 px, a 24 px compact chip, an
// exact-model popover wide enough for its placeholder, content-width shadow
// rows, an @ menu as tall as the one it replaced, and the rules this lot adds
// on the design tokens only (text on --kr-type-*, spacing on --kr-space-*).
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import fr from '../../lib/i18n/locales/fr';
import en from '../../lib/i18n/locales/en';
import es from '../../lib/i18n/locales/es';
import zh from '../../lib/i18n/locales/zh';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const read = (path: string) => readFileSync(join(SRC, path), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
const PICKER = read('components/AgentSwitchPicker.css');

/** The declarations of the rule whose selector is exactly `selector`. */
function rule(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = css.match(new RegExp(`(?:^|\\n)\\s*${escaped}\\s*\\{([^}]*)\\}`));
  expect(match, `missing rule ${selector}`).not.toBeNull();
  return match?.[1] ?? '';
}

const declaration = (body: string, property: string) =>
  body.match(new RegExp(`(?:^|[;\\s])${property}\\s*:\\s*([^;]+);`))?.[1].trim();

describe('AgentSwitchPicker.css — tier choices (KT-1885 review)', () => {
  it('a tier choice is never under 24 px and has no fixed height', () => {
    const body = rule(PICKER, '.kr-agent-switch-tier-option');
    // 26 px, a margin over the 24 px floor whatever rounds it; border-box (reset.css).
    expect(declaration(body, 'min-height')).toBe('calc(var(--kr-space-6) + var(--kr-space-1) / 2)');
    expect(declaration(body, 'flex-shrink')).toBe('0');
    expect(declaration(body, 'height')).toBeUndefined();
    // No other rule shrinks it back to a fixed height (the @ menu's used to).
    for (const [, selectors, block] of PICKER.matchAll(/([^{}]+)\{([^}]*)\}/g)) {
      if (selectors.includes('.kr-agent-switch-tier-option')) expect(declaration(block, 'height')).toBeUndefined();
    }
  });

  it('its catalogue state wraps instead of being cut with an ellipsis', () => {
    const body = rule(PICKER, '.kr-agent-switch-catalog-meta');
    expect(body).not.toMatch(/text-overflow\s*:\s*ellipsis/);
    expect(body).not.toMatch(/white-space\s*:\s*nowrap/);
    expect(body).not.toMatch(/position\s*:\s*absolute/);
    expect(declaration(body, 'white-space')).toBe('normal');
  });
});

// KT-1885 review 2 — the row layout the principal decided: one line per agent
// (name and ONE state badge, then the tiers), tiers on equal tracks shared by
// every row, one-line tier buttons, a 480 px popover, stacked under 420 px.
describe('AgentSwitchPicker.css — one line per agent, tiers on equal tracks (KT-1885 review 2)', () => {
  it('a tier row and a palette row put the agent on the left and the tiers on the right', () => {
    for (const selector of ['.kr-agent-switch-tier-row', '.kr-agent-switch-palette-row']) {
      const body = rule(PICKER, selector);
      expect(declaration(body, 'display')).toBe('grid');
      expect(declaration(body, 'grid-template-columns')).toBe('minmax(0, 1fr) auto');
      expect(body).not.toMatch(/min-height/);
    }
  });

  it('the tiers sit on equal tracks, the same in every row, as many as the component says', () => {
    const body = rule(PICKER, '.kr-agent-switch-tier-choices');
    expect(declaration(body, 'display')).toBe('grid');
    expect(declaration(body, 'grid-template-columns'))
      .toBe('repeat(var(--agent-switch-tier-count, 3), minmax(var(--agent-switch-tier-track), max-content))');
    expect(declaration(body, '--agent-switch-tier-track')).toBe('88px');
    // A button fills its track; its icon and name stay on one line.
    expect(declaration(rule(PICKER, '.kr-agent-switch-tier-option'), 'width')).toBe('100%');
    expect(declaration(rule(PICKER, '.kr-agent-switch-tier-label'), 'white-space')).toBe('nowrap');
    // A tier's own state wraps inside its track rather than widening it.
    expect(declaration(rule(PICKER, '.kr-agent-switch-tier-option > .kr-agent-switch-catalog-meta'), 'max-width'))
      .toBe('calc(var(--agent-switch-tier-track) - 2 * var(--kr-space-1) - 2px)');
  });

  it('the agent side never cuts its text: the name, the kind and the badge wrap', () => {
    for (const selector of ['.kr-agent-switch-tier-agent', '.kr-agent-switch-palette-main']) {
      expect(declaration(rule(PICKER, selector), 'flex-wrap')).toBe('wrap');
    }
    for (const selector of ['.kr-agent-switch-tier-name', '.kr-agent-switch-palette-label', '.kr-agent-switch-palette-detail']) {
      const body = rule(PICKER, selector);
      expect(body).not.toMatch(/text-overflow|white-space\s*:\s*nowrap|overflow\s*:\s*hidden/);
      expect(declaration(body, 'overflow-wrap')).toBe('anywhere');
    }
  });

  it('a catalogue state badge is neutral; unavailable keeps its danger colour', () => {
    const catalog = rule(PICKER, ".kr-agent-switch-badge[data-state='catalog']");
    expect(declaration(catalog, 'color')).toBe('var(--kr-text-tertiary)');
    expect(PICKER).toMatch(/\.kr-agent-switch-badge\[data-state='unavailable'\][^{]*\{[^}]*color: var\(--kr-danger\)/);
  });

  it('the popover is 480 px at most, never wider than the window', () => {
    expect(declaration(rule(PICKER, '.kr-agent-switch-popover'), 'max-width'))
      .toBe('min(480px, calc(100vw - 2 * var(--kr-space-2)))');
  });

  it('under 420 px the tiers go under the agent, in equal parts', () => {
    const media = PICKER.match(/@media \(max-width: 419px\) \{([\s\S]*?)\n\}/)?.[1] ?? '';
    expect(media).toMatch(/\.kr-agent-switch-tier-row,\s*\.kr-agent-switch-palette-row\s*\{\s*grid-template-columns: minmax\(0, 1fr\);/);
    expect(media).toMatch(/\.kr-agent-switch-tier-choices\s*\{\s*grid-template-columns: repeat\(var\(--agent-switch-tier-count, 3\), minmax\(0, 1fr\)\);/);
  });
});

// KT-1885 review 4 — the dot and the name are one box that never splits across
// lines; the state badge sits beside the name when there is room, under it only
// when there is not (the name must not take the whole line).
describe('AgentSwitchPicker.css — the agent identity keeps its dot with its name (KT-1885 review 4)', () => {
  const TSX = read('components/AgentSwitchPicker.tsx');

  it('the markup wraps the dot and the name in one identity box', () => {
    expect(TSX).toMatch(
      /className="kr-agent-switch-tier-id">\s*<span\s+className="kr-agent-switch-option-dot"[\s\S]*?<span className="kr-agent-switch-tier-name">/,
    );
  });

  it('the identity box is one inline-flex line that takes only its own width', () => {
    const id = rule(PICKER, '.kr-agent-switch-tier-id');
    expect(declaration(id, 'display')).toBe('inline-flex');
    expect(declaration(id, 'flex')).toBe('0 1 auto');
    expect(declaration(id, 'max-width')).toBe('100%');
  });

  it('the name takes its own width, not the whole line, so the badge can sit beside it', () => {
    const name = rule(PICKER, '.kr-agent-switch-tier-name');
    expect(declaration(name, 'flex')).toBe('0 1 auto');
    expect(name).not.toMatch(/calc\(100%/);
    expect(declaration(name, 'overflow-wrap')).toBe('anywhere');
  });
});

describe('AgentSwitchPicker.css — triggers and popovers (KT-1885 review)', () => {
  it('the compact chip is a 24 px target', () => {
    expect(declaration(rule(PICKER, ".kr-agent-switch[data-compact='true'] > .kr-agent-switch-btn"), 'min-height'))
      .toBe('var(--kr-space-6)');
  });

  // KT-1994 — the exact-model mode and the tier mode are one grouped list, one width.
  it('the grouped list is 440 px wide, within the window, and the model search placeholder is short enough for it', () => {
    expect(declaration(rule(PICKER, '.kr-agent-switch-popover:has(.kr-agent-switch-mode)'), 'width'))
      .toBe('min(440px, calc(100vw - 2 * var(--kr-space-2)))');
    expect(PICKER).not.toMatch(/\.kr-agent-switch-popover\[data-mode='exact-model'\]\s*\{/);
    for (const dictionary of [fr, en, es, zh]) {
      expect((dictionary as Record<string, string>)['agentPicker.searchModel'].length).toBeLessThanOrEqual(32);
    }
  });

  it('a shadow row’s picker is as wide as its text, not the step routing’s full width', () => {
    const rows = read('components/workflows/StepShadowRows.css');
    expect(declaration(rule(rows, '.wf-shadow-rows .kr-agent-switch.wf-shadow-picker'), 'width')).toBe('auto');
    expect(declaration(rule(rows, '.wf-shadow-rows .wf-shadow-picker .kr-agent-switch-btn'), 'width')).toBe('auto');
    // Three classes beat the two of `.wf-step-agent-routing .kr-agent-switch { width: 100% }`.
    expect(declaration(rule(read('pages/WorkflowsPage.css'), '.wf-step-agent-routing .kr-agent-switch'), 'width')).toBe('100%');
  });

  it('the @ menu scrolls inside; its stylesheet cap only holds until the palette measures its room', () => {
    const body = rule(read('pages/DiscussionsPage.css'), '.disc-mention-popover.kr-agent-switch-palette');
    // The real bound is measured (AgentPickerPalette `boundary`, see AgentSwitchPicker.modes.test.tsx).
    expect(declaration(body, 'max-height')).toBe('calc(100vh - 2 * var(--kr-space-8))');
    expect(declaration(body, 'overflow-y')).toBe('auto');
  });
});

describe('AgentSwitchPicker.css — the rules this lot adds stay on the tokens (KT-1885 review)', () => {
  const OWN_RULES: [file: string, selector: string][] = [
    ['components/AgentSwitchPicker.css', ".kr-agent-switch[data-variant='field'] .kr-agent-switch-btn"],
    ['components/AgentSwitchPicker.css', ".kr-agent-switch[data-compact='true'] > .kr-agent-switch-btn"],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-search'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-tier-option'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-tier-label'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-catalog-meta'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-check'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-group'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-group-block'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-model'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-results > .kr-agent-switch-model'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-hint'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-palette'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-palette-row'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-palette-main'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-palette-label'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-palette-detail'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-tier-row'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-tier-agent'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-tier-choices'],
    ['components/AgentSwitchPicker.css', ".kr-agent-switch-badge[data-state='catalog']"],
    // KT-1994 — the grouped list's rows.
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-group-header'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-mode'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-mode-label'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-mode-value'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-more-toggle'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-recent-choice'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-custom'],
    ['components/AgentSwitchPicker.css', '.kr-agent-switch-tier-model'],
    ['components/aiHelper.css', '.wf-apicall-ai-agent-selector .kr-agent-switch-btn'],
    ['pages/DiscussionsPage.css', '.disc-mention-popover.kr-agent-switch-palette'],
  ];

  it.each(OWN_RULES)('%s %s: text on --kr-type-*, spacing on --kr-space-*', (file, selector) => {
    const body = rule(read(file), selector);
    expect(body).not.toMatch(/--kr-fs-|--kr-sp-/);
    const fontSize = declaration(body, 'font-size');
    if (fontSize) expect(fontSize).toMatch(/^var\(--kr-type-(sm|md|lg|xl|2xl)\)$/);
    for (const [, , value] of body.matchAll(/(?:^|[;\s])((?:padding|margin|gap)[\w-]*)\s*:\s*([^;]+);/g)) {
      expect(value, `${selector}: raw spacing « ${value} »`).not.toMatch(/\b[1-9]\d*px\b/);
    }
  });

  it('the model id is written in the code face, whole, at the 12 px floor', () => {
    const body = rule(PICKER, '.kr-agent-switch-mode-model');
    expect(declaration(body, 'font-family')).toBe('var(--kr-font-mono)');
    expect(declaration(body, 'font-size')).toBe('var(--kr-type-sm)');
    expect(declaration(body, 'overflow-wrap')).toBe('anywhere');
    expect(body).not.toMatch(/text-overflow|white-space\s*:\s*nowrap/);
  });

  // KT-1994 review, point 5 — the @ menu's cell never breaks inside a token:
  // only between the id's parts (a <wbr> after `/` and `:`), two lines at most.
  it('the @ menu cell breaks an id only between its parts, on two lines at most', () => {
    const cell = rule(PICKER, '.kr-agent-switch-tier-model');
    expect(cell).not.toMatch(/overflow-wrap|word-break/);
    expect(declaration(cell, 'font-size')).toBe('var(--kr-type-sm)');
    expect(declaration(cell, '-webkit-line-clamp')).toBe('2');
    expect(declaration(cell, 'overflow')).toBe('hidden');
    expect(declaration(rule(PICKER, 'code.kr-agent-switch-tier-model'), 'font-family')).toBe('var(--kr-font-mono)');
    const part = rule(PICKER, '.kr-agent-switch-id-part');
    expect(declaration(part, 'white-space')).toBe('nowrap');
    expect(declaration(part, 'text-overflow')).toBe('ellipsis');
  });

  // KT-1994 review round 3: measured in the browser, the 12px mono face is
  // 7.22px a character, so a 25-character id needs a 180.5px text box.
  it('from a 748px palette content box the tiers sit in 192px tracks (182px text box, a 25-character id on one line); narrower, three equal columns', () => {
    expect(declaration(rule(PICKER, '.kr-agent-switch-palette'), 'container-type')).toBe('inline-size');
    expect(declaration(rule(PICKER, ".kr-agent-switch-tier-choices[data-models='true']"), 'grid-template-columns'))
      .toBe('repeat(var(--agent-switch-tier-count, 3), minmax(0, 1fr))');
    const wide = PICKER.match(/@container \(min-width: 748px\) \{([\s\S]*?)\n\}/)?.[1] ?? '';
    const tracks = rule(wide, ".kr-agent-switch-tier-choices[data-models='true']");
    expect(declaration(tracks, '--agent-switch-tier-track')).toBe('192px');
    expect(declaration(tracks, 'grid-template-columns'))
      .toBe('repeat(var(--agent-switch-tier-count, 3), var(--agent-switch-tier-track))');
    // The cell takes 2 x (4px padding + 1px border) from its track: 192 - 10 = 182px.
    const cell = rule(PICKER, '.kr-agent-switch-tier-option');
    expect(declaration(cell, 'padding')).toBe('0 var(--kr-space-1)');
    expect(declaration(cell, 'border')).toBe('1px solid var(--kr-border-ghost)');
    const textBox = 192 - 2 * (4 + 1);
    expect(textBox).toBeGreaterThanOrEqual(182);
    expect(25 * 7.22).toBeLessThanOrEqual(textBox);
    // Threshold: 3 tracks + 2 gaps of 4px, the row's 8px gap and 16px padding,
    // and an agent column of at least 140px.
    expect(declaration(rule(PICKER, '.kr-agent-switch-tier-choices'), 'gap')).toBe('var(--kr-space-1)');
    expect(3 * 192 + 2 * 4 + 8 + 16 + 140).toBe(748);
  });

  it('from a 768px window the @ menu takes up to 760px when its anchor allows, so the agent column keeps 142px', () => {
    const page = read('pages/DiscussionsPage.css');
    const wide = page.match(/@media \(min-width: 768px\) \{([\s\S]*?)\n\}/)?.[1] ?? '';
    expect(declaration(rule(wide, '.disc-mention-popover.kr-agent-switch-palette'), 'width'))
      .toBe('min(760px, 100%, calc(100vw - 2 * var(--kr-space-4)))');
    // 760 - 2 x (1px border + 4px padding) = 750px content box, at least the 748px threshold.
    // The palette's box rule is its second block (the first holds only the
    // container query's type).
    const first = PICKER.indexOf('\n.kr-agent-switch-palette {');
    const palette = rule(PICKER.slice(first + 1).replace(/^\.kr-agent-switch-palette \{[^}]*\}/, ''), '.kr-agent-switch-palette');
    expect(declaration(palette, 'padding')).toBe('var(--kr-space-1)');
    expect(declaration(palette, 'border')).toBe('1px solid var(--kr-border)');
    const contentBox = 760 - 2 * (1 + 4);
    expect(contentBox).toBeGreaterThanOrEqual(748);
    expect(contentBox - 16 - 8 - (3 * 192 + 2 * 4)).toBe(142);
  });

  // KT-1994 review round 4: in a 768px window the 520px floor
  // (`max(100%, 520px)`) pushed the palette 125px past `.disc-messages-col`
  // (overflow hidden) and cut the « Avancé » column.
  it('the @ menu is never wider than its anchor: every width expression is capped by 100% and holds no floor', () => {
    const page = read('pages/DiscussionsPage.css');
    const wide = page.match(/@media \(min-width: 768px\) \{([\s\S]*?)\n\}/)?.[1] ?? '';
    const widths = [
      declaration(rule(page, '.disc-mention-popover.kr-agent-switch-palette'), 'width'),
      declaration(rule(wide, '.disc-mention-popover.kr-agent-switch-palette'), 'width'),
    ];
    expect(widths[0]).toBe('min(520px, 100%, calc(100vw - 2 * var(--kr-space-4)))');
    for (const width of widths) {
      // A top-level min() holding 100% among its terms, and no max()/clamp() floor.
      expect(width).toMatch(/^min\((?:[^,]+,\s*)*100%(?:,\s*[^,]+)*\)$/);
      expect(width).not.toMatch(/max\(|clamp\(|min-width/);
    }
    expect(declaration(rule(page, '.disc-mention-popover.kr-agent-switch-palette'), 'min-width')).toBeFalsy();
    // `100%` is the anchor's width because both anchors are the palette's
    // containing block and it starts at their left edge.
    expect(declaration(rule(page, '.disc-mention-popover'), 'left')).toBe('0');
    expect(declaration(rule(page, '.disc-mention-popover'), 'position')).toBe('absolute');
    expect(declaration(rule(page, '.disc-composer'), 'position')).toBe('relative');
    expect(declaration(rule(page, '.disc-new-prompt-wrap'), 'position')).toBe('relative');
  });

  // KT-1994 review, point 6.
  it('the status line, the blocked reason and a recent agent read at the 12 px floor; a recent agent stays on one line', () => {
    expect(declaration(rule(PICKER, '.kr-agent-switch-status'), 'font-size')).toBe('var(--kr-type-sm)');
    expect(declaration(rule(PICKER, '.kr-agent-switch-reason'), 'font-size')).toBe('var(--kr-type-sm)');
    expect(declaration(rule(PICKER, '.kr-agent-switch-recent .kr-agent-switch-tier-name'), 'white-space')).toBe('nowrap');
    expect(declaration(rule(PICKER, '.kr-agent-switch-recent'), 'grid-template-columns'))
      .toBe('max-content minmax(0, 1fr) var(--kr-space-3)');
  });

  it('the availability badge reads at the 12 px floor', () => {
    expect(declaration(rule(PICKER, '.kr-agent-switch-badge'), 'font-size')).toBe('var(--kr-type-sm)');
  });
});
