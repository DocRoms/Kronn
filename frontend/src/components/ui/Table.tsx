import {
  useEffect, useId, useLayoutEffect, useRef, useState,
  type ComponentPropsWithoutRef, type MouseEvent, type ReactNode, type RefObject,
} from 'react';
import { ArrowDown, ArrowUp, ArrowUpDown } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { LoadingState } from './LoadingState';
import {
  nextSort,
  type TableDensity,
  type TableMaxHeight,
  type TableNarrow,
  type TableRowTone,
  type TableSort,
} from './dataKinds';
import './visuallyHidden.css';
import './Table.css';

export interface TableColumn<Row> {
  /** Stable id: the sort refers to it. */
  id: string;
  /** The header text; also the label before each value in the cards form. */
  header: ReactNode;
  /** What the cell shows. */
  cell: (row: Row) => ReactNode;
  /** Default start; `numeric` implies end. */
  align?: 'start' | 'center' | 'end';
  /** A number: end-aligned, tabular figures. */
  numeric?: boolean;
  /** The header is a button that asks for this sort (`onSortChange`). */
  sortable?: boolean;
  /** ellipsis: one line, cut, the full value in a title; wrap: breaks
   *  anywhere (a path, a hash). Default: wraps between words. */
  truncate?: 'ellipsis' | 'wrap';
  /** The full value an ellipsis hides, when `cell` is not a string. */
  text?: (row: Row) => string;
  /** The cell names its row: a `th scope="row"`. */
  rowHeader?: boolean;
}

export interface TableSelection<Row> {
  /** The ids (`getRowId`) of the selected rows. */
  selected: readonly string[];
  onChange: (ids: string[]) => void;
  /** The row in words, for its checkbox: "Select {rowLabel}". */
  rowLabel: (row: Row) => string;
  /** The select-all's name; default "Select all rows". */
  allLabel?: string;
}

export interface TableProps<Row> {
  /** What the table holds. Required: it names the table (and its scroll
   *  region). */
  caption: ReactNode;
  /** Keep the caption for assistive technology only (a heading above already
   *  says it). */
  hideCaption?: boolean;
  columns: readonly TableColumn<Row>[];
  rows: readonly Row[];
  getRowId: (row: Row) => string;
  /** Controlled: the table never sorts the rows, the caller does. */
  sort?: TableSort | null;
  onSortChange?: (sort: TableSort) => void;
  stickyHeader?: boolean;
  /** Bounds the height: the scroll area becomes a focusable region. */
  maxHeight?: TableMaxHeight;
  density?: TableDensity;
  rowTone?: (row: Row) => TableRowTone | undefined;
  /** Light the row under the pointer. Default: on when rows open. */
  hover?: boolean;
  selection?: TableSelection<Row>;
  /** A last column of controls on the row (IconButtons, a Menu). */
  actions?: (row: Row) => ReactNode;
  /** The actions column's hidden header; default "Actions". */
  actionsLabel?: string;
  /** Opens a row: its first data cell holds a button (the keyboard way); a
   *  click elsewhere on the row is a mouse shortcut, never fired from a
   *  control inside the row. */
  onRowOpen?: (row: Row) => void;
  /** What shows when there are no rows: an EmptyState, or a sentence.
   *  Default "Nothing to show". */
  empty?: ReactNode;
  loading?: boolean;
  /** The loading sentence; default "Loading…". */
  loadingLabel?: string;
  /** Under the table: a pager, "show more", an "N more rows" note. */
  footer?: ReactNode;
  /** A narrow container (under 40rem): scroll (default) or cards. */
  narrow?: TableNarrow;
  className?: string;
}

/** What a click on a row must leave to the control it lands on. */
const INTERACTIVE = [
  'a[href]', 'button', 'input', 'select', 'textarea', 'label', 'summary',
  '[role="button"]', '[role="link"]', '[role="checkbox"]', '[role="switch"]', '[role="menuitem"]',
  '[contenteditable="true"]', '[tabindex]:not([tabindex="-1"])',
].join(', ');

const SORT_ICONS = {
  ascending: <ArrowUp data-sort-icon="ascending" />,
  descending: <ArrowDown data-sort-icon="descending" />,
  none: <ArrowUpDown data-sort-icon="none" />,
};

/** Whether `el` is wider than its box: a region the keyboard must reach. */
function useOverflowsSideways(ref: RefObject<HTMLElement | null>): boolean {
  const [overflows, setOverflows] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setOverflows(el.scrollWidth > el.clientWidth + 1);
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    if (el.firstElementChild) observer.observe(el.firstElementChild);
    return () => observer.disconnect();
  }, [ref]);
  return overflows;
}

/** A checkbox whose mixed state follows `indeterminate`. */
function Checkbox({ indeterminate = false, ...rest }: ComponentPropsWithoutRef<'input'> & { indeterminate?: boolean }) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = indeterminate;
  }, [indeterminate]);
  return <input ref={ref} type="checkbox" className="ui-table-checkbox" {...rest} />;
}

/** KT-1886 — a data table. Native table semantics, a required caption,
 *  controlled sorting, an always-present horizontal scroll wrapper, and a
 *  narrow form decided by the width of its container, not of the window. */
export function Table<Row>({
  caption, hideCaption = false, columns, rows, getRowId, sort, onSortChange, stickyHeader = false, maxHeight,
  density = 'normal', rowTone, hover, selection, actions, actionsLabel, onRowOpen, empty, loading = false,
  loadingLabel, footer, narrow = 'scroll', className,
}: TableProps<Row>) {
  const { t } = useT();
  const captionId = useId();
  const scrollRef = useRef<HTMLDivElement>(null);
  const overflows = useOverflowsSideways(scrollRef);
  const region = maxHeight !== undefined || overflows;
  const cards = narrow === 'cards';
  const actionsName = actionsLabel ?? t('ui.table.actions');
  const openColumn = onRowOpen ? (columns.find(c => c.rowHeader) ?? columns[0])?.id : undefined;
  const span = columns.length + (selection ? 1 : 0) + (actions ? 1 : 0);

  const rowIds = rows.map(getRowId);
  const selected = new Set(selection?.selected ?? []);
  const selectedCount = rowIds.filter(id => selected.has(id)).length;
  const allSelected = rowIds.length > 0 && selectedCount === rowIds.length;

  const toggleAll = () => {
    if (!selection) return;
    const others = selection.selected.filter(id => !rowIds.includes(id));
    selection.onChange(allSelected ? others : [...others, ...rowIds]);
  };
  const toggleRow = (id: string) => {
    if (!selection) return;
    selection.onChange(selected.has(id) ? selection.selected.filter(x => x !== id) : [...selection.selected, id]);
  };

  const rowClick = (row: Row) => (e: MouseEvent<HTMLTableRowElement>) => {
    const tr = e.currentTarget;
    const target = e.target as Element;
    // A click from a portal (a menu, a tooltip, a dialog) is not on the row.
    if (!tr.contains(target)) return;
    const control = target.closest(INTERACTIVE);
    if (control && tr.contains(control)) return;
    // Selecting text in a cell is not opening the row.
    if (typeof window !== 'undefined' && window.getSelection()?.toString()) return;
    onRowOpen?.(row);
  };

  const classes = ['ui-table', className].filter(Boolean).join(' ');
  const label = (text: ReactNode) => cards ? <span className="ui-table-cell-label" aria-hidden="true">{text}</span> : null;
  // The cards form changes the display of the table, its rows and cells, and
  // engines differ on whether a table keeps its roles then: the roles are
  // written out (they equal the native ones), and each cell names its column
  // header by id, so a value is never read without its header.
  const role = (r: 'table' | 'rowgroup' | 'row' | 'columnheader' | 'rowheader' | 'cell') => (cards ? r : undefined);
  const headerId = (key: string) => `${captionId}-h-${key}`;
  const headers = (key: string) => (cards ? headerId(key) : undefined);
  const columnKeys = columns.map((_, i) => String(i));

  const body = () => {
    if (loading) {
      return (
        <tr className="ui-table-state-row" role={role('row')}>
          <td colSpan={span} role={role('cell')}><LoadingState size="sm" message={loadingLabel ?? t('ui.loading.label')} /></td>
        </tr>
      );
    }
    if (rows.length === 0) {
      return (
        <tr className="ui-table-state-row" role={role('row')}>
          <td colSpan={span} role={role('cell')}>{empty ?? <p className="ui-table-empty">{t('ui.table.empty')}</p>}</td>
        </tr>
      );
    }
    return rows.map(row => {
      const id = getRowId(row);
      const isSelected = selected.has(id);
      return (
        <tr
          key={id}
          data-tone={rowTone?.(row)}
          data-opens={onRowOpen ? 'true' : undefined}
          data-selected={isSelected ? 'true' : undefined}
          role={role('row')}
          onClick={onRowOpen ? rowClick(row) : undefined}
        >
          {selection && (
            <td className="ui-table-select" role={role('cell')} headers={headers('select')}>
              <Checkbox
                checked={isSelected}
                onChange={() => toggleRow(id)}
                aria-label={t('ui.table.selectRow', selection.rowLabel(row))}
              />
            </td>
          )}
          {columns.map((col, i) => {
            const content = col.cell(row);
            const full = col.truncate === 'ellipsis'
              ? col.text?.(row) ?? (typeof content === 'string' || typeof content === 'number' ? String(content) : undefined)
              : undefined;
            let value: ReactNode = col.truncate === 'ellipsis'
              ? <span className="ui-table-value ui-table-truncate" title={full}>{content}</span>
              : <span className="ui-table-value">{content}</span>;
            if (col.id === openColumn) {
              value = <button type="button" className="ui-table-open" onClick={() => onRowOpen?.(row)}>{value}</button>;
            }
            const props = {
              'data-align': col.numeric ? 'end' : col.align ?? 'start',
              'data-numeric': col.numeric ? 'true' : undefined,
              'data-truncate': col.truncate,
              headers: headers(columnKeys[i]),
            };
            return col.rowHeader
              ? <th key={col.id} scope="row" role={role('rowheader')} {...props}>{label(col.header)}{value}</th>
              : <td key={col.id} role={role('cell')} {...props}>{label(col.header)}{value}</td>;
          })}
          {actions && <td className="ui-table-actions" role={role('cell')} headers={headers('actions')}>{label(actionsName)}<span className="ui-table-actions-items">{actions(row)}</span></td>}
        </tr>
      );
    });
  };

  return (
    <div
      className={classes}
      data-density={density}
      data-narrow={narrow}
      data-sticky-header={stickyHeader ? 'true' : undefined}
      data-hover={(hover ?? Boolean(onRowOpen)) ? 'true' : undefined}
    >
      <div
        ref={scrollRef}
        className="ui-table-scroll"
        data-max-height={maxHeight}
        role={region ? 'region' : undefined}
        aria-labelledby={region ? captionId : undefined}
        tabIndex={region ? 0 : undefined}
      >
        <table aria-busy={loading || undefined} role={role('table')} aria-labelledby={cards ? captionId : undefined}>
          <caption id={captionId} className={hideCaption ? 'ui-table-caption ui-visually-hidden' : 'ui-table-caption'}>
            {caption}
          </caption>
          <thead role={role('rowgroup')}>
            <tr role={role('row')}>
              {selection && (
                <th scope="col" className="ui-table-select" role={role('columnheader')} id={cards ? headerId('select') : undefined}>
                  <Checkbox
                    checked={allSelected}
                    indeterminate={selectedCount > 0 && !allSelected}
                    disabled={rows.length === 0 || loading}
                    onChange={toggleAll}
                    aria-label={selection.allLabel ?? t('ui.table.selectAll')}
                  />
                </th>
              )}
              {columns.map((col, i) => {
                const sorted = sort?.column === col.id ? sort.direction : undefined;
                return (
                  <th
                    key={col.id}
                    scope="col"
                    role={role('columnheader')}
                    id={cards ? headerId(columnKeys[i]) : undefined}
                    aria-sort={sorted}
                    data-align={col.numeric ? 'end' : col.align ?? 'start'}
                    data-numeric={col.numeric ? 'true' : undefined}
                  >
                    {col.sortable && onSortChange ? (
                      <button type="button" className="ui-table-sort" onClick={() => onSortChange(nextSort(sort, col.id))}>
                        <span>{col.header}</span>
                        <span className="ui-table-sort-icon" aria-hidden="true" data-sorted={sorted ? 'true' : undefined}>
                          {SORT_ICONS[sorted ?? 'none']}
                        </span>
                      </button>
                    ) : col.header}
                  </th>
                );
              })}
              {actions && (
                <th scope="col" className="ui-table-actions" role={role('columnheader')} id={cards ? headerId('actions') : undefined}><span className="ui-visually-hidden">{actionsName}</span></th>
              )}
            </tr>
          </thead>
          <tbody role={role('rowgroup')}>{body()}</tbody>
        </table>
      </div>
      {footer && <div className="ui-table-footer">{footer}</div>}
    </div>
  );
}
