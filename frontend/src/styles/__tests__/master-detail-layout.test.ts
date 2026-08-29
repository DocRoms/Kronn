import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const readCss = (path: string) => readFileSync(resolve(process.cwd(), path), 'utf8');

const ruleBody = (css: string, selector: string): string => {
  const escapedSelector = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = css.match(new RegExp(`${escapedSelector}\\s*\\{([^}]+)\\}`));
  expect(match, `Missing CSS rule for ${selector}`).not.toBeNull();
  return match?.[1] ?? '';
};

const fullBleedMargin = [
  'calc(-1 * var(--dash-main-padding-block))',
  'calc(-1 * var(--dash-main-padding-inline))',
];

describe('master/detail page frame', () => {
  it('exposes the Dashboard padding as the shared full-bleed inset', () => {
    const css = readCss('src/pages/Dashboard.css');
    const defaultRule = ruleBody(css, '.dash-main');
    const largeRule = ruleBody(css, 'html[data-density="large"] .dash-main');

    expect(defaultRule).toContain('--dash-main-padding-block: var(--kr-sp-9)');
    expect(defaultRule).toContain('--dash-main-padding-inline: var(--kr-sp-8)');
    expect(defaultRule).toContain(
      'padding: var(--dash-main-padding-block) var(--dash-main-padding-inline)',
    );
    expect(largeRule).toContain('--dash-main-padding-inline: var(--kr-sp-9)');
  });

  it.each([
    ['Automations', 'src/pages/WorkflowsPage.css', '.automation-page'],
    ['Pages', 'src/pages/PagesPage.css', '.live-pages'],
    ['Plugins', 'src/pages/McpPage.css', '.mcp-page'],
  ])('%s fills the Dashboard frame without overshooting its viewport', (_name, path, selector) => {
    const rootRule = ruleBody(readCss(path), selector);

    expect(rootRule).toContain('height: calc(100vh - 56px)');
    expect(rootRule).toContain('min-height: 0');
    for (const inset of fullBleedMargin) expect(rootRule).toContain(inset);
  });

  it('does not render the Automations root as a rounded card', () => {
    const rootRule = ruleBody(readCss('src/pages/WorkflowsPage.css'), '.automation-page');

    expect(rootRule).not.toMatch(/(^|\n)\s*border\s*:/);
    expect(rootRule).not.toMatch(/(^|\n)\s*border-radius\s*:/);
    expect(rootRule).not.toMatch(/(^|\n)\s*width\s*:/);
  });
});
