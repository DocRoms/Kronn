// KT-1886 — Table: one data table. A caption always names it; columns declare
// their alignment, numbers, sorting, truncation and row header; sorting is
// controlled; the scroll area is a named region; selection, an actions
// column, a row that opens from the keyboard, empty and loading rows, a footer
// slot and two narrow forms decided by the container's width. Written before
// the component.
import { describe, expect, it, vi } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { act, useState } from 'react';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Table, type TableColumn } from './Table';
import { EmptyState } from './EmptyState';
import { IconButton } from './IconButton';
import { nextSort, type TableSort } from './dataKinds';
import { cssText } from '../../test/cssBlocks';
import { describeViolations, runAxe } from '../../test/axe';

const CSS = cssText(join(dirname(fileURLToPath(import.meta.url)), 'Table.css'));

interface Run { id: string; name: string; status: string; tokens: number; path: string }
const RUNS: Run[] = [
  { id: 'r1', name: 'Nightly digest', status: 'Failed', tokens: 1200, path: '/very/long/path/to/a/worktree/that/never/ends' },
  { id: 'r2', name: 'Weekly audit', status: 'Done', tokens: 98000, path: '/short' },
  { id: 'r3', name: 'Release notes', status: 'Running', tokens: 450, path: '/srv' },
];

const COLUMNS: TableColumn<Run>[] = [
  { id: 'name', header: 'Workflow', cell: r => r.name, rowHeader: true, sortable: true },
  { id: 'status', header: 'Status', cell: r => r.status },
  { id: 'tokens', header: 'Tokens', cell: r => r.tokens.toLocaleString('en'), numeric: true, sortable: true },
  { id: 'path', header: 'Worktree', cell: r => r.path, truncate: 'ellipsis' },
];

const base = { caption: 'Workflow runs', columns: COLUMNS, rows: RUNS, getRowId: (r: Run) => r.id };

const headerCell = (name: string) => screen.getAllByRole('columnheader').find(th => th.textContent?.includes(name))!;

describe('Table', () => {
  it('is a table named by its caption, visible or visually hidden', () => {
    const { rerender } = render(<Table {...base} />);
    const table = screen.getByRole('table', { name: 'Workflow runs' });
    const caption = table.querySelector('caption')!;
    expect(caption).not.toHaveClass('ui-visually-hidden');
    rerender(<Table {...base} hideCaption />);
    expect(screen.getByRole('table', { name: 'Workflow runs' }).querySelector('caption')).toHaveClass('ui-visually-hidden');
  });

  it('gives every header cell a column scope and makes the rowHeader cell a th scope="row"', () => {
    render(<Table {...base} />);
    for (const th of screen.getAllByRole('columnheader')) expect(th).toHaveAttribute('scope', 'col');
    const rowHeaders = screen.getAllByRole('rowheader');
    expect(rowHeaders.map(th => th.textContent)).toEqual(RUNS.map(r => r.name));
    for (const th of rowHeaders) expect(th).toHaveAttribute('scope', 'row');
  });

  it('end-aligns a numeric column with tabular figures, and takes an explicit alignment', () => {
    render(<Table {...base} columns={[...COLUMNS, { id: 'c', header: 'Centre', cell: () => 'x', align: 'center' }]} />);
    const tokens = screen.getByText('98,000').closest('td')!;
    expect(tokens).toHaveAttribute('data-align', 'end');
    expect(tokens).toHaveAttribute('data-numeric', 'true');
    expect(headerCell('Tokens')).toHaveAttribute('data-align', 'end');
    expect(CSS).toMatch(/\[data-numeric="true"\][^{]*\{[^}]*font-variant-numeric:\s*tabular-nums/);
    expect(screen.getAllByText('x')[0].closest('td')).toHaveAttribute('data-align', 'center');
  });

  it('truncates a column with an ellipsis and the full value in a title, or wraps it', () => {
    const { rerender } = render(<Table {...base} />);
    const cut = screen.getByText(RUNS[0].path);
    expect(cut).toHaveClass('ui-table-truncate');
    expect(cut).toHaveAttribute('title', RUNS[0].path);
    expect(cut.closest('td')).toHaveAttribute('data-truncate', 'ellipsis');
    expect(CSS).toMatch(/\.ui-table-truncate\s*\{[^}]*text-overflow:\s*ellipsis/);
    rerender(<Table {...base} columns={COLUMNS.map(c => c.id === 'path' ? { ...c, truncate: 'wrap' as const } : c)} />);
    expect(screen.getByText(RUNS[0].path).closest('td')).toHaveAttribute('data-truncate', 'wrap');
    expect(screen.getByText(RUNS[0].path)).not.toHaveAttribute('title');
  });

  describe('sorting (controlled)', () => {
    function Sorted({ onSort }: { onSort?: (s: TableSort) => void }) {
      const [sort, setSort] = useState<TableSort | null>({ column: 'name', direction: 'ascending' });
      return <Table {...base} sort={sort} onSortChange={s => { setSort(s); onSort?.(s); }} />;
    }

    it('puts a button in each sortable header, aria-sort on the sorted one only, and a direction icon', () => {
      render(<Sorted />);
      expect(headerCell('Workflow')).toHaveAttribute('aria-sort', 'ascending');
      expect(headerCell('Tokens')).not.toHaveAttribute('aria-sort');
      expect(within(headerCell('Status')).queryByRole('button')).toBeNull();
      const button = within(headerCell('Workflow')).getByRole('button', { name: 'Workflow' });
      expect(button).toHaveAttribute('type', 'button');
      expect(button.querySelector('[data-sort-icon="ascending"]')!.closest('[aria-hidden="true"]')).toBeTruthy();
      expect(within(headerCell('Tokens')).getByRole('button').querySelector('[data-sort-icon="none"]')).toBeTruthy();
    });

    it('asks for ascending on a new column, then the other way, from the mouse and the keyboard', async () => {
      const user = userEvent.setup();
      const onSort = vi.fn();
      render(<Sorted onSort={onSort} />);
      await user.click(within(headerCell('Workflow')).getByRole('button'));
      expect(onSort).toHaveBeenLastCalledWith({ column: 'name', direction: 'descending' });
      expect(headerCell('Workflow')).toHaveAttribute('aria-sort', 'descending');

      within(headerCell('Tokens')).getByRole('button').focus();
      await user.keyboard('{Enter}');
      expect(onSort).toHaveBeenLastCalledWith({ column: 'tokens', direction: 'ascending' });
      expect(headerCell('Tokens')).toHaveAttribute('aria-sort', 'ascending');
      expect(headerCell('Workflow')).not.toHaveAttribute('aria-sort');
    });

    it('does not sort the rows itself: the caller does', () => {
      render(<Table {...base} sort={{ column: 'name', direction: 'descending' }} onSortChange={() => {}} />);
      expect(screen.getAllByRole('rowheader').map(th => th.textContent)).toEqual(RUNS.map(r => r.name));
      expect(nextSort(null, 'a')).toEqual({ column: 'a', direction: 'ascending' });
      expect(nextSort({ column: 'a', direction: 'descending' }, 'a')).toEqual({ column: 'a', direction: 'ascending' });
    });
  });

  it('keeps the header row in view when stickyHeader is set', () => {
    const { container } = render(<Table {...base} stickyHeader />);
    expect(container.querySelector('.ui-table')).toHaveAttribute('data-sticky-header', 'true');
    expect(CSS).toMatch(/\[data-sticky-header="true"\] thead th\s*\{[^}]*position:\s*sticky[^}]*top:\s*0/);
    expect(CSS).toMatch(/\[data-sticky-header="true"\] thead th\s*\{[^}]*z-index:\s*var\(--kr-z-sticky\)/);
  });

  it('bounds its height in three presets: the scroll area becomes a focusable region named by the caption', () => {
    const { container, rerender } = render(<Table {...base} />);
    const scroll = () => container.querySelector('.ui-table-scroll')!;
    // The horizontal-scroll wrapper is always there; unbounded and not
    // overflowing, it is no tab stop.
    expect(scroll()).toBeInTheDocument();
    expect(scroll()).not.toHaveAttribute('tabindex');
    expect(scroll()).not.toHaveAttribute('role');
    for (const size of ['sm', 'md', 'lg'] as const) {
      rerender(<Table {...base} maxHeight={size} />);
      expect(scroll()).toHaveAttribute('data-max-height', size);
      expect(scroll()).toHaveAttribute('tabindex', '0');
      expect(screen.getByRole('region', { name: 'Workflow runs' })).toBe(scroll());
      expect(CSS).toMatch(new RegExp(`\\[data-max-height="${size}"\\]\\s*\\{[^}]*max-height:`));
    }
    expect(CSS).toMatch(/\.ui-table-scroll\s*\{[^}]*overflow:\s*auto/);
  });

  it('writes both densities at 12 px or more', () => {
    const { container, rerender } = render(<Table {...base} />);
    expect(container.querySelector('.ui-table')).toHaveAttribute('data-density', 'normal');
    rerender(<Table {...base} density="compact" />);
    expect(container.querySelector('.ui-table')).toHaveAttribute('data-density', 'compact');
    for (const [, size] of CSS.matchAll(/font-size:\s*var\(--kr-type-(\w+)\)/g)) expect(['sm', 'md', 'lg']).toContain(size);
  });

  it('tints a row by its tone, and lights rows on hover when asked', () => {
    const tones = { r1: 'danger', r2: 'muted' } as const;
    const { container } = render(<Table {...base} rowTone={r => tones[r.id as keyof typeof tones]} hover />);
    const rows = [...container.querySelectorAll('tbody tr')];
    expect(rows.map(r => r.getAttribute('data-tone'))).toEqual(['danger', 'muted', null]);
    expect(container.querySelector('.ui-table')).toHaveAttribute('data-hover', 'true');
    for (const tone of ['danger', 'warning', 'muted']) expect(CSS, tone).toContain(`tr[data-tone="${tone}"]`);
  });

  describe('selection', () => {
    function Selectable({ initial = [] as string[] }) {
      const [selected, setSelected] = useState<string[]>(initial);
      return (
        <>
          <Table {...base} selection={{ selected, onChange: setSelected, rowLabel: r => r.name }} />
          <output data-testid="selected">{selected.join(',')}</output>
        </>
      );
    }

    it('gives each row a checkbox named after it, and a select-all in the header', async () => {
      const user = userEvent.setup();
      render(<Selectable />);
      const all = screen.getByRole('checkbox', { name: 'ui.table.selectAll' });
      expect(all.closest('th')).toHaveAttribute('scope', 'col');
      // "Select {0}", the row's label as its argument (the test dictionary
      // prints the key alone).
      const boxes = screen.getAllByRole('checkbox', { name: 'ui.table.selectRow' });
      expect(boxes).toHaveLength(RUNS.length);
      for (const box of boxes) expect(box.closest('td')).toHaveClass('ui-table-select');
      await user.click(boxes[1]);
      expect(screen.getByTestId('selected')).toHaveTextContent('r2');
      expect(all).toBePartiallyChecked();
      // The checkbox is the only semantic of the selection: aria-selected
      // belongs to a grid row, not to the row of a plain table. The row only
      // carries a data attribute for its colour.
      expect(screen.getAllByRole('row')[2]).toHaveAttribute('data-selected', 'true');
      expect(screen.getAllByRole('row')[1]).not.toHaveAttribute('data-selected');
      for (const row of screen.getAllByRole('row')) expect(row).not.toHaveAttribute('aria-selected');
    });

    it('colours a selected row with the accent selection tint, over its tone tint and keeping its tone border', () => {
      expect(CSS).not.toContain('aria-selected');
      expect(CSS).not.toContain('--kr-bg-selected');
      // The token of the selected rows of the collection lists (CollectionShell.css).
      const rule = CSS.match(/tbody tr\[data-selected="true"\]\s*\{([^}]*)\}/)?.[1] ?? '';
      expect(rule).toMatch(/background-image:\s*linear-gradient\(var\(--kr-accent-bg\), var\(--kr-accent-bg\)\)/);
      // An image over the colour: the danger / warning tint stays underneath,
      // and nothing in the rule touches the tone border.
      expect(rule).not.toMatch(/background-color|border/);
      expect(CSS).toMatch(/tr\[data-tone="danger"\] > :first-child\s*\{[^}]*border-inline-start/);
      const query = CSS.match(/@container ui-table \(width < 40rem\)\s*\{([\s\S]*)\}/)?.[1] ?? '';
      expect(query).toMatch(/\[data-narrow="cards"\] tbody tr\[data-tone="danger"\]\s*\{[^}]*border-inline-start/);
      expect(query).not.toContain('data-selected');
    });

    it('selects all, then none, from the header; the mixed state when some are selected', async () => {
      const user = userEvent.setup();
      render(<Selectable initial={['r1']} />);
      const all = screen.getByRole('checkbox', { name: 'ui.table.selectAll' });
      expect(all).toBePartiallyChecked();
      await user.click(all);
      expect(screen.getByTestId('selected')).toHaveTextContent('r1,r2,r3');
      expect(all).toBeChecked();
      expect(all).not.toBePartiallyChecked();
      await user.click(all);
      expect(screen.getByTestId('selected')).toHaveTextContent('');
      expect(all).not.toBeChecked();
    });
  });

  it('adds an actions column whose header has a hidden name', () => {
    render(
      <Table
        {...base}
        actions={r => <IconButton size="sm" label={`Delete ${r.name}`} icon={<span>x</span>} />}
      />,
    );
    const header = screen.getByRole('columnheader', { name: 'ui.table.actions' });
    expect(header.querySelector('.ui-visually-hidden')).toHaveTextContent('ui.table.actions');
    expect(screen.getByRole('button', { name: 'Delete Weekly audit' }).closest('td')).toHaveClass('ui-table-actions');
  });

  describe('row open', () => {
    it('opens a row from a real button in its first cell, with the keyboard', async () => {
      const user = userEvent.setup();
      const onRowOpen = vi.fn();
      render(<Table {...base} onRowOpen={onRowOpen} />);
      const open = within(screen.getAllByRole('rowheader')[1]).getByRole('button', { name: 'Weekly audit' });
      open.focus();
      await user.keyboard('{Enter}');
      expect(onRowOpen).toHaveBeenCalledWith(RUNS[1]);
      await user.keyboard(' ');
      expect(onRowOpen).toHaveBeenCalledTimes(2);
    });

    it('opens on a click anywhere in the row, never from a control inside it', async () => {
      const user = userEvent.setup();
      const onRowOpen = vi.fn();
      const onDelete = vi.fn();
      render(
        <Table
          {...base}
          onRowOpen={onRowOpen}
          selection={{ selected: [], onChange: () => {}, rowLabel: r => r.name }}
          actions={r => <button type="button" onClick={onDelete}>{`Delete ${r.name}`}</button>}
        />,
      );
      await user.click(screen.getByText('98,000'));
      expect(onRowOpen).toHaveBeenCalledTimes(1);
      expect(onRowOpen).toHaveBeenLastCalledWith(RUNS[1]);
      // The open button itself fires once, not twice.
      await user.click(screen.getByRole('button', { name: 'Nightly digest' }));
      expect(onRowOpen).toHaveBeenCalledTimes(2);
      await user.click(screen.getByRole('button', { name: 'Delete Release notes' }));
      await user.click(screen.getAllByRole('checkbox', { name: 'ui.table.selectRow' })[2]);
      expect(onDelete).toHaveBeenCalledTimes(1);
      expect(onRowOpen).toHaveBeenCalledTimes(2);
    });

    it('puts the open button in the first data column when no column is a row header', () => {
      render(<Table {...base} columns={COLUMNS.map(c => ({ ...c, rowHeader: false }))} onRowOpen={() => {}} />);
      const firstCells = screen.getAllByRole('row').slice(1).map(row => within(row).getAllByRole('cell')[0]);
      for (const [i, cell] of firstCells.entries()) expect(within(cell).getByRole('button')).toHaveTextContent(RUNS[i].name);
    });
  });

  describe('empty and loading', () => {
    it('shows the empty state in one row spanning every column', () => {
      render(
        <Table
          {...base}
          rows={[]}
          selection={{ selected: [], onChange: () => {}, rowLabel: r => r.name }}
          actions={() => null}
          empty={<EmptyState kind="noResult" title="No run matches" onClear={() => {}} />}
        />,
      );
      const rows = screen.getAllByRole('row');
      expect(rows).toHaveLength(2);
      const cell = within(rows[1]).getByRole('cell');
      expect(cell).toHaveAttribute('colspan', String(COLUMNS.length + 2));
      expect(within(cell).getByText('No run matches')).toBeInTheDocument();
      // Nothing to select: the select-all is disabled.
      expect(screen.getByRole('checkbox', { name: 'ui.table.selectAll' })).toBeDisabled();
    });

    it('says "Nothing to show" by default when there are no rows', () => {
      render(<Table {...base} rows={[]} />);
      expect(within(screen.getAllByRole('row')[1]).getByRole('cell')).toHaveTextContent('ui.table.empty');
    });

    it('shows a loading row, a status, in place of the rows', () => {
      const { rerender } = render(<Table {...base} loading />);
      const rows = screen.getAllByRole('row');
      expect(rows).toHaveLength(2);
      const status = within(rows[1]).getByRole('status');
      expect(status).toHaveTextContent('ui.loading.label');
      expect(within(rows[1]).getByRole('cell')).toHaveAttribute('colspan', String(COLUMNS.length));
      expect(screen.getByRole('table')).toHaveAttribute('aria-busy', 'true');
      rerender(<Table {...base} loading loadingLabel="Loading the runs…" />);
      expect(screen.getByRole('status')).toHaveTextContent('Loading the runs…');
    });
  });

  it('renders a footer slot under the table (pager, show more, "N more rows")', () => {
    const { container } = render(<Table {...base} footer={<p>12 more rows</p>} />);
    const footer = container.querySelector('.ui-table-footer')!;
    expect(footer).toHaveTextContent('12 more rows');
    expect(container.querySelector('.ui-table-scroll')!.compareDocumentPosition(footer) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  describe('narrow forms, by the container width', () => {
    it('scrolls sideways by default: the root is a size container and nothing turns into cards', () => {
      const { container } = render(<Table {...base} />);
      expect(container.querySelector('.ui-table')).toHaveAttribute('data-narrow', 'scroll');
      expect(CSS).toMatch(/\.ui-table\s*\{[^}]*container-type:\s*inline-size/);
      expect(CSS).toMatch(/\.ui-table\s*\{[^}]*container-name:\s*ui-table/);
      expect(CSS).not.toMatch(/@media[^{]*width/);
    });

    it('turns each row into a card in cards form, every value preceded by its column header', () => {
      const { container } = render(<Table {...base} narrow="cards" actions={() => <button type="button">Act</button>} />);
      expect(container.querySelector('.ui-table')).toHaveAttribute('data-narrow', 'cards');
      const firstRow = screen.getAllByRole('row')[1];
      const labels = [...firstRow.querySelectorAll('.ui-table-cell-label')];
      expect(labels.map(l => l.textContent)).toEqual(['Workflow', 'Status', 'Tokens', 'Worktree', 'ui.table.actions']);
      // The header row says it already to assistive technology.
      for (const label of labels) expect(label).toHaveAttribute('aria-hidden', 'true');

      const query = CSS.match(/@container ui-table \(width < 40rem\)\s*\{([\s\S]*)\}/)?.[1] ?? '';
      expect(query, 'a container query, not a window query').not.toBe('');
      expect(query).toMatch(/\[data-narrow="cards"\] tbody tr\s*\{[^}]*display:\s*(block|grid)/);
      expect(query).toMatch(/\[data-narrow="cards"\] \.ui-table-cell-label\s*\{[^}]*display:\s*block/);
      // The header row is visually hidden, and shows again when a control in
      // it (a sort button, the select-all) takes the focus.
      expect(query).toMatch(/\[data-narrow="cards"\] thead\s*\{[^}]*clip-path:\s*inset\(50%\)/);
      expect(query).toMatch(/\[data-narrow="cards"\] thead:focus-within\s*\{[^}]*clip-path:\s*none/);
      expect(CSS).toMatch(/\.ui-table-cell-label\s*\{[^}]*display:\s*none/);
    });

    it('writes the table roles out in cards form, so a value keeps its row, its cell and its column header', () => {
      const { container } = render(
        <Table
          {...base}
          narrow="cards"
          selection={{ selected: ['r2'], onChange: () => {}, rowLabel: r => r.name }}
          actions={r => <button type="button">{`Delete ${r.name}`}</button>}
        />,
      );
      const table = container.querySelector('table')!;
      expect(table).toHaveAttribute('role', 'table');
      expect(screen.getByRole('table', { name: 'Workflow runs' })).toBe(table);
      expect(container.querySelector('thead')).toHaveAttribute('role', 'rowgroup');
      expect(container.querySelector('tbody')).toHaveAttribute('role', 'rowgroup');
      for (const tr of container.querySelectorAll('tr')) expect(tr).toHaveAttribute('role', 'row');
      for (const th of container.querySelectorAll('thead th')) expect(th).toHaveAttribute('role', 'columnheader');

      // A row is found by its role, with its cells.
      const row = screen.getByRole('row', { name: /Weekly audit/ });
      expect(row).toHaveAttribute('data-selected', 'true');
      expect(within(row).getByRole('rowheader')).toHaveTextContent('Weekly audit');
      const cells = within(row).getAllByRole('cell');
      expect(cells).toHaveLength(5); // checkbox, status, tokens, worktree, actions
      for (const cell of cells) expect(cell).toHaveAttribute('role', 'cell');

      // Each cell names its column header, whose name is the column's.
      const headerOf = (cell: Element) => document.getElementById(cell.getAttribute('headers') ?? '')!;
      const tokens = within(row).getByText('98,000').closest('td')!;
      expect(headerOf(tokens)).toHaveAttribute('role', 'columnheader');
      expect(headerOf(tokens)).toHaveAccessibleName('Tokens');
      expect(headerOf(within(row).getByRole('rowheader'))).toHaveAccessibleName('Workflow');
      expect(headerOf(cells[0])).toHaveAccessibleName('ui.table.selectAll');
      expect(headerOf(cells[4])).toHaveAccessibleName('ui.table.actions');
      // The same position too: the header at the cell's index is its column's.
      const headerRow = screen.getAllByRole('row')[0];
      expect(within(headerRow).getAllByRole('columnheader')[[...row.children].indexOf(tokens)]).toBe(headerOf(tokens));
    });

    it('leaves the native roles implicit in the wide form', () => {
      const { container } = render(<Table {...base} />);
      for (const el of container.querySelectorAll('table, thead, tbody, tr, th, td')) {
        expect(el).not.toHaveAttribute('role');
        expect(el).not.toHaveAttribute('headers');
      }
    });
  });

  it('passes axe with every option on', async () => {
    const user = userEvent.setup();
    render(
      <main>
        <Table
          {...base}
          sort={{ column: 'tokens', direction: 'descending' }}
          onSortChange={() => {}}
          stickyHeader
          maxHeight="md"
          density="compact"
          rowTone={r => (r.id === 'r1' ? 'danger' : undefined)}
          selection={{ selected: ['r2'], onChange: () => {}, rowLabel: r => r.name }}
          actions={r => <IconButton size="sm" label={`Delete ${r.name}`} icon={<span>x</span>} />}
          onRowOpen={() => {}}
          narrow="cards"
          footer={<p>12 more rows</p>}
        />
        <Table {...base} caption="Empty" rows={[]} />
        <Table {...base} caption="Loading" loading />
      </main>,
    );
    await user.tab();
    const results = await act(() => runAxe(document.querySelector('main')!));
    expect(describeViolations(results)).toEqual([]);
  });
});
