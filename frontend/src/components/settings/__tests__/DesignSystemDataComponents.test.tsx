// KT-1886 — the gallery entries of the dense-data components: each one shows
// its normal form in a wide container on its own line, then its narrow forms
// together on the next line, each in a narrow container (not a narrow
// window), with sample data written in the interface's locale; the demos work
// and pass axe.
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { act } from 'react';
import { DesignSystemPage } from '../../../pages/DesignSystemPage';
import { describeViolations, runAxe } from '../../../test/axe';
import { dictionaries } from '../../../lib/i18n/testing';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const t = (key: string) => key;
const ENTRIES = ['CopyButton', 'Table', 'KeyValueList', 'StatTile', 'ProgressBar'] as const;

afterEach(cleanup);

const section = (name: string) => screen.getByRole('region', { name });
/** A number as the demos write it (the test context's locale is fr). */
const fr = (n: number) => new Intl.NumberFormat('fr').format(n);

describe('DesignSystemPage — dense-data components (KT-1886)', () => {
  it('shows each one in a wide container on its own line, then its narrow containers together on the next line', () => {
    render(<DesignSystemPage t={t} tab="components" />);
    for (const name of ENTRIES) {
      const pairs = [...section(name).querySelectorAll('.ds-demo-pair')];
      expect(pairs.length, name).toBeGreaterThan(0);
      for (const pair of pairs) {
        const lines = [...pair.children];
        expect(lines.map(l => l.className), name).toEqual(['ds-demo-container', 'ds-demo-narrow-line']);
        expect(lines[0]).toHaveAttribute('data-form', 'wide');
        expect(within(lines[0] as HTMLElement).getByText('designSystem.demo.data.wide')).toBeInTheDocument();
        const narrow = [...lines[1].children];
        expect(narrow.length, name).toBeGreaterThan(0);
        for (const box of narrow) {
          expect(box).toHaveClass('ds-demo-container');
          expect(box).toHaveAttribute('data-form', 'narrow');
          expect(box.querySelector('.ds-demo-label')?.textContent).toMatch(/^designSystem\.demo\.data\.narrow/);
        }
      }
    }
  });

  it('puts the wide container on a line of its own and lets the 20rem narrow ones wrap under each other', () => {
    const css = readFileSync(join(SRC, 'components', 'settings', 'DesignSystemSection.css'), 'utf8');
    expect(css).toMatch(/\.ds-demo-pair\s*\{[^}]*flex-direction:\s*column/);
    expect(css).toMatch(/\.ds-demo-container\[data-form="wide"\]\s*\{[^}]*align-self:\s*stretch/);
    expect(css).toMatch(/\.ds-demo-narrow-line\s*\{[^}]*display:\s*flex[^}]*flex-wrap:\s*wrap/);
    expect(css).toMatch(/\.ds-demo-container\[data-form="narrow"\]\s*\{[^}]*max-width:\s*20rem/);
    // No padding on the container: it would take the wide Table under 40rem.
    expect(css.match(/\.ds-demo-container\s*\{([^}]*)\}/)?.[1] ?? '').not.toMatch(/padding/);
  });

  it('gives every size container of the gallery a wrapper with a width of its own, so none collapses to 0 px', () => {
    // A size container (container-type) has a zero intrinsic width: as a flex
    // item with no width or flex-basis it collapses and its demo vanishes.
    const sizes = /(?:^|;)\s*(?:width|flex-basis):|(?:^|;)\s*flex:\s*\S+\s+\S+\s+\S+/;
    const rules = (css: string) => [...css.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{([^{}]*)\}/g)]
      .map(([, selector, body]) => ({ selector: selector.trim(), body }));
    const gallery = rules(readFileSync(join(SRC, 'components', 'settings', 'DesignSystemSection.css'), 'utf8'));
    // In the gallery's stylesheet, a rule that makes a size container sizes it.
    for (const rule of gallery.filter(r => /container-type:\s*(?:inline-)?size/.test(r.body))) {
      expect(rule.body, rule.selector).toMatch(sizes);
    }
    // The size containers the new components make, read from their stylesheets.
    const containers = ['Table', 'KeyValueList'].flatMap(file => rules(readFileSync(join(SRC, 'components', 'ui', `${file}.css`), 'utf8'))
      .filter(r => /container-type:\s*(?:inline-)?size/.test(r.body)).map(r => r.selector));
    expect(containers).toEqual(['.ui-table', '.ui-kv-frame']);
    render(<DesignSystemPage t={t} tab="components" />);
    const forms = new Set<string>();
    for (const name of ENTRIES) {
      for (const block of section(name).querySelectorAll(containers.join(', '))) {
        // Its direct wrapper is a demo container of a declared form...
        const wrapper = block.parentElement!;
        expect(wrapper, `${name} ${block.className}`).toHaveClass('ds-demo-container');
        expect(wrapper.getAttribute('data-form'), name).toMatch(/^(?:wide|narrow)$/);
        forms.add(wrapper.getAttribute('data-form')!);
      }
    }
    expect([...forms].sort()).toEqual(['narrow', 'wide']);
    // ...and the rule of each form sets a width or a flex-basis.
    for (const form of forms) {
      const rule = gallery.find(r => r.selector === `.ds-demo-container[data-form="${form}"]`);
      expect(rule?.body ?? '', form).toMatch(sizes);
    }
  });

  it('writes the sample numbers in the interface\'s locale, never with English separators', () => {
    render(<DesignSystemPage t={t} tab="components" />);
    const all = ENTRIES.map(name => section(name).textContent ?? '').join(' ');
    for (const english of ['96,400', '48,210', '18,240', '4,310', '96.4']) expect(all, english).not.toContain(english);
    // The raw text: a text query would fold the French narrow no-break space.
    const text = (name: string) => section(name).textContent ?? '';
    expect(text('Table')).toContain(fr(96_400));
    expect(text('KeyValueList')).toContain(fr(48_210));
    expect(text('StatTile')).toContain(new Intl.NumberFormat('fr').format(96.4));
    expect(text('ProgressBar')).toContain(new Intl.NumberFormat('fr', { style: 'percent', maximumFractionDigits: 0 }).format(0.7));
  });

  it('labels every caption and demo string in the four locales', () => {
    const keys = [
      'ui.copy.copied', 'ui.copy.failed', 'ui.table.selectAll', 'ui.table.selectRow', 'ui.table.actions',
      'ui.table.empty', 'ui.keyValue.missing', 'ui.keyValue.copy', 'ui.stat.unknown', 'ui.stat.up', 'ui.stat.down',
      'ui.stat.flat', 'designSystem.demo.data.statusFailed', 'designSystem.demo.data.statusDone',
      'designSystem.demo.data.statusRunning', 'designSystem.demo.data.statusCancelled',
      'designSystem.demo.data.wide', 'designSystem.demo.data.narrow', 'designSystem.demo.data.formCards',
      'designSystem.demo.data.formScroll', 'designSystem.demo.data.bounded', 'designSystem.demo.data.boundedCaption',
      ...['copyButton', 'table', 'keyValueList', 'statTile', 'progressBar'].map(id => `designSystem.caption.${id}`),
    ];
    for (const [locale, dict] of Object.entries(dictionaries)) {
      expect(keys.filter(key => !(key in dict) || !dict[key].trim()), locale).toEqual([]);
    }
  });

  it('shows the CopyButton in its two forms and its failure', async () => {
    render(<DesignSystemPage t={t} tab="components" />);
    const copy = section('CopyButton');
    expect(within(copy).getAllByRole('button', { name: 'designSystem.demo.data.copyId' }).map(b => b.getAttribute('data-size')))
      .toEqual(['sm', 'md', 'lg']);
    expect(within(copy).getAllByRole('button', { name: 'designSystem.demo.data.copyCommand' }).some(b => b.classList.contains('ui-button'))).toBe(true);
    const broken = within(copy).getAllByRole('button', { name: 'designSystem.demo.data.copyBroken' })[0];
    await act(async () => { broken.click(); });
    expect(broken).toHaveAttribute('data-state', 'failed');
    expect(within(broken.closest('.ui-copy-button-wrap') as HTMLElement).getByRole('status')).toHaveTextContent('ui.copy.failed');
  });

  it('shows a Table that sorts, selects and opens a row, its cards and scroll forms side by side, a bounded one, empty and loading', async () => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} tab="components" />);
    const table = section('Table');
    const tables = [...table.querySelectorAll('.ui-table')];
    expect(tables.map(tb => tb.getAttribute('data-narrow'))).toEqual(['cards', 'cards', 'scroll', 'scroll', 'scroll', 'scroll']);
    // The two narrow forms sit together on the line under the wide table.
    const narrowLine = table.querySelector('.ds-demo-narrow-line')!;
    expect([...narrowLine.querySelectorAll('.ui-table')].map(tb => tb.getAttribute('data-narrow'))).toEqual(['cards', 'scroll']);
    expect([...narrowLine.querySelectorAll('.ds-demo-label')].map(l => l.textContent)).toEqual([
      'designSystem.demo.data.narrow · designSystem.demo.data.formCards',
      'designSystem.demo.data.narrow · designSystem.demo.data.formScroll',
    ]);
    // The wide one is a bounded region named by its caption.
    expect(within(table).getByRole('region', { name: 'designSystem.demo.data.runsCaption' })).toHaveAttribute('data-max-height', 'md');
    // A bounded table, preset sm, with a sticky header and enough rows to
    // scroll: a focusable region named by its caption.
    const bounded = within(table).getByRole('region', { name: 'designSystem.demo.data.boundedCaption' });
    expect(bounded).toHaveAttribute('data-max-height', 'sm');
    expect(bounded).toHaveAttribute('tabindex', '0');
    expect(bounded.closest('.ui-table')).toHaveAttribute('data-sticky-header', 'true');
    expect(within(bounded).getAllByRole('row').length - 1).toBeGreaterThanOrEqual(10);

    const [wide] = within(table).getAllByRole('table');
    const tokens = within(wide).getByRole('columnheader', { name: 'designSystem.demo.data.colTokens' });
    expect(tokens).toHaveAttribute('aria-sort', 'descending');
    await user.click(within(tokens).getByRole('button'));
    expect(tokens).toHaveAttribute('aria-sort', 'ascending');

    const openButton = within(wide).getByRole('button', { name: 'designSystem.demo.data.run3' });
    openButton.focus();
    await user.keyboard('{Enter}');
    expect(screen.getByTestId('ds-table-opened')).toHaveTextContent('designSystem.demo.data.opened');

    const all = within(wide).getByRole('checkbox', { name: 'ui.table.selectAll' });
    expect(all).toBePartiallyChecked();
    await user.click(all);
    expect(all).toBeChecked();

    expect(within(table).getAllByText('designSystem.demo.data.emptyTitle')).toHaveLength(1);
    expect(within(table).getByRole('status')).toHaveTextContent('designSystem.demo.data.loadingRuns');
  });

  it('shows KeyValueList in its two layouts, with a missing and a copyable value', () => {
    render(<DesignSystemPage t={t} tab="components" />);
    const kv = section('KeyValueList');
    const lists = [...kv.querySelectorAll('dl')];
    expect(lists.map(dl => dl.getAttribute('data-layout'))).toEqual(['grid', 'grid', 'inline', 'inline']);
    expect(kv.querySelectorAll('dd[data-missing="true"]')).toHaveLength(4);
    expect(within(kv).getAllByRole('button', { name: 'ui.keyValue.copy' })).toHaveLength(4);
  });

  it('shows StatTiles known, unknown, loading, with a delta, and one that toggles', async () => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} tab="components" />);
    const stats = section('StatTile');
    expect(within(stats).getAllByRole('group', { name: 'designSystem.demo.data.statWeek' })).toHaveLength(2);
    expect(stats.querySelector('.ui-stat-value[data-unknown="true"]')).toBeTruthy();
    expect(stats.querySelector('.ui-stat-tile[aria-busy="true"]')).toBeTruthy();
    expect(stats.querySelector('.ui-stat-tile[data-size="lg"]')).toBeTruthy();
    // Two filter tiles in each grid, one of them pressed from the start so the
    // selected state shows in the gallery.
    const pressed = () => within(stats).getAllByRole('button').map(b => b.getAttribute('aria-pressed'));
    expect(pressed()).toEqual(['false', 'true', 'false', 'true']);
    expect(stats.querySelector('.ui-stat-tile[aria-pressed="true"]')).toHaveTextContent('designSystem.demo.data.statusRunning');
    await user.click(within(stats).getAllByRole('button')[0]);
    expect(pressed()).toEqual(['true', 'false', 'true', 'false']);
  });

  it('shows ProgressBar determinate, indeterminate, stacked with its legend, and its five tones', () => {
    render(<DesignSystemPage t={t} tab="components" />);
    const progress = section('ProgressBar');
    const bars = within(progress).getAllByRole('progressbar');
    expect(bars.some(b => b.hasAttribute('aria-valuenow'))).toBe(true);
    expect(bars.some(b => !b.hasAttribute('aria-valuenow'))).toBe(true);
    expect(within(progress).getAllByRole('img').length).toBeGreaterThanOrEqual(2);
    expect(progress.querySelectorAll('.ui-progress-legend')).toHaveLength(2);
    expect(new Set([...progress.querySelectorAll('.ui-progress[data-tone]')].map(p => p.getAttribute('data-tone'))))
      .toEqual(new Set(['accent', 'success', 'warning', 'danger', 'info']));
  });

  it('passes axe on the five entries', async () => {
    render(<main><DesignSystemPage t={t} tab="components" /></main>);
    for (const name of ENTRIES) {
      const results = await act(() => runAxe(section(name)));
      expect(describeViolations(results), name).toEqual([]);
    }
  });
});
