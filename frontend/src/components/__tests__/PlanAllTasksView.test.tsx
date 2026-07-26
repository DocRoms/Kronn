import { beforeAll, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { PlanAllTasksView } from '../PlanAllTasksView';
import type { PlanningDiscussionRelation, PlanningTaskSummary } from '../../types/generated';

// jsdom has no layout, so TanStack Virtual would see a 0-height viewport and
// window nothing. Give the scroll container a viewport and every row a fixed
// height so virtualization actually renders a bounded window we can assert on.
const rowHeight = (el: HTMLElement) => (el.classList?.contains('plan-all-scroll') ? 480 : 60);

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, 'getBoundingClientRect', {
    configurable: true,
    value(this: HTMLElement) {
      const height = rowHeight(this);
      return {
        height,
        width: 320,
        top: 0,
        left: 0,
        right: 320,
        bottom: height,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect;
    },
  });
  for (const prop of ['offsetHeight', 'clientHeight', 'scrollHeight'] as const) {
    Object.defineProperty(HTMLElement.prototype, prop, {
      configurable: true,
      get(this: HTMLElement) {
        return rowHeight(this);
      },
    });
  }
  // A ResizeObserver that actually FIRES so the virtualizer receives a viewport
  // rect (the no-op jsdom/polyfill default leaves it with a 0-height window and
  // it renders nothing).
  class FiringResizeObserver {
    constructor(private cb: ResizeObserverCallback) {}
    observe(el: Element) {
      this.cb(
        [{ target: el, contentRect: el.getBoundingClientRect() } as ResizeObserverEntry],
        this as unknown as ResizeObserver,
      );
    }
    unobserve() {}
    disconnect() {}
  }
  (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = FiringResizeObserver;
  (window as unknown as { ResizeObserver: unknown }).ResizeObserver = FiringResizeObserver;
});

const echoT = (key: string, ...args: (string | number)[]) =>
  (args.length > 0 ? `${key}:${args.join('/')}` : key);

function task(overrides: Partial<PlanningTaskSummary> = {}): PlanningTaskSummary {
  return {
    id: 'task-1',
    reference: 'KT-1',
    parent_id: null,
    parent_reference: null,
    parent_title: null,
    title: 'A task',
    status: 'todo',
    priority: 'normal',
    rank: 1,
    completed_subtasks: 0,
    total_subtasks: 0,
    project_ids: [],
    discussion_ids: [],
    tags: [],
    blocker_count: 0,
    created_at: '2026-07-25T00:00:00Z',
    updated_at: '2026-07-25T00:00:00Z',
    ...overrides,
  };
}

function relation(taskOverrides: Partial<PlanningTaskSummary>): PlanningDiscussionRelation {
  return {
    placement: 'active',
    is_primary: false,
    position: 0,
    task: task(taskOverrides),
    active_blockers: [],
    actionable: true,
  };
}

const renderTask = (r: PlanningDiscussionRelation) => (
  <span>{`${r.task.title} (${r.task.reference})`}</span>
);

function renderView(props: Partial<React.ComponentProps<typeof PlanAllTasksView>> = {}) {
  const onSelect = vi.fn();
  const onQueryChange = vi.fn();
  const utils = render(
    <PlanAllTasksView
      active={props.active ?? [relation({ id: 't1', reference: 'KT-1', title: 'Alpha' })]}
      later={props.later ?? [relation({ id: 't2', reference: 'KT-2', title: 'Beta' })]}
      query={props.query ?? ''}
      onQueryChange={props.onQueryChange ?? onQueryChange}
      selectedTaskId={props.selectedTaskId ?? null}
      onSelect={props.onSelect ?? onSelect}
      renderTask={props.renderTask ?? renderTask}
      t={echoT}
    />,
  );
  return { ...utils, onSelect, onQueryChange };
}

describe('PlanAllTasksView', () => {
  it('renders without any React console error (e.g. a key-in-spread warning)', () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    // Exercises both the header and option branches (key must be a direct prop).
    renderView();
    expect(errorSpy).not.toHaveBeenCalled();
    errorSpy.mockRestore();
  });

  it('renders Active and Later groups with their rows', () => {
    renderView();
    expect(screen.getByText('planning.allGroupActive')).toBeInTheDocument();
    expect(screen.getByText('planning.allGroupLater')).toBeInTheDocument();
    expect(screen.getByText('Alpha (KT-1)')).toBeInTheDocument();
    expect(screen.getByText('Beta (KT-2)')).toBeInTheDocument();
    // Group headers are presentational — never options.
    expect(screen.getAllByRole('option')).toHaveLength(2);
  });

  it('filters by title, reference and parent crumb, dropping empty groups', () => {
    const active = [
      relation({ id: 't1', reference: 'KT-1', title: 'Alpha' }),
      relation({
        id: 't2',
        reference: 'KT-2',
        title: 'Beta',
        parent_reference: 'KT-9',
        parent_title: 'Parent crumb',
      }),
    ];
    const later = [relation({ id: 't3', reference: 'KT-3', title: 'Gamma' })];

    // Match by parent crumb → only Beta, and the (empty) Later group is dropped.
    renderView({ active, later, query: 'crumb' });
    expect(screen.getByText('Beta (KT-2)')).toBeInTheDocument();
    expect(screen.queryByText('Alpha (KT-1)')).not.toBeInTheDocument();
    expect(screen.queryByText('Gamma (KT-3)')).not.toBeInTheDocument();
    expect(screen.queryByText('planning.allGroupLater')).not.toBeInTheDocument();
  });

  it('matches by reference too', () => {
    renderView({ query: 'kt-2' });
    expect(screen.getByText('Beta (KT-2)')).toBeInTheDocument();
    expect(screen.queryByText('Alpha (KT-1)')).not.toBeInTheDocument();
  });

  it.each([200, 1000])('keeps the DOM bounded for a %i-row plan', (count) => {
    const active = Array.from({ length: count }, (_, i) =>
      relation({ id: `t${i}`, reference: `KT-${i}`, title: `Task ${i}` }),
    );
    renderView({ active, later: [] });
    const rendered = screen.getAllByRole('option').length;
    expect(rendered).toBeGreaterThan(0);
    expect(rendered).toBeLessThan(100); // virtualised window, not all rows
  });

  it('reflects the selected task as aria-selected', () => {
    renderView({ selectedTaskId: 't2' });
    const selected = screen.getByText('Beta (KT-2)').closest('[role="option"]');
    expect(selected).toHaveAttribute('aria-selected', 'true');
    const other = screen.getByText('Alpha (KT-1)').closest('[role="option"]');
    expect(other).toHaveAttribute('aria-selected', 'false');
  });

  it('navigates rows by keyboard (skipping headers) and selects with Enter', () => {
    const { onSelect } = renderView();
    const listbox = screen.getByRole('listbox');
    // First selectable row is active initially.
    expect(listbox).toHaveAttribute('aria-activedescendant', 'plan-all-option-t1');
    // ArrowDown moves to the next ROW — never lands on the Later header.
    fireEvent.keyDown(listbox, { key: 'ArrowDown' });
    expect(listbox).toHaveAttribute('aria-activedescendant', 'plan-all-option-t2');
    fireEvent.keyDown(listbox, { key: 'Enter' });
    expect(onSelect).toHaveBeenCalledWith('t2');
  });

  it('syncs the roving pointer to a clicked row (click then Enter both hit it)', () => {
    const { onSelect } = renderView();
    const listbox = screen.getByRole('listbox');
    // Beta is NOT the initially-active row (Alpha is).
    fireEvent.click(screen.getByText('Beta (KT-2)'));
    expect(onSelect).toHaveBeenCalledWith('t2');
    expect(listbox).toHaveAttribute('aria-activedescendant', 'plan-all-option-t2');
    // Enter now re-activates Beta, not the previously-active Alpha.
    onSelect.mockClear();
    fireEvent.keyDown(listbox, { key: 'Enter' });
    expect(onSelect).toHaveBeenCalledWith('t2');
  });

  it('shows an empty state when the search matches nothing', () => {
    renderView({ query: 'no-such-task' });
    expect(screen.getByText('planning.allEmpty')).toBeInTheDocument();
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  });

  it('forwards search input changes to the parent', () => {
    const { onQueryChange } = renderView();
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'alp' } });
    expect(onQueryChange).toHaveBeenCalledWith('alp');
  });

  it('falls back to the first option by IDENTITY when the active row leaves a same-length result', () => {
    const first = [
      relation({ id: 'a', reference: 'KT-A', title: 'AAA' }),
      relation({ id: 'b', reference: 'KT-B', title: 'BBB' }),
      relation({ id: 'c', reference: 'KT-C', title: 'CCC' }),
    ];
    const { rerender } = renderView({ active: first, later: [] });
    const listbox = screen.getByRole('listbox');
    // Move the roving pointer OFF the first option.
    fireEvent.keyDown(listbox, { key: 'ArrowDown' });
    expect(listbox).toHaveAttribute('aria-activedescendant', 'plan-all-option-b');

    // A new result of the SAME length (3) that no longer contains 'b'. An
    // index-based fallback would keep index 1 ('y'); identity-based falls back
    // to the first visible option ('x'). Same instance → the effect re-runs.
    const replaced = [
      relation({ id: 'x', reference: 'KT-X', title: 'XXX' }),
      relation({ id: 'y', reference: 'KT-Y', title: 'YYY' }),
      relation({ id: 'z', reference: 'KT-Z', title: 'ZZZ' }),
    ];
    rerender(
      <PlanAllTasksView
        active={replaced}
        later={[]}
        query=""
        onQueryChange={vi.fn()}
        selectedTaskId={null}
        onSelect={vi.fn()}
        renderTask={renderTask}
        t={echoT}
      />,
    );
    expect(listbox).toHaveAttribute('aria-activedescendant', 'plan-all-option-x');
  });

  it('does not clear the parent selection when the filter changes', () => {
    // Selection is parent-owned: filtering out the selected row must not call
    // onSelect (the parent keeps selectedTaskId; the detail may stay open).
    const { rerender, onSelect } = renderView({ selectedTaskId: 't1' });
    rerender(
      <PlanAllTasksView
        active={[relation({ id: 't2', reference: 'KT-2', title: 'Beta' })]}
        later={[]}
        query="beta"
        onQueryChange={vi.fn()}
        selectedTaskId="t1"
        onSelect={onSelect}
        renderTask={renderTask}
        t={echoT}
      />,
    );
    expect(onSelect).not.toHaveBeenCalled();
    // The listbox still renders the remaining row (roving fell back to it).
    expect(within(screen.getByRole('listbox')).getByText('Beta (KT-2)')).toBeInTheDocument();
  });
});
