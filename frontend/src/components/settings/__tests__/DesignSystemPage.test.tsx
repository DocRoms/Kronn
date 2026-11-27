// KT-1878 — Settings > Design system shows every token of the design scales
// with its name, its value in the current theme and its use. A token of a
// scale that tokens.css declares but the page does not show fails here.
// KT-1987 — the gallery is a sub-page of its own: tokens and components in
// two tabs, under one index; every test runs on the tab that shows its entry.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { readdirSync, readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { act } from 'react';
import { DesignSystemPage } from '../../../pages/DesignSystemPage';
import { describeViolations, runAxe } from '../../../test/axe';
import { GALLERY_COMPONENTS, UI_GALLERY_COMPONENTS } from '../designSystemGallery';
import { STATUS_TONES } from '../../ui/statusTones';
import { HEADER_KINDS, HEADER_LEVELS } from '../../ui/headerKinds';
import { DESIGN_TOKEN_GROUPS, DESIGN_TOKENS } from '../../../styles/designTokens';
import { loadThemes } from '../../../styles/themeAudit';
import { dictionaries } from '../../../lib/i18n/testing';
import { armBootScreen, bootScreenActive, resetBootScreenForTests } from '../../../lib/bootScreen';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const t = (key: string) => key;
const root = document.documentElement;

afterEach(() => {
  cleanup();
  root.removeAttribute('style');
  root.removeAttribute('data-theme');
});

const rowOf = (name: string) => document.querySelector(`tr[data-token="${name}"]`) as HTMLElement | null;

describe('DesignSystemPage — its own page, two tabs (KT-1987)', () => {
  it('opens on its page header (its h1) with the way back to Settings, then the index, then the tabs', async () => {
    const onBack = vi.fn();
    render(<DesignSystemPage t={t} onBack={onBack} />);
    const page = screen.getByTestId('design-system-page');
    const header = page.querySelector('.ui-header') as HTMLElement;
    expect(header).toHaveAttribute('data-kind', 'list');
    expect(within(header).getByRole('heading', { name: 'designSystem.title', level: 1 })).toBeInTheDocument();
    expect(within(header).getByText('designSystem.hint')).toBeInTheDocument();
    // Back to the Settings section that holds the link, not to the top of Settings.
    const back = within(header).getByRole('link', { name: 'designSystem.back' });
    expect(back).toHaveAttribute('href', '/config#settings-appearance');
    await userEvent.click(back);
    expect(onBack).toHaveBeenCalledTimes(1);

    const index = screen.getByRole('navigation', { name: 'designSystem.index' });
    const tablist = screen.getByRole('tablist', { name: 'designSystem.parts' });
    expect(header.compareDocumentPosition(index) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(index.compareDocumentPosition(tablist) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(tablist).getAllByRole('tab').map(tab => tab.textContent))
      .toEqual(['designSystem.tokens', 'designSystem.components']);
  });

  it('shows one tab at a time: the tokens first, the components on their tab', async () => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} />);
    const tablist = screen.getByRole('tablist', { name: 'designSystem.parts' });
    expect(within(tablist).getByRole('tab', { name: 'designSystem.tokens' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getAllByRole('tabpanel')).toHaveLength(1);
    expect(rowOf('--kr-type-sm')).toBeTruthy();
    expect(screen.queryByRole('region', { name: 'Menu' })).toBeNull();

    await user.click(within(tablist).getByRole('tab', { name: 'designSystem.components' }));
    expect(screen.getByRole('region', { name: 'Menu' })).toBeInTheDocument();
    expect(rowOf('--kr-type-sm')).toBeNull();
  });

  it('reports a tab the reader picks, and shows the tab it is given', async () => {
    const user = userEvent.setup();
    const onTabChange = vi.fn();
    const { rerender } = render(<DesignSystemPage t={t} tab="tokens" onTabChange={onTabChange} />);
    await user.click(screen.getByRole('tab', { name: 'designSystem.components' }));
    expect(onTabChange).toHaveBeenCalledWith('components');
    // The address has not moved yet: the page still shows what it is given.
    expect(screen.getByRole('tab', { name: 'designSystem.tokens' })).toHaveAttribute('aria-selected', 'true');

    rerender(<DesignSystemPage t={t} tab="components" onTabChange={onTabChange} />);
    expect(screen.getByRole('tab', { name: 'designSystem.components' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('region', { name: 'Menu' })).toBeInTheDocument();
  });

  it('opens the tab of an index entry and brings its heading into focus', async () => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} />);
    const index = screen.getByRole('navigation', { name: 'designSystem.index' });
    await user.click(within(index).getByRole('link', { name: 'Tabs' }));
    expect(screen.getByRole('tab', { name: 'designSystem.components' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('heading', { name: 'Tabs', level: 3 })).toHaveFocus();

    await user.click(within(index).getByRole('link', { name: 'designSystem.tokens' }));
    expect(screen.getByRole('tab', { name: 'designSystem.tokens' })).toHaveAttribute('aria-selected', 'true');
    expect(document.getElementById('ds-tokens')).toHaveFocus();
  });

  // A page of its own: its title is its one h1, then the parts (h2), then the
  // token groups and the gallery entries (h3). A demo inside an entry draws
  // its real heading (an h1 for ListHeader): it is not the page's outline.
  it.each(['tokens', 'components'] as const)('has one h1 and skips no level among its own headings, on the %s tab', tab => {
    render(<DesignSystemPage t={t} tab={tab} />);
    const page = screen.getByTestId('design-system-page');
    const own = [...page.querySelectorAll<HTMLElement>('h1, h2, h3, h4, h5, h6')].filter(h => !h.closest('.ds-demo'));
    const levels = own.map(h => Number(h.tagName.slice(1)));
    expect(own.filter(h => h.tagName === 'H1').map(h => h.textContent)).toEqual(['designSystem.title']);
    expect(levels[0]).toBe(1);
    const skips = own.filter((_, i) => i > 0 && levels[i] > levels[i - 1] + 1).map(h => `${h.tagName} ${h.textContent}`);
    expect(skips).toEqual([]);
    const part = tab === 'tokens' ? 'ds-tokens' : 'ds-components';
    expect(document.getElementById(part)?.tagName).toBe('H2');
    const entries = tab === 'tokens'
      ? DESIGN_TOKEN_GROUPS.map(g => `ds-${g.scale}`)
      : GALLERY_COMPONENTS.map(c => `ds-c-${c.id}`);
    for (const id of entries) expect(document.getElementById(id)?.tagName, id).toBe('H3');
  });

  it('keeps the Tokens and Components parts in the page stylesheet without a horizontal overflow at 400px', () => {
    const css = readFileSync(join(SRC, 'components', 'settings', 'DesignSystemSection.css'), 'utf8');
    expect(css.match(/\.ds-page\s*\{([^}]*)\}/)?.[1] ?? '').toMatch(/min-width:\s*0/);
    // The tab list scrolls on its own rather than widening the page.
    const tabs = readFileSync(join(SRC, 'components', 'ui', 'Tabs.css'), 'utf8');
    expect(tabs.match(/\.ui-tabs-list\s*\{([^}]*)\}/)?.[1] ?? '').toMatch(/overflow-x:\s*auto/);
  });
});

describe('DesignSystemPage — tokens', () => {
  it('shows every scale tokens.css declares, in every theme, with its name and use', () => {
    render(<DesignSystemPage t={t} tab="tokens" />);
    for (const [theme, tokens] of loadThemes(SRC)) {
      const declared = [...tokens.keys()].filter(name => DESIGN_TOKEN_GROUPS.some(g => g.match.test(name)));
      const missing = declared.filter(name => !rowOf(name));
      expect(missing, `${theme}: tokens of the scales the page does not show`).toEqual([]);
    }
    for (const token of DESIGN_TOKENS) {
      const row = rowOf(token.name)!;
      expect(row, token.name).toBeTruthy();
      expect(within(row).getByText(token.name)).toBeInTheDocument();
      expect(within(row).getByText(token.use)).toBeInTheDocument();
    }
  });

  it('groups the tokens by scale, each under its title, in order', () => {
    render(<DesignSystemPage t={t} tab="tokens" />);
    const tokensPart = screen.getByRole('region', { name: 'designSystem.tokens' });
    const titles = within(tokensPart).getAllByRole('heading', { level: 3 }).map(h => h.textContent);
    expect(titles).toEqual(DESIGN_TOKEN_GROUPS.map(g => g.title));
    for (const group of DESIGN_TOKEN_GROUPS) {
      const section = document.querySelector(`section[data-scale="${group.scale}"]`) as HTMLElement;
      const rows = [...section.querySelectorAll('tbody tr')].map(r => r.getAttribute('data-token'));
      expect(rows).toEqual(group.tokens.map(token => token.name));
    }
  });

  it('shows a text size in rem and the px it computes to at the current root size', async () => {
    root.style.setProperty('--kr-type-sm', '0.75rem');
    root.style.setProperty('--kr-type-2xl', '1.25rem');
    render(<DesignSystemPage t={t} tab="tokens" />);
    expect(screen.getByTestId('ds-value---kr-type-sm')).toHaveTextContent('0.75rem · 12px');
    expect(screen.getByTestId('ds-value---kr-type-2xl')).toHaveTextContent('1.25rem · 20px');

    // The reader's larger text size: the same token, a larger px.
    root.style.fontSize = '20px';
    await waitFor(() => expect(screen.getByTestId('ds-value---kr-type-sm')).toHaveTextContent('0.75rem · 15px'));
  });

  it('follows the theme: the value shown is the one the document computes now', async () => {
    root.style.setProperty('--kr-status-success', '#73c991');
    render(<DesignSystemPage t={t} tab="tokens" />);
    expect(screen.getByTestId('ds-value---kr-status-success')).toHaveTextContent('#73c991');

    root.style.setProperty('--kr-status-success', '#15803d');
    root.setAttribute('data-theme', 'light');
    await waitFor(() => expect(screen.getByTestId('ds-value---kr-status-success')).toHaveTextContent('#15803d'));
  });

  it('labels every scale, every use and every component caption in the four locales', () => {
    const keys = [
      'designSystem.link', 'designSystem.title', 'designSystem.hint',
      'designSystem.column.token', 'designSystem.column.value', 'designSystem.column.use', 'designSystem.column.preview',
      'designSystem.index', 'designSystem.tokens', 'designSystem.components', 'designSystem.componentsHint',
      'designSystem.back', 'designSystem.parts',
      'ui.searchField.clear', 'ui.overlay.close', 'ui.confirm.cancel',
      'ui.field.optional', 'ui.formFooter.cancel', 'ui.formFooter.blockers',
      'ui.notice.dismiss', 'ui.emptyState.clear', 'ui.loading.label',
      ...DESIGN_TOKEN_GROUPS.map(g => g.title),
      ...DESIGN_TOKENS.map(token => token.use),
      ...GALLERY_COMPONENTS.map(c => c.caption),
      ...['primary', 'neutral', 'ghost', 'danger'].map(v => `designSystem.demo.variant.${v}`),
      ...['sm', 'md', 'lg'].map(s => `designSystem.demo.size.${s}`),
      ...STATUS_TONES.map(tone => `designSystem.demo.status.${tone}`),
      ...['overview', 'activity', 'archived'].map(tab => `designSystem.demo.tab.${tab}`),
    ];
    // The demo keys the gallery reads literally, from its source.
    const gallery = readFileSync(join(SRC, 'components', 'settings', 'DesignSystemComponents.tsx'), 'utf8');
    const literal = [...gallery.matchAll(/t\('(designSystem\.demo\.[\w.]+)'/g)].map(m => m[1]);
    expect(literal.length).toBeGreaterThan(10);
    for (const [locale, dict] of Object.entries(dictionaries)) {
      const missing = [...keys, ...literal].filter(key => !(key in dict) || !dict[key].trim());
      expect(missing, `${locale} labels`).toEqual([]);
    }
  });
});

describe('DesignSystemPage — index and components (KT-1880)', () => {
  const UI_DIR = join(SRC, 'components', 'ui');

  it('shows every component of components/ui/, under its own heading, in the index order', () => {
    const inFolder = readdirSync(UI_DIR).filter(f => /^[A-Z]\w+\.tsx$/.test(f)).map(f => f.replace('.tsx', ''));
    expect(UI_GALLERY_COMPONENTS.map(c => c.name).sort()).toEqual(inFolder.sort());

    render(<DesignSystemPage t={t} tab="components" />);
    const part = screen.getByRole('region', { name: 'designSystem.components' });
    const headings = within(part).getAllByRole('heading', { level: 3 }).filter(h => !h.closest('.ds-demo'));
    expect(headings.map(h => h.textContent)).toEqual(GALLERY_COMPONENTS.map(c => c.name));
    for (const component of GALLERY_COMPONENTS) {
      const section = screen.getByRole('region', { name: component.name });
      expect(within(section).getByText(component.caption)).toBeInTheDocument();
    }
  });

  it('lists the tokens, then each component, in an index whose links reach their heading', async () => {
    render(<DesignSystemPage t={t} tab="components" />);
    const index = screen.getByRole('navigation', { name: 'designSystem.index' });
    const links = within(index).getAllByRole('link');
    expect(links.map(l => l.textContent)).toEqual(['designSystem.tokens', ...GALLERY_COMPONENTS.map(c => c.name)]);
    // Each link is the entry's own address: its tab, and its heading in the hash.
    expect(links[0].getAttribute('href')).toBe('/config/design-system#ds-tokens');
    for (const link of links.slice(1)) {
      const [path, anchor] = link.getAttribute('href')!.split('#');
      expect(path, link.textContent!).toBe('/config/design-system/components');
      const target = document.getElementById(anchor);
      expect(target, link.textContent!).toBeTruthy();
      expect(target!.tagName).toBe('H3');
    }
    await userEvent.click(within(index).getByRole('link', { name: 'Menu' }));
    expect(screen.getByRole('heading', { name: 'Menu' })).toHaveFocus();
    expect(window.location.hash).not.toBe('#ds-c-menu');
  });

  it('shows Button and IconButton in their four variants, three sizes and disabled', () => {
    render(<DesignSystemPage t={t} tab="components" />);
    for (const name of ['Button', 'IconButton']) {
      const section = screen.getByRole('region', { name });
      const buttons = within(section).getAllByRole('button');
      for (const variant of ['primary', 'neutral', 'ghost', 'danger']) {
        const ofVariant = buttons.filter(b => b.getAttribute('data-variant') === variant);
        expect(new Set(ofVariant.map(b => b.getAttribute('data-size'))), `${name} ${variant}`).toEqual(new Set(['sm', 'md', 'lg']));
        expect(ofVariant.some(b => (b as HTMLButtonElement).disabled), `${name} ${variant} disabled`).toBe(true);
      }
      for (const button of buttons) expect(button).toHaveAccessibleName();
    }
  });

  it('shows a working Menu, the five status tones, Tabs with roles, a SearchField and a Tooltip', async () => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} tab="components" />);

    const menu = screen.getByRole('region', { name: 'Menu' });
    await user.click(within(menu).getByRole('button', { name: 'designSystem.demo.menuLabel' }));
    const items = screen.getAllByRole('menuitem');
    expect(items.at(-1)).toHaveTextContent('designSystem.demo.delete');
    await user.click(items[0]);
    expect(screen.getByTestId('ds-menu-chosen')).toHaveTextContent('designSystem.demo.chosen');

    const badges = screen.getByRole('region', { name: 'StatusBadge' }).querySelectorAll('[data-tone]');
    expect([...badges].map(b => b.getAttribute('data-tone'))).toEqual([...STATUS_TONES]);

    const tabs = screen.getByRole('region', { name: 'Tabs' });
    expect(within(tabs).getAllByRole('tablist', { name: 'designSystem.demo.tabs' })).toHaveLength(2);
    expect(within(tabs).getByRole('tabpanel')).toBeInTheDocument();

    const search = screen.getByRole('region', { name: 'SearchField' });
    await user.type(within(search).getAllByRole('searchbox')[0], 'menu');
    expect(within(search).getByRole('button', { name: 'ui.searchField.clear' })).toBeInTheDocument();

    const tooltip = screen.getByRole('region', { name: 'Tooltip' });
    within(tooltip).getByRole('button', { name: 'designSystem.demo.schedule' });
    await user.hover(within(tooltip).getByRole('button'));
    expect(document.querySelector('[role="tooltip"]')).toHaveTextContent('designSystem.demo.tooltipText');
  });

  it.each([
    ['Dialog', 'dialog'],
    ['Drawer', 'dialog'],
    ['ConfirmDialog', 'alertdialog'],
  ] as const)('opens %s from a button in each of the three sizes, and Escape gives the focus back (KT-1881)', async (name, role) => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} tab="components" />);
    const section = screen.getByRole('region', { name });
    for (const size of ['sm', 'md', 'lg']) {
      const opener = section.querySelector(`button[data-size-demo="${size}"]`) as HTMLButtonElement;
      expect(opener, `${name} ${size}`).toBeTruthy();
      await user.click(opener);
      const layer = screen.getByRole(role);
      expect(layer).toHaveAttribute('data-size', size);
      expect(layer).toHaveAccessibleName();
      await user.keyboard('{Escape}');
      expect(screen.queryByRole(role)).toBeNull();
      expect(opener).toHaveFocus();
    }
  });

  it('shows Field and Select in each state, every control named by its label (KT-1882)', () => {
    render(<DesignSystemPage t={t} tab="components" />);
    const STATES = ['empty', 'filled', 'help', 'error', 'disabled', 'required'];
    for (const [name, role] of [['Field', 'textbox'], ['Select', 'combobox']] as const) {
      const section = screen.getByRole('region', { name });
      const rows = [...section.querySelectorAll('.ds-demo-label')].map(l => l.textContent);
      for (const state of STATES) expect(rows, `${name} ${state}`).toContain(`designSystem.demo.state.${state}`);
      const controls = within(section).getAllByRole(role);
      for (const control of controls) expect(control, `${name} control`).toHaveAccessibleName();
      expect(controls.some(c => c.getAttribute('aria-invalid') === 'true'), `${name} error`).toBe(true);
      expect(controls.some(c => (c as HTMLInputElement).disabled), `${name} disabled`).toBe(true);
      expect(controls.some(c => c.getAttribute('aria-required') === 'true'), `${name} required`).toBe(true);
      // The asterisk is only on the required field.
      expect(section.querySelectorAll('.ui-field-required')).toHaveLength(1);
    }
  });

  it('shows a Switch off, on, described and disabled, that turns on and off (KT-1882)', async () => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} tab="components" />);
    const section = screen.getByRole('region', { name: 'Switch' });
    const switches = within(section).getAllByRole('switch');
    expect(switches.some(s => (s as HTMLInputElement).checked)).toBe(true);
    expect(switches.some(s => !(s as HTMLInputElement).checked)).toBe(true);
    expect(switches.some(s => (s as HTMLInputElement).disabled)).toBe(true);
    expect(switches.some(s => s.getAttribute('aria-describedby'))).toBe(true);
    const first = switches[0] as HTMLInputElement;
    const before = first.checked;
    await user.click(first);
    expect(first.checked).toBe(!before);
  });

  it('holds the FormFooter action with its reasons until the required fields are filled (KT-1882)', async () => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} tab="components" />);
    const form = screen.getByTestId('ds-form-demo');
    const action = within(form).getByRole('button', { name: 'designSystem.demo.createWorkflow' });
    expect(action).toHaveAttribute('aria-disabled', 'true');
    expect(within(form).getAllByRole('listitem').map(li => li.textContent))
      .toEqual(['designSystem.demo.blockerName', 'designSystem.demo.blockerAgent']);

    await user.type(within(form).getByRole('textbox', { name: 'designSystem.demo.workflowName' }), 'Nightly');
    await user.click(action);
    expect(screen.getByTestId('ds-form-created')).toHaveTextContent('designSystem.demo.createdNone');
    await user.selectOptions(within(form).getByRole('combobox', { name: 'designSystem.demo.agent' }), 'codex');
    expect(within(form).queryAllByRole('listitem')).toHaveLength(0);
    expect(action).not.toHaveAttribute('aria-disabled');
    await user.click(action);
    expect(screen.getByTestId('ds-form-created')).toHaveTextContent('designSystem.demo.created');

    const footer = screen.getByRole('region', { name: 'FormFooter' });
    expect(within(footer).getAllByRole('button').some(b => b.getAttribute('aria-busy') === 'true')).toBe(true);
    expect(within(footer).getAllByRole('button').some(b => b.getAttribute('data-variant') === 'danger')).toBe(true);
  });

  it('passes axe on the form components of the gallery (KT-1882)', async () => {
    render(<main><DesignSystemPage t={t} tab="components" /></main>);
    for (const name of ['Field', 'Select', 'Switch', 'FormFooter']) {
      const results = await act(() => runAxe(screen.getByRole('region', { name })));
      expect(describeViolations(results), name).toEqual([]);
    }
  });

  it('shows Notice in its four tones, with its action and a cross that dismisses it (KT-1883)', async () => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} tab="components" />);
    const section = screen.getByRole('region', { name: 'Notice' });
    const notices = [...section.querySelectorAll('.ui-notice')];
    expect(notices.map(n => n.getAttribute('data-tone'))).toEqual(['info', 'warning', 'danger', 'success']);
    await user.click(within(section).getByRole('button', { name: 'designSystem.demo.retry' }));
    expect(screen.getByTestId('ds-notice-retried')).toHaveAttribute('data-count', '1');
    await user.click(within(section).getByRole('button', { name: 'ui.notice.dismiss' }));
    expect(section.querySelector('.ui-notice[data-tone="success"]')).toBeNull();
    await user.click(within(section).getByRole('button', { name: 'designSystem.demo.showNoticeAgain' }));
    expect(section.querySelector('.ui-notice[data-tone="success"]')).toBeInTheDocument();
  });

  it('shows EmptyState in its three cases, and LoadingState in three sizes with both skeletons (KT-1883)', () => {
    render(<DesignSystemPage t={t} tab="components" />);
    const empty = screen.getByRole('region', { name: 'EmptyState' });
    expect([...empty.querySelectorAll('.ui-empty-state')].map(e => e.getAttribute('data-kind')))
      .toEqual(['firstUse', 'noResult', 'nothingSelected']);
    within(empty).getByRole('button', { name: 'designSystem.demo.emptyFirstUseAction' });
    within(empty).getByRole('button', { name: 'ui.emptyState.clear' });

    const loading = screen.getByRole('region', { name: 'LoadingState' });
    expect([...loading.querySelectorAll('.ui-loading-state')].map(l => l.getAttribute('data-size'))).toEqual(['sm', 'md', 'lg']);
    expect([...loading.querySelectorAll('.ui-skeleton')].map(s => s.getAttribute('data-kind'))).toEqual(['list', 'detail']);
  });

  it('lets the cold-start screen finish when Settings is the first page opened (KT-1883)', async () => {
    // What index.html paints before any script runs.
    resetBootScreenForTests();
    const boot = document.createElement('div');
    boot.id = 'kronn-boot';
    boot.innerHTML = '<span data-boot-text></span><button data-boot-retry hidden></button>';
    document.body.prepend(boot);
    try {
      render(<DesignSystemPage t={t} tab="components" />);
      armBootScreen();
      // The md and lg demos are content: drawn at once, holding nothing.
      const loading = screen.getByRole('region', { name: 'LoadingState' });
      expect([...loading.querySelectorAll('.ui-loading-state')].map(l => l.getAttribute('data-size'))).toEqual(['sm', 'md', 'lg']);
      await waitFor(() => expect(bootScreenActive()).toBe(false));
    } finally {
      document.getElementById('kronn-boot')?.remove();
      resetBootScreenForTests();
    }
  });

  it('passes axe on the feedback components of the gallery (KT-1883)', async () => {
    render(<main><DesignSystemPage t={t} tab="components" /></main>);
    for (const name of ['Notice', 'EmptyState', 'LoadingState']) {
      const results = await act(() => runAxe(screen.getByRole('region', { name })));
      expect(describeViolations(results), name).toEqual([]);
    }
  });

  const HEADER_ENTRIES = [
    ['ListHeader', 'list'], ['DetailHeader', 'detail'], ['TabHeader', 'tab'],
    ['SectionHeader', 'section'], ['DrawerHeader', 'drawer'], ['CardHeader', 'card'],
  ] as const;

  it('shows each header kind in its normal and compact form, each with one heading at its level (KT-1884)', () => {
    render(<DesignSystemPage t={t} tab="components" />);
    expect(HEADER_KINDS).toEqual(HEADER_ENTRIES.map(([, kind]) => kind));
    for (const [name, kind] of HEADER_ENTRIES) {
      const section = screen.getByRole('region', { name });
      const headers = [...section.querySelectorAll<HTMLElement>('.ui-header')];
      expect(headers.map(h => h.getAttribute('data-kind')), name).toEqual([kind, kind]);
      expect(headers.map(h => h.hasAttribute('data-compact')), name).toEqual([false, true]);
      for (const header of headers) {
        const headings = within(header).getAllByRole('heading');
        expect(headings, name).toHaveLength(1);
        expect(headings[0].tagName, name).toBe(`H${HEADER_LEVELS[kind]}`);
        expect(header.querySelector('.ui-header-actions'), `${name} actions`).toBeTruthy();
      }
      const rows = [...section.querySelectorAll('.ds-demo-label')].map(l => l.textContent);
      expect(rows, name).toEqual(['designSystem.demo.header.normal', 'designSystem.demo.header.compact']);
    }
  });

  it('names the drawer and the card from their header, and closes the drawer from its last control (KT-1884)', async () => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} tab="components" />);
    const drawer = screen.getByRole('region', { name: 'DrawerHeader' });
    for (const group of within(drawer).getAllByRole('group')) {
      expect(group).toHaveAccessibleName();
      const buttons = within(group).getAllByRole('button');
      expect(buttons.at(-1)).toHaveAccessibleName('ui.overlay.close');
    }
    await user.click(within(drawer).getAllByRole('button', { name: 'ui.overlay.close' })[0]);
    expect(screen.getByTestId('ds-drawer-header-closed')).toHaveAttribute('data-count', '1');

    const cards = within(screen.getByRole('region', { name: 'CardHeader' })).getAllByRole('article');
    expect(cards.map(card => card.querySelector('.ui-header-status [data-tone]')?.getAttribute('data-tone')))
      .toEqual(['success', 'success']);
    for (const card of cards) expect(card).toHaveAccessibleName();
  });

  it('passes axe on the header kinds of the gallery (KT-1884)', async () => {
    render(<main><DesignSystemPage t={t} tab="components" /></main>);
    for (const [name] of HEADER_ENTRIES) {
      const results = await act(() => runAxe(screen.getByRole('region', { name })));
      // A demo renders its real heading (an h1 for ListHeader): the gallery's
      // own h3 entry title that follows an h1 or h2 demo is the one jump
      // allowed. A jump inside a demo, or on any other node, still fails.
      const own = results.violations.filter(v => !(
        v.id === 'heading-order' && v.nodes.every(n => String(n.target[0]).startsWith('#ds-c-'))
      ));
      expect(describeViolations({ ...results, violations: own }), name).toEqual([]);
    }
  });

  it('lets a header demo shrink to a 400px window: the title ellipsizes inside its frame (KT-1884)', () => {
    const css = readFileSync(join(SRC, 'components', 'settings', 'DesignSystemSection.css'), 'utf8');
    const frame = css.match(/\.ds-demo-header-frame\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(frame).toMatch(/min-width:\s*0/);
    expect(frame).toMatch(/overflow:\s*hidden/);
    const shared = readFileSync(join(SRC, 'components', 'ui', 'headerFrame.css'), 'utf8');
    const title = shared.match(/\.ui-header-title\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(title).toMatch(/text-overflow:\s*ellipsis/);
    expect(title).toMatch(/white-space:\s*nowrap/);
    const identity = shared.match(/\.ui-header-identity\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(identity).toMatch(/min-width:\s*0/);
    const actions = shared.match(/\.ui-header-actions\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(actions).toMatch(/flex-shrink:\s*0/);
  });

  it('turns each token row into a block under md, keeping its preview and its name on one line', () => {
    const css = readFileSync(join(SRC, 'components', 'settings', 'DesignSystemSection.css'), 'utf8');
    const narrow = css.slice(css.indexOf('@media (max-width: 767px)'));
    expect(narrow).not.toMatch(/\.ds-preview-cell[^{]*\{[^}]*display:\s*none/);
    expect(narrow).toMatch(/\.ds-table tbody tr\s*\{[^}]*display:\s*grid/);
    expect(narrow).toMatch(/\.ds-table tbody th\s*\{[^}]*white-space:\s*nowrap/);
    expect(narrow).not.toMatch(/overflow-wrap:\s*anywhere/);
  });
});
