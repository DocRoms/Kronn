// KT-2004 — the gallery entries of LogViewer and DiffViewer: each shows its
// normal form in a wide container on its own line, then its narrow form in a
// narrow container on the next line, with sample data (a log that looks
// streaming with stderr lines; a two-file diff with a rename, a binary file
// and a changed line), their captions in the four locales; the demos work and
// pass axe.
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { act } from 'react';
import { DesignSystemPage } from '../../../pages/DesignSystemPage';
import { describeViolations, runAxe } from '../../../test/axe';
import { dictionaries } from '../../../lib/i18n/testing';

const t = (key: string) => key;
const ENTRIES = ['LogViewer', 'DiffViewer'] as const;

afterEach(cleanup);

const section = (name: string) => screen.getByRole('region', { name });

describe('DesignSystemPage — log and diff viewers (KT-2004)', () => {
  it('shows each one in a wide container on its own line, then its narrow container on the next line', () => {
    render(<DesignSystemPage t={t} tab="components" />);
    for (const name of ENTRIES) {
      const [pair] = [...section(name).querySelectorAll('.ds-demo-pair')];
      expect(pair, name).toBeTruthy();
      const lines = [...pair.children];
      expect(lines.map(l => l.className), name).toEqual(['ds-demo-container', 'ds-demo-narrow-line']);
      expect(lines[0]).toHaveAttribute('data-form', 'wide');
      const narrow = [...lines[1].children];
      expect(narrow).toHaveLength(1);
      expect(narrow[0]).toHaveAttribute('data-form', 'narrow');
      const viewer = name === 'LogViewer' ? '.ui-log' : '.ui-diff';
      expect(lines[0].querySelector(viewer), `${name} wide`).toBeTruthy();
      expect(narrow[0].querySelector(viewer), `${name} narrow`).toBeTruthy();
    }
  });

  it('labels both captions and every new string in the four locales', () => {
    const keys = [
      'designSystem.caption.logViewer', 'designSystem.caption.diffViewer',
      'designSystem.demo.log.label', 'designSystem.demo.log.note', 'designSystem.demo.log.sideways',
      'designSystem.demo.log.emptyText', 'designSystem.demo.diff.label', 'designSystem.demo.diff.selected',
      'designSystem.demo.diff.selectedNone', 'designSystem.demo.diff.comment', 'designSystem.demo.diff.loadingDiff',
      'ui.log.empty', 'ui.log.stderr', 'ui.log.level.error', 'ui.log.level.warn', 'ui.log.wrap', 'ui.log.jumpToEnd',
      'ui.log.copy', 'ui.log.refresh', 'ui.log.clear',
      'ui.diff.label', 'ui.diff.empty', 'ui.diff.binary', 'ui.diff.truncated', 'ui.diff.counts', 'ui.diff.previousPath',
      'ui.diff.status.added', 'ui.diff.status.deleted', 'ui.diff.status.modified', 'ui.diff.status.renamed',
      'ui.diff.status.copied', 'ui.diff.copyFile', 'ui.diff.layout', 'ui.diff.layout.unified', 'ui.diff.layout.split',
      'ui.diff.wrap', 'ui.diff.selectLine', 'ui.diff.selectRemovedLine',
      'designSystem.use.diff-changed-bg', 'designSystem.use.diff-changed-border', 'designSystem.use.diff-changed-text',
    ];
    for (const [locale, dict] of Object.entries(dictionaries)) {
      expect(keys.filter(key => !(key in dict) || !dict[key].trim()), locale).toEqual([]);
    }
  });

  it('shows a streaming-looking log with stderr lines, a note, the whole toolbar, a sideways one and an empty one', async () => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} tab="components" />);
    const entry = section('LogViewer');
    const logs = within(entry).getAllByRole('log', { name: 'designSystem.demo.log.label' });
    expect(logs).toHaveLength(4);
    const [wide] = logs;
    expect(wide.querySelectorAll('.ui-log-line[data-stream="stderr"]').length).toBeGreaterThanOrEqual(2);
    expect(wide.querySelector('.ui-log-cursor')).toBeTruthy();
    // A tail: numbered where it starts in the longer output, ending at its total.
    const numbers = [...wide.querySelectorAll('.ui-log-number')].map(n => n.textContent);
    expect(numbers[0]).not.toBe('1');
    expect(numbers.at(-1)).toBe('1248');
    expect(wide.querySelectorAll('time.ui-log-time').length).toBeGreaterThan(0);
    expect(wide.textContent).not.toContain('\u001b');
    expect(within(entry).getAllByText('designSystem.demo.log.note')).toHaveLength(2);
    expect(entry.querySelector('.ui-log[data-wrap="false"]')).toBeTruthy();
    expect(within(entry).getByText('designSystem.demo.log.emptyText')).toBeInTheDocument();
    // Clear empties the log; refresh brings the lines back.
    await user.click(within(entry).getAllByRole('button', { name: 'ui.log.clear' })[0]);
    expect(within(wide).getByText('ui.log.empty')).toBeInTheDocument();
    await user.click(within(entry).getAllByRole('button', { name: 'ui.log.refresh' })[0]);
    expect(wide.querySelectorAll('.ui-log-line').length).toBeGreaterThan(5);
  });

  it('shows a two-file diff with a rename, a binary file and a changed line, a selection, loading and empty', async () => {
    const user = userEvent.setup();
    render(<DesignSystemPage t={t} tab="components" />);
    const entry = section('DiffViewer');
    const [wide] = [...entry.querySelectorAll('.ui-diff')] as HTMLElement[];
    expect(wide).toHaveAttribute('data-layout', 'split');
    const files = within(wide).getAllByRole('group').filter(g => g.classList.contains('ui-diff-file'));
    expect(files).toHaveLength(2);
    expect(within(files[0]).getByText('src/lib/execution.ts')).toHaveClass('ui-diff-previous-path');
    expect(within(files[0]).getByText('ui.diff.status.renamed')).toBeInTheDocument();
    expect(files[0].querySelector('.ui-diff-text[data-kind="changed"]')).toBeTruthy();
    expect(files[0].querySelectorAll('.hljs-string, .hljs-keyword').length).toBeGreaterThan(0);
    expect(within(files[1]).getByText('ui.diff.binary')).toBeInTheDocument();
    // A selection from the start, with the caller's toolbar under it.
    const toggles = within(files[0]).getAllByRole('button', { name: /^ui\.diff\.select(Removed)?Line$/ });
    expect(toggles.filter(b => b.getAttribute('aria-pressed') === 'true').length).toBe(3);
    expect(within(files[0]).getByRole('button', { name: 'designSystem.demo.diff.comment' })).toBeInTheDocument();
    expect(screen.getByTestId('ds-diff-selection')).toHaveTextContent('designSystem.demo.diff.selected');
    await user.click(toggles[0]);
    expect(toggles[0]).toHaveAttribute('aria-pressed', 'true');
    // The narrow one, bounded, is a named region.
    expect(within(entry).getByRole('region', { name: 'designSystem.demo.diff.label' })).toHaveAttribute('data-max-height', 'sm');
    // Each CopyButton has a polite status of its own: the loading one is found by its text.
    const loading = within(entry).getAllByRole('status').filter(s => s.textContent?.includes('designSystem.demo.diff.loadingDiff'));
    expect(loading).toHaveLength(1);
    expect(within(entry).getByText('ui.diff.empty')).toBeInTheDocument();
  });

  it('passes axe on the two entries', async () => {
    render(<main><DesignSystemPage t={t} tab="components" /></main>);
    for (const name of ENTRIES) {
      const results = await act(() => runAxe(section(name)));
      expect(describeViolations(results), name).toEqual([]);
    }
  });
});
