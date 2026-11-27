// KT-2004 — DiffViewer: renders a diff model (lib/diffModel.ts), not raw
// text. Unified or side by side with a toggle, side by side falling back to
// unified when its own container is narrow; per file a header with its path
// (and previous path), status and +added −removed, or "binary" / "truncated"
// in place of the lines; number gutters and a +/−/space marker written as
// text; hunk headers and meta lines; a caller's highlighter skipped above a
// line limit; a wrap toggle; a CopyButton per file; a line selection made of
// named toggle buttons with aria-pressed, shift-click extending the range;
// loading and empty states. Written before the component.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { act, useState } from 'react';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { DiffViewer } from './DiffViewer';
import { diffFileFromRows, parseUnifiedDiff, type DiffFileModel, type DiffSelection } from '../../lib/diffModel';
import { diffLines } from '../../lib/qp-history-diff';
import { I18nProvider } from '../../lib/I18nContext';
import { getUILocale, t as translate } from '../../lib/i18n';
import { cssText } from '../../test/cssBlocks';
import { describeViolations, runAxe } from '../../test/axe';

const CSS = cssText(join(dirname(fileURLToPath(import.meta.url)), 'DiffViewer.css'));

const PATCH = [
  'diff --git a/src/app.ts b/src/app.ts',
  '--- a/src/app.ts',
  '+++ b/src/app.ts',
  '@@ -10,3 +10,4 @@ export function main() {',
  ' const a = 1;',
  '-const b = 2;',
  '+const b = 3;',
  '+const c = 4;',
  ' return a;',
  '\\ No newline at end of file',
  'diff --git a/docs/old.md b/docs/new.md',
  'similarity index 90%',
  'rename from docs/old.md',
  'rename to docs/new.md',
  '--- a/docs/old.md',
  '+++ b/docs/new.md',
  '@@ -1 +1 @@',
  '-# Old',
  '+# New',
  'diff --git a/logo.png b/logo.png',
  'Binary files a/logo.png and b/logo.png differ',
].join('\n');

const FILES = parseUnifiedDiff(PATCH);
const APP = FILES[0];

const file = (path: string) => screen.getByRole('group', { name: new RegExp(path.replace(/[.]/g, '\\.')) });
const rows = (scope: HTMLElement) => [...scope.querySelectorAll('.ui-diff-row')] as HTMLElement[];
const markers = (scope: HTMLElement) => [...scope.querySelectorAll('.ui-diff-marker')].map(m => m.textContent);

/** jsdom lays nothing out: report a width for the viewer, and run the
 *  ResizeObserver callbacks by hand. */
let width = 0;
let observers: (() => void)[] = [];
const descriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
beforeEach(() => {
  width = 0;
  observers = [];
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
    configurable: true,
    get(this: HTMLElement) { return this.classList.contains('ui-diff') ? width : 0; },
  });
  vi.stubGlobal('ResizeObserver', class {
    constructor(private cb: () => void) { observers.push(() => this.cb()); }
    observe() {}
    disconnect() {}
  });
});
afterEach(() => {
  if (descriptor) Object.defineProperty(HTMLElement.prototype, 'clientWidth', descriptor);
  vi.unstubAllGlobals();
});
const resize = (px: number) => act(() => { width = px; observers.forEach(cb => cb()); });

describe('DiffViewer', () => {
  describe('file header', () => {
    it('names each file by its path, with its status and its counts', () => {
      render(<DiffViewer files={FILES} />);
      const app = file('src/app.ts');
      const header = app.querySelector('.ui-diff-file-header') as HTMLElement;
      expect(within(header).getByText('src/app.ts')).toHaveClass('ui-diff-path');
      expect(within(header).getByText('ui.diff.status.modified')).toBeInTheDocument();
      expect(within(header).getByText('+2')).toHaveAttribute('data-kind', 'added');
      expect(within(header).getByText('−1')).toHaveAttribute('data-kind', 'removed');
      // The counts are said in words, once.
      expect(within(header).getByText('ui.diff.counts')).toHaveClass('ui-visually-hidden');
    });

    it('shows the previous path of a rename', () => {
      render(<DiffViewer files={FILES} />);
      const renamed = file('docs/new.md');
      expect(within(renamed).getByText('docs/old.md')).toHaveClass('ui-diff-previous-path');
      expect(within(renamed).getByText('ui.diff.status.renamed')).toBeInTheDocument();
    });

    it('says "binary" or "truncated" in place of the lines', () => {
      const truncated: DiffFileModel = { path: 'huge.json', status: 'modified', truncated: true, hunks: [] };
      render(<DiffViewer files={[...FILES, truncated]} />);
      const binary = file('logo.png');
      expect(within(binary).getByText('ui.diff.binary')).toBeInTheDocument();
      expect(rows(binary)).toHaveLength(0);
      expect(within(binary).queryByRole('button', { name: 'ui.diff.copyFile' })).toBeNull();
      expect(within(file('huge.json')).getByText('ui.diff.truncated')).toBeInTheDocument();
    });
  });

  describe('unified', () => {
    it('writes two number gutters, a text marker, the hunk header and the meta line', () => {
      const { container } = render(<DiffViewer files={[APP]} />);
      expect(container.querySelector('.ui-diff')).toHaveAttribute('data-layout', 'unified');
      const app = file('src/app.ts');
      expect(within(app).getByText('@@ -10,3 +10,4 @@ export function main() {')).toHaveClass('ui-diff-hunk-header');
      const lines = rows(app).filter(r => r.dataset.kind !== 'meta');
      expect(lines.map(r => r.dataset.kind)).toEqual(['context', 'removed', 'added', 'added', 'context']);
      expect(markers(app)).toEqual([' ', '−', '+', '+', ' ']);
      const removed = lines[1];
      expect([...removed.querySelectorAll('.ui-diff-number')].map(n => n.textContent)).toEqual(['11', '']);
      expect([...lines[2].querySelectorAll('.ui-diff-number')].map(n => n.textContent)).toEqual(['', '11']);
      expect(within(app).getByText('\\ No newline at end of file').closest('.ui-diff-row')).toHaveAttribute('data-kind', 'meta');
    });

    it('takes its colours from the diff tokens, and the three changed tokens for a changed line', () => {
      expect(CSS).toMatch(/\[data-kind="added"\][^{]*\{[^}]*var\(--kr-diff-added-bg\)/);
      expect(CSS).toMatch(/\[data-kind="removed"\][^{]*\{[^}]*var\(--kr-diff-removed-bg\)/);
      for (const part of ['bg', 'border', 'text']) expect(CSS).toContain(`var(--kr-diff-changed-${part})`);
    });

    it('writes a changed line as its old then its new text, marked − and +', () => {
      const model = diffFileFromRows(diffLines('alpha\nbeta', 'alpha\nBETA'), { path: 'prompt.md' });
      render(<DiffViewer files={[model]} />);
      const changed = rows(file('prompt.md')).filter(r => r.dataset.kind === 'changed');
      expect(changed.map(r => r.querySelector('.ui-diff-text')?.textContent)).toEqual(['beta', 'BETA']);
      expect(changed.map(r => r.querySelector('.ui-diff-marker')?.textContent)).toEqual(['−', '+']);
    });
  });

  describe('side by side', () => {
    it('pairs a removed run with the added run that follows, one number gutter per side', () => {
      const { container } = render(<DiffViewer files={[APP]} layout="split" />);
      expect(container.querySelector('.ui-diff')).toHaveAttribute('data-layout', 'split');
      const lines = rows(file('src/app.ts')).filter(r => !r.classList.contains('ui-diff-row-full'));
      const sides = lines.map(r => [...r.querySelectorAll('.ui-diff-text')].map(c => c.textContent));
      expect(sides).toEqual([
        ['const a = 1;', 'const a = 1;'],
        ['const b = 2;', 'const b = 3;'],
        ['', 'const c = 4;'],
        ['return a;', 'return a;'],
      ]);
      expect([...lines[1].querySelectorAll('.ui-diff-number')].map(n => n.textContent)).toEqual(['11', '11']);
      expect([...lines[1].querySelectorAll('.ui-diff-cell[data-side]')].map(c => `${c.getAttribute('data-side')}:${c.getAttribute('data-kind')}`))
        .toEqual(expect.arrayContaining(['old:removed', 'new:added']));
      expect([...lines[2].querySelectorAll('.ui-diff-text')][0]).toHaveAttribute('data-kind', 'empty');
    });

    it('puts the two texts of a changed line from the positional adapter side by side', () => {
      const model = diffFileFromRows(diffLines('alpha\nbeta\n', 'alpha\nBETA\ngamma'), { path: 'prompt.md' });
      render(<DiffViewer files={[model]} layout="split" />);
      const changed = rows(file('prompt.md')).find(r => r.querySelector('[data-kind="changed"]'))!;
      expect([...changed.querySelectorAll('.ui-diff-text')].map(c => [c.textContent, c.getAttribute('data-kind')]))
        .toEqual([['beta', 'changed'], ['BETA', 'changed']]);
    });

    it('switches with the layout toggle', async () => {
      const user = userEvent.setup();
      const onLayoutChange = vi.fn();
      const { container } = render(<DiffViewer files={[APP]} onLayoutChange={onLayoutChange} />);
      const split = screen.getByRole('button', { name: 'ui.diff.layout.split' });
      const unified = screen.getByRole('button', { name: 'ui.diff.layout.unified' });
      expect(unified).toHaveAttribute('aria-pressed', 'true');
      await user.click(split);
      expect(split).toHaveAttribute('aria-pressed', 'true');
      expect(container.querySelector('.ui-diff')).toHaveAttribute('data-layout', 'split');
      expect(onLayoutChange).toHaveBeenCalledWith('split');
    });

    it('falls back to unified when its container is narrow, and comes back when it widens', async () => {
      const { container } = render(<DiffViewer files={[APP]} layout="split" />);
      const root = container.querySelector('.ui-diff')!;
      await resize(1200);
      expect(root).toHaveAttribute('data-layout', 'split');
      await resize(320);
      expect(root).toHaveAttribute('data-layout', 'unified');
      expect(root).toHaveAttribute('data-narrow', 'true');
      // No side-by-side choice to offer in a narrow container.
      expect(screen.queryByRole('button', { name: 'ui.diff.layout.split' })).toBeNull();
      await resize(1200);
      expect(root).toHaveAttribute('data-layout', 'split');
    });
  });

  describe('highlight', () => {
    // Made in each test: the setup resets mock implementations between tests.
    let highlight: ReturnType<typeof vi.fn<(text: string, path: string) => string>>;
    beforeEach(() => {
      highlight = vi.fn((text: string) => `<span class="hljs-keyword">${text.replace(/</g, '&lt;')}</span>`);
    });

    it('passes each new text and the path to the highlighter, and colours its classes from the code tokens', () => {
      render(<DiffViewer files={[APP]} highlight={highlight} />);
      expect(highlight).toHaveBeenCalledWith('const b = 3;', 'src/app.ts');
      expect(highlight).not.toHaveBeenCalledWith('const b = 2;', 'src/app.ts');
      expect(file('src/app.ts').querySelectorAll('.hljs-keyword').length).toBeGreaterThan(0);
      expect(CSS).toMatch(/\.hljs-keyword[^{]*\{[^}]*var\(--kr-code-keyword\)/);
    });

    it('skips a file above the highlight limit', () => {
      render(<DiffViewer files={[APP]} highlight={highlight} highlightLimit={3} />);
      expect(highlight).not.toHaveBeenCalled();
      expect(file('src/app.ts').querySelector('.hljs-keyword')).toBeNull();
      expect(within(file('src/app.ts')).getByText('const b = 3;')).toBeInTheDocument();
    });
  });

  it('wraps by default; the toggle switches to sideways scroll', async () => {
    const user = userEvent.setup();
    const { container } = render(<DiffViewer files={[APP]} />);
    const toggle = screen.getByRole('button', { name: 'ui.diff.wrap' });
    expect(container.querySelector('.ui-diff')).toHaveAttribute('data-wrap', 'true');
    await user.click(toggle);
    expect(toggle).toHaveAttribute('aria-pressed', 'false');
    expect(container.querySelector('.ui-diff')).toHaveAttribute('data-wrap', 'false');
  });

  it('bounds its height with a preset: the files become a focusable named region', () => {
    render(<DiffViewer files={FILES} maxHeight="md" label="Changes of KT-2004" />);
    const region = screen.getByRole('region', { name: 'Changes of KT-2004' });
    expect(region).toHaveAttribute('data-max-height', 'md');
    expect(region).toHaveAttribute('tabindex', '0');
  });

  describe('copy', () => {
    const original = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    let writeText: ReturnType<typeof vi.fn>;
    beforeEach(() => {
      writeText = vi.fn().mockResolvedValue(undefined);
      Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    });
    afterEach(() => {
      if (original) Object.defineProperty(navigator, 'clipboard', original);
      else delete (navigator as { clipboard?: unknown }).clipboard;
    });

    it('copies the new text of the shown lines of one file', async () => {
      render(<DiffViewer files={FILES} />);
      await act(async () => { within(file('src/app.ts')).getByRole('button', { name: 'ui.diff.copyFile' }).click(); });
      expect(writeText).toHaveBeenCalledWith('const a = 1;\nconst b = 3;\nconst c = 4;\nreturn a;');
    });
  });

  describe('line selection', () => {
    function Selectable({ max, onChange }: { max?: number; onChange?: (s: DiffSelection | null) => void }) {
      const [selection, setSelection] = useState<DiffSelection | null>(null);
      return (
        <DiffViewer
          files={[APP]}
          selectable
          selection={selection}
          maxSelection={max}
          onSelectionChange={s => { setSelection(s); onChange?.(s); }}
          selectionToolbar={<button type="button">Comment</button>}
        />
      );
    }
    const SELECT = /^ui\.diff\.select(Removed)?Line$/;
    const toggles = () => within(file('src/app.ts')).getAllByRole('button', { name: SELECT });
    const pressed = () => toggles().map(b => b.getAttribute('aria-pressed'));

    it('gives every line but the meta one a named toggle button', () => {
      render(<Selectable />);
      expect(toggles()).toHaveLength(5);
      expect(pressed()).toEqual(['false', 'false', 'false', 'false', 'false']);
    });

    it('selects one line, extends the range with shift-click, and shows the caller\'s toolbar', async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<Selectable onChange={onChange} />);
      expect(screen.queryByRole('button', { name: 'Comment' })).toBeNull();
      await user.click(toggles()[1]);
      expect(onChange).toHaveBeenLastCalledWith({ path: 'src/app.ts', start: 1, end: 1 });
      expect(pressed()).toEqual(['false', 'true', 'false', 'false', 'false']);
      await user.keyboard('{Shift>}');
      await user.click(toggles()[3]);
      await user.keyboard('{/Shift}');
      expect(onChange).toHaveBeenLastCalledWith({ path: 'src/app.ts', start: 1, end: 3 });
      expect(pressed()).toEqual(['false', 'true', 'true', 'true', 'false']);
      expect(screen.getByRole('button', { name: 'Comment' })).toBeInTheDocument();
      // A plain click on the one selected line clears it.
      await user.click(toggles()[0]);
      await user.click(toggles()[0]);
      expect(onChange).toHaveBeenLastCalledWith(null);
      expect(pressed().every(p => p === 'false')).toBe(true);
    });

    it('never selects more than maxSelection lines', async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<Selectable max={2} onChange={onChange} />);
      await user.click(toggles()[0]);
      await user.keyboard('{Shift>}');
      await user.click(toggles()[4]);
      await user.keyboard('{/Shift}');
      expect(onChange).toHaveBeenLastCalledWith({ path: 'src/app.ts', start: 0, end: 1 });
    });

    it('names every toggle of a file uniquely, a removed line saying so, in both layouts', () => {
      // Line 11 is removed (old) and added (new): the two toggles must differ.
      expect(APP.hunks[0].lines.filter(l => l.oldNumber === 11 && l.kind === 'removed')).toHaveLength(1);
      expect(APP.hunks[0].lines.filter(l => l.newNumber === 11 && l.kind === 'added')).toHaveLength(1);
      for (const layout of ['unified', 'split'] as const) {
        const { unmount } = render(
          <I18nProvider>
            <DiffViewer files={[APP]} layout={layout} selectable selection={null} onSelectionChange={() => {}} />
          </I18nProvider>,
        );
        const names = within(file('src/app.ts')).getAllByRole('button')
          .filter(b => b.classList.contains('ui-diff-select-button'))
          .map(b => b.getAttribute('aria-label') ?? '');
        expect(names, layout).toHaveLength(5);
        expect(new Set(names).size, `${layout}: ${names.join(' | ')}`).toBe(names.length);
        const locale = getUILocale();
        expect(names.filter(n => n === translate(locale, 'ui.diff.selectRemovedLine', '11')), layout).toHaveLength(1);
        expect(names.filter(n => n === translate(locale, 'ui.diff.selectLine', '11')), layout).toHaveLength(1);
        unmount();
      }
    });

    it('shows no toggle when not selectable', () => {
      render(<DiffViewer files={[APP]} />);
      expect(screen.queryByRole('button', { name: SELECT })).toBeNull();
    });
  });

  describe('states', () => {
    it('shows a loading state, busy', () => {
      const { container } = render(<DiffViewer files={[]} loading loadingLabel="Loading the diff…" />);
      expect(screen.getByRole('status')).toHaveTextContent('Loading the diff…');
      expect(container.querySelector('.ui-diff')).toHaveAttribute('aria-busy', 'true');
    });

    it('says "no change" when there is no file, or the caller\'s text', () => {
      const { rerender } = render(<DiffViewer files={[]} />);
      expect(screen.getByText('ui.diff.empty')).toBeInTheDocument();
      rerender(<DiffViewer files={[]} empty="Nothing staged" />);
      expect(screen.getByText('Nothing staged')).toBeInTheDocument();
    });
  });

  it('writes no text under --kr-type-sm and a mono font from the tokens', () => {
    for (const [, value] of CSS.matchAll(/font-size:\s*([^;]+);/g)) {
      expect(value.trim()).toMatch(/^var\(--kr-type-(sm|md|lg|xl|2xl)\)$/);
    }
    expect(CSS).toMatch(/font-family:\s*var\(--kr-font-mono\)/);
  });

  it('passes axe in both layouts, selectable', async () => {
    for (const layout of ['unified', 'split'] as const) {
      const { container, unmount } = render(
        <main><DiffViewer files={FILES} layout={layout} selectable selection={{ path: 'src/app.ts', start: 1, end: 2 }} onSelectionChange={() => {}} /></main>,
      );
      const results = await act(() => runAxe(container));
      expect(describeViolations(results), layout).toEqual([]);
      unmount();
    }
  });
});
