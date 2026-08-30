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
  it('contains horizontal page overflow inside the Dashboard viewport', () => {
    const appRule = ruleBody(readCss('src/pages/Dashboard.css'), '.dash-app');

    expect(appRule).toContain('overflow-x: clip');
  });

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

  it.each([
    ['Discussions', 'src/pages/DiscussionsPage.css', '.disc-chat-area'],
    ['Pages', 'src/pages/PagesPage.css', '.live-pages-viewer'],
  ])('%s uses the same inset, border, and radius as the other detail panels', (_name, path, selector) => {
    const detailRule = ruleBody(readCss(path), selector);

    expect(detailRule).toContain('min-height: 0');
    expect(detailRule).toContain('margin: var(--kr-sp-4)');
    expect(detailRule).toContain('border: 1px solid var(--kr-border)');
    expect(detailRule).toContain('border-radius: var(--kr-r-xl)');
    expect(detailRule).toContain('background: var(--kr-bg-surface)');
  });

  it('fills the project Docs tab down to the viewport without an outer scrollbar', () => {
    const css = readCss('src/pages/Dashboard.css');
    const cardRule = ruleBody(css, '.project-detail-card');
    const bodyRule = ruleBody(css, ".project-detail-card .dash-card-body[data-detail-view='docs']");
    const sectionRule = ruleBody(
      css,
      ".project-detail-card .dash-card-body[data-detail-mode='true'][data-detail-view='docs'] .project-detail-section[data-project-view='docs']",
    );
    const viewerRule = ruleBody(
      css,
      ".project-detail-card .dash-card-body[data-detail-view='docs'] .aidoc-root",
    );

    expect(cardRule).toContain('display: flex');
    expect(cardRule).toContain('height: 100%');
    expect(cardRule).toContain('overflow: hidden');
    expect(bodyRule).toContain('display: flex');
    expect(bodyRule).toContain('overflow: hidden');
    expect(sectionRule).toContain('min-height: 0');
    expect(sectionRule).toContain('flex: 1');
    expect(viewerRule).toContain('max-height: none');
    expect(viewerRule).toContain('flex: 1');
  });

  it('fills every project Code view down to the viewport without an outer scrollbar', () => {
    const dashboardCss = readCss('src/pages/Dashboard.css');
    const codeCss = readCss('src/components/ProjectCodePanel.css');
    const sourceCss = readCss('src/components/SourceCodeViewer.css');
    const bodyRule = ruleBody(
      dashboardCss,
      ".project-detail-card .dash-card-body[data-detail-view='code']",
    );
    const sectionRule = ruleBody(
      dashboardCss,
      ".project-detail-card .dash-card-body[data-detail-mode='true'][data-detail-view='code'] .project-detail-section[data-project-view='code']",
    );

    expect(bodyRule).toContain('display: flex');
    expect(bodyRule).toContain('overflow: hidden');
    expect(sectionRule).toContain('min-height: 0');
    expect(sectionRule).toContain('flex: 1');

    for (const selector of ['.project-code-diff-layout', '.project-code-commit-layout']) {
      const rule = ruleBody(codeCss, selector);
      expect(rule).toContain('height: auto');
      expect(rule).toContain('min-height: 0');
      expect(rule).toContain('flex: 1');
    }

    const sourceRule = ruleBody(sourceCss, '.source-viewer');
    expect(sourceRule).toContain('height: auto');
    expect(sourceRule).toContain('min-height: 0');
    expect(sourceRule).toContain('flex: 1');
  });
});
