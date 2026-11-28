// KT-1880 — rules every component of components/ui/ holds to:
//   - its stylesheet uses the design tokens only and names no theme;
//   - each interactive part is at least --kr-control-h-touch (40px) on a
//     coarse pointer (a media query jsdom cannot evaluate, so it is read here);
//   - each component has its stylesheet and its test next to it.
import { describe, expect, it } from 'vitest';
import { readdirSync, readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadThemes, resolveValue } from '../../styles/themeAudit';
import { BREAKPOINTS } from '../../styles/designTokens';

const DIR = dirname(fileURLToPath(import.meta.url));
const SRC = join(DIR, '..', '..');
const files = readdirSync(DIR);
const CSS = files.filter(f => f.endsWith('.css')).map(file => ({
  file,
  // Comments are prose, not rules.
  text: readFileSync(join(DIR, file), 'utf8').replace(/\/\*[\s\S]*?\*\//g, ''),
}));
/** `@media (max-width: <bp - 1>px)`: a custom property cannot be read in a
 *  media query, so a breakpoint is the one px length a stylesheet writes. */
const BREAKPOINT_QUERY = /@media\s*\(max-width:\s*(\d+)px\)/g;
const BREAKPOINT_EDGES = new Set(Object.values(BREAKPOINTS).map(px => px - 1));
const THEME_NAMES = ['dark', 'arcade', 'light', 'matrix', 'gotham', 'sakura', 'euronews'];

/** The body of every `@media (pointer: coarse)` block of a stylesheet. */
function coarseBlocks(text: string): string[] {
  return mediaBlocks(text, /@media\s*\(pointer:\s*coarse\)\s*\{/g);
}

/** The body of every block of `text` whose opening `re` (global, ending on
 *  its `{`) matches. */
function mediaBlocks(text: string, re: RegExp): string[] {
  const blocks: string[] = [];
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    let depth = 1;
    let i = m.index + m[0].length;
    const start = i;
    while (depth > 0 && i < text.length) {
      if (text[i] === '{') depth++;
      else if (text[i] === '}') depth--;
      i++;
    }
    blocks.push(text.slice(start, i - 1));
  }
  return blocks;
}

/** The rules of `block`: each selector of its list, and its declarations. */
function rulesOf(block: string): { selectors: string[]; body: string }[] {
  return [...block.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .map(([, sel, body]) => ({ selectors: sel.split(',').map(s => s.trim()), body }));
}

/** Declarations of the rules of `block` whose selector list holds `selector`. */
function declsFor(block: string, selector: string): string {
  return [...block.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter(([, sel]) => sel.split(',').map(s => s.trim()).includes(selector))
    .map(([, , body]) => body).join(';');
}

describe('components/ui stylesheets', () => {
  it('finds the stylesheets, so the checks below are not vacuous', () => {
    expect(CSS.map(c => c.file).sort()).toEqual([
      'AgentIdentity.css', 'Button.css', 'CardHeader.css', 'ConfirmDialog.css', 'CopyButton.css', 'DetailHeader.css', 'Dialog.css',
      'DiffViewer.css', 'Drawer.css', 'DrawerHeader.css', 'EmptyState.css', 'Field.css', 'FormFooter.css', 'IconButton.css',
      'KeyValueList.css', 'ListHeader.css', 'LoadingState.css', 'LogViewer.css', 'Menu.css', 'Notice.css', 'ProgressBar.css',
      'SearchField.css', 'SectionHeader.css', 'Select.css', 'StatTile.css', 'StatusBadge.css', 'Switch.css',
      'TabHeader.css', 'Table.css', 'Tabs.css', 'Tooltip.css', 'headerFrame.css', 'overlay.css',
      'visuallyHidden.css',
    ]);
  });

  it('write no colour literal: every colour is a token', () => {
    for (const { file, text } of CSS) {
      expect(text.match(/#[0-9a-f]{3,8}\b/gi), `${file}: hex colour`).toBeNull();
      expect(text.match(/rgba?\(\s*\d/gi), `${file}: rgb() with literal channels`).toBeNull();
      expect(text.match(/:\s*(white|black)\b/gi), `${file}: named colour`).toBeNull();
    }
  });

  it('write no length in px but a 1px or 2px line: sizes come from the scales', () => {
    for (const { file, text } of CSS) {
      // A breakpoint query is checked on its own below.
      const rules = text.replace(BREAKPOINT_QUERY, '@media (max-width: bp)');
      const lengths = [...rules.matchAll(/(?<![\w-])-?(\d*\.?\d+)px\b/g)].map(m => Number(m[1]));
      expect(lengths.filter(px => px > 2), `${file}: px lengths`).toEqual([]);
    }
  });

  it('write a max-width query only at a breakpoint minus one (KT-1881)', () => {
    const queries = CSS.flatMap(({ file, text }) => [...text.matchAll(BREAKPOINT_QUERY)].map(m => ({ file, px: Number(m[1]) })));
    expect(queries.map(q => q.file)).toEqual(expect.arrayContaining(['Dialog.css', 'Drawer.css', 'FormFooter.css']));
    for (const { file, px } of queries) expect(BREAKPOINT_EDGES.has(px), `${file}: max-width ${px}px`).toBe(true);
  });

  it('keeps ConfirmDialog a card under sm: it takes none of the full-screen rules of Dialog (KT-1881)', () => {
    const SM_QUERY = /@media\s*\(max-width:\s*639px\)\s*\{/g;
    const narrowRules = (file: string) =>
      mediaBlocks(CSS.find(c => c.file === file)!.text, SM_QUERY).flatMap(rulesOf);
    const panel = (extra: string, size: string) => {
      const el = document.createElement('div');
      el.className = `ui-overlay-panel ui-dialog ${extra}`.trim();
      el.setAttribute('data-size', size);
      return el;
    };
    const matches = (el: Element, selectors: string[]) => selectors.some(sel => el.matches(sel));
    const FULL_SCREEN = /(?:^|;)\s*(?:width:\s*100%|height:\s*100%|max-height:\s*none|margin:\s*0|border-radius:\s*0)\s*(?:;|$)/;

    const dialogFullScreen = narrowRules('Dialog.css').filter(r => FULL_SCREEN.test(r.body));
    expect(dialogFullScreen.length, 'Dialog.css fills the screen under sm').toBeGreaterThan(0);
    for (const size of ['sm', 'md', 'lg']) {
      expect(matches(panel('', size), dialogFullScreen.flatMap(r => r.selectors)), `Dialog ${size} fills the screen`).toBe(true);
      const confirm = panel('ui-confirm-dialog', size);
      for (const { file } of CSS) {
        for (const rule of narrowRules(file).filter(r => matches(confirm, r.selectors))) {
          expect(rule.body, `${file} ${rule.selectors.join(', ')}: ConfirmDialog ${size}`).not.toMatch(FULL_SCREEN);
        }
      }
      // A card as wide as the window minus --kr-space-4 on each side, never
      // taller than the window minus the same margin.
      const own = narrowRules('ConfirmDialog.css').filter(r => matches(confirm, r.selectors)).map(r => r.body).join(';');
      expect(own, `ConfirmDialog ${size} margin`).toMatch(/margin:\s*var\(--kr-space-4\)/);
      expect(own, `ConfirmDialog ${size} width`).toMatch(/width:\s*calc\(100%\s*-\s*2\s*\*\s*var\(--kr-space-4\)\)/);
      expect(own, `ConfirmDialog ${size} max-height`).toMatch(/max-height:\s*calc\(100dvh\s*-\s*2\s*\*\s*var\(--kr-space-4\)\)/);
    }
  });

  it('take text sizes, durations and z-index from their tokens', () => {
    for (const { file, text } of CSS) {
      for (const [, value] of text.matchAll(/font-size:\s*([^;]+);/g)) {
        expect(value.trim(), `${file} font-size`).toMatch(/^var\(--kr-type-[a-z0-9]+\)$/);
      }
      for (const [, value] of text.matchAll(/(?:transition|animation)(?:-duration)?:\s*([^;]+);/g)) {
        expect(value.match(/\b\d+m?s\b/g), `${file}: literal duration in ${value.trim()}`).toBeNull();
      }
      for (const [, value] of text.matchAll(/z-index:\s*([^;]+);/g)) {
        expect(value.trim(), `${file} z-index`).toMatch(/^var\(--kr-z-[a-z-]+\)$/);
      }
    }
  });

  it('name no theme: the themes set the tokens', () => {
    for (const { file, text } of CSS) {
      expect(text, file).not.toMatch(/data-theme/);
      for (const theme of THEME_NAMES) expect(text, `${file} names ${theme}`).not.toMatch(new RegExp(`\\b${theme}\\b`));
    }
  });

  it.each([
    ['Button.css', '.ui-button', ['min-height']],
    ['Button.css', '.ui-button[data-size="sm"]', ['min-height']],
    ['IconButton.css', '.ui-icon-button', ['min-height', 'min-width']],
    ['IconButton.css', '.ui-icon-button[data-size="sm"]', ['min-height', 'min-width']],
    ['Menu.css', '.ui-menu-item', ['min-height']],
    ['Tabs.css', '.ui-tab', ['min-height']],
    ['Tabs.css', '.ui-tabs[data-size="sm"] .ui-tab', ['min-height']],
    ['SearchField.css', '.ui-search-field-input', ['min-height']],
    ['SearchField.css', '.ui-search-field[data-size="sm"] .ui-search-field-input', ['min-height']],
    ['Field.css', '.ui-field-control', ['min-height']],
    ['Select.css', '.ui-select-input', ['min-height']],
    ['Select.css', '.ui-select[data-size="sm"] .ui-select-input', ['min-height']],
    ['Switch.css', '.ui-switch-row', ['min-height']],
  ])('%s grows %s to the touch height on a coarse pointer', (file, selector, props) => {
    const text = CSS.find(c => c.file === file)!.text;
    const body = coarseBlocks(text).map(block => declsFor(block, selector)).join(';');
    for (const prop of props) {
      expect(body, `${file} ${selector} ${prop}`).toMatch(new RegExp(`${prop}:\\s*var\\(--kr-control-h-touch\\)`));
    }
  });

  it('holds the touch height at 40px in every theme, and lg above it', () => {
    const themes = loadThemes(SRC);
    for (const theme of THEME_NAMES) {
      const tokens = themes.get(theme)!;
      expect(resolveValue(tokens.get('--kr-control-h-touch')!, tokens), theme).toBe('40px');
      expect(parseFloat(resolveValue(tokens.get('--kr-control-h-lg')!, tokens)), theme).toBeGreaterThanOrEqual(40);
    }
  });

  it('gives every component its stylesheet and its test, next to it', () => {
    const components = files.filter(f => /^[A-Z]\w+\.tsx$/.test(f) && !f.endsWith('.test.tsx'));
    expect(components.sort()).toEqual([
      'AgentIdentity.tsx', 'Button.tsx', 'CardHeader.tsx', 'ConfirmDialog.tsx', 'CopyButton.tsx', 'DetailHeader.tsx', 'Dialog.tsx',
      'DiffViewer.tsx', 'Drawer.tsx', 'DrawerHeader.tsx', 'EmptyState.tsx', 'Field.tsx', 'FormFooter.tsx', 'IconButton.tsx',
      'KeyValueList.tsx', 'ListHeader.tsx', 'LoadingState.tsx', 'LogViewer.tsx', 'Menu.tsx', 'Notice.tsx', 'ProgressBar.tsx',
      'SearchField.tsx', 'SectionHeader.tsx', 'Select.tsx', 'StatTile.tsx', 'StatusBadge.tsx', 'Switch.tsx',
      'TabHeader.tsx', 'Table.tsx', 'Tabs.tsx', 'Tooltip.tsx',
    ]);
    for (const component of components) {
      const name = component.replace(/\.tsx$/, '');
      expect(files, `${name} stylesheet`).toContain(`${name}.css`);
      expect(files, `${name} test`).toContain(`${name}.test.tsx`);
    }
  });

  // KT-1936 review, finding 1, then round 2 — at 400 and 768 px the room
  // header's agent row first held a 388px trigger in a 310 / 358px row (the
  // participants off-screen), then a trigger squeezed to 89px with not one
  // letter of its name. Owner option A: the whole agent line scrolls
  // sideways, the « +N » counter pinned outside it; the trigger has a floor
  // (name and role chip whole, model at least 8 characters before the
  // ellipsis) and above it the model gives way first.
  it('scrolls the room header\'s agent line sideways and holds the trigger\'s floor', () => {
    const read = (path: string) => readFileSync(join(SRC, path), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    const page = read('pages/DiscussionsPage.css');
    const line = read('components/HeaderScrollLine.css');
    const strip = read('components/ParticipantsStrip.css');
    const decls = (text: string, selector: string) => new Map(
      declsFor(text, selector).split(';').map(d => d.trim()).filter(Boolean)
        .map(d => [d.slice(0, d.indexOf(':')).trim(), d.slice(d.indexOf(':') + 1).trim()] as const),
    );

    // The line: it fits its row, its scroller scrolls sideways only, with no
    // scrollbar, and keeps the header's height (the focus rings' room is
    // given back by a negative margin).
    const lineBox = decls(line, '.kr-header-line');
    expect(lineBox.get('min-width')).toBe('0');
    expect(lineBox.get('container-type')).toBe('inline-size');
    const scroller = decls(line, '.kr-header-line-scroller');
    expect(scroller.get('min-width')).toBe('0');
    expect(scroller.get('overflow-x')).toBe('auto');
    expect(scroller.get('overflow-y')).toBe('hidden');
    expect(scroller.get('scrollbar-width')).toBe('none');
    expect(scroller.get('margin-block')).toBe(`calc(-1 * ${scroller.get('padding-block')})`);
    // Its content at its own width, on one line: nothing wraps, nothing shrinks.
    const track = decls(line, '.kr-header-line-track');
    expect(track.get('flex-wrap')).toBe('nowrap');
    expect(track.get('width')).toBe('max-content');
    expect(decls(line, '.kr-header-line-track > *').get('flex')).toBe('0 0 auto');
    // The fade while it scrolls further right; the end slot never shrinks.
    expect(decls(line, '.kr-header-line[data-fade-end] .kr-header-line-scroller').get('mask-image'))
      .toContain('transparent');
    expect(decls(line, '.kr-header-line-end').get('flex')).toBe('0 0 auto');
    // The strip does not scroll by itself on the line.
    expect(decls(line, '.kr-header-line .kr-participants-strip-list').get('overflow')).toBe('visible');

    // The trigger's floor: no part of the chain shrinks or is capped …
    for (const part of ['.disc-native-agent-control', '.kr-agent-switch', '.kr-agent-switch-btn', '.kr-agent-switch-identity']) {
      const rule = decls(page, `.kr-header-line ${part}`);
      expect(rule.get('flex-shrink'), part).toBe('0');
      expect(rule.get('max-width'), part).toBe('none');
    }
    // … the name and the role chip are whole …
    const trigger = '.kr-header-line .kr-agent-switch-identity.ui-agent-identity';
    for (const part of ['name', 'role']) {
      const rule = decls(page, `${trigger} .ui-agent-identity-${part}`);
      expect(rule.get('flex-shrink'), part).toBe('0');
      expect(rule.get('overflow'), part).toBe('visible');
    }
    // … and the model is 9ch at least (8 characters and the ellipsis), 17 at
    // most, between the two following the line's width.
    const model = decls(page, `${trigger} .ui-agent-identity-model`);
    expect(model.get('max-width')).toMatch(/^clamp\(9ch, 100cqw - [^,]+, 17ch\)$/);
    expect(model.get('flex-shrink')).toBe('0');
    expect(decls(page, '.disc-native-agent-toggle').get('flex-shrink')).toBe('0');

    // The counter never shrinks; the full list is a fixed layer, not absolute in the row.
    expect(decls(strip, '.kr-participants-strip-more').get('flex-shrink')).toBe('0');
    expect(decls(strip, '.kr-participants-strip-all').get('position')).toBeUndefined();
  });

  // KT-1936 review round 3 — the participant details layer (320px) took a
  // full-size identity wider than itself: the close button was pushed out of
  // the layer (at 400px, out of the window). Its header is a flex row whose
  // identity shrinks and wraps, and whose close button never shrinks.
  it('keeps the participant details\' close button inside the layer', () => {
    const css = readFileSync(join(SRC, 'styles/components.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    const decls = (selector: string) => new Map(
      declsFor(css, selector).split(';').map(d => d.trim()).filter(Boolean)
        .map(d => [d.slice(0, d.indexOf(':')).trim(), d.slice(d.indexOf(':') + 1).trim()] as const),
    );
    const header = decls('.disc-participant-details > header');
    expect(header.get('display')).toBe('flex');
    expect(header.get('flex-wrap') ?? 'nowrap').toBe('nowrap');
    // The identity's box can shrink below its content …
    const identityBox = decls('.disc-participant-details > header > strong');
    expect(identityBox.get('min-width')).toBe('0');
    expect(identityBox.get('flex')).toBe('1 1 auto');
    // … and wraps rather than overflows; the name and the role chip may break
    // only when one of them alone is wider than the layer.
    const identity = decls('.disc-participant-details .disc-participant-details-identity');
    expect(identity.get('flex-wrap')).toBe('wrap');
    for (const part of ['name', 'role']) {
      const rule = decls(`.disc-participant-details .disc-participant-details-identity .ui-agent-identity-${part}`);
      expect(rule.get('min-width'), part).toBe('0');
      expect(rule.get('white-space'), part).toBe('normal');
    }
    // The model keeps its ellipsis (AgentIdentity.css).
    const model = declsFor(CSS.find(c => c.file === 'AgentIdentity.css')!.text, '.ui-agent-identity-model');
    expect(model).toContain('min-width: 0');
    expect(model).toContain('text-overflow: ellipsis');
    // The button never shrinks.
    expect(decls('.disc-participant-details > header > button').get('flex-shrink')).toBe('0');
  });
});
