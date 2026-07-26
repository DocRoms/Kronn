import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent, ReactNode } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import type { PlanningDiscussionRelation } from '../types/generated';
import './PlanAllTasksView.css';

export interface PlanAllTasksViewProps {
  /** Active-plan relations, already in plan order (from the backend). */
  active: PlanningDiscussionRelation[];
  /** Later-plan relations, already in plan order. */
  later: PlanningDiscussionRelation[];
  /** Search text. Parent-owned so it survives a Focus↔All toggle. */
  query: string;
  onQueryChange: (next: string) => void;
  /** The currently selected task, if any — reflected as `aria-selected`. */
  selectedTaskId: string | null;
  /** Fired when a row is activated (click / Enter / Space). */
  onSelect: (taskId: string) => void;
  /**
   * Presentational row content ONLY — no interactive children. Each row is a
   * `role="option"` owned by this view (keyboard selection); actions live in
   * the parent's detail panel, never nested inside an option.
   */
  renderTask: (
    relation: PlanningDiscussionRelation,
    ctx: { selected: boolean },
  ) => ReactNode;
  /** Localised label lookup (injected so this view never touches i18n.ts). */
  t: (key: string, ...args: (string | number)[]) => string;
}

type FlatRow =
  | { kind: 'header'; group: 'active' | 'later'; label: string }
  | { kind: 'row'; group: 'active' | 'later'; relation: PlanningDiscussionRelation };

function matchesQuery(relation: PlanningDiscussionRelation, query: string): boolean {
  const { task } = relation;
  const haystack = [
    task.title,
    task.reference,
    task.parent_reference ?? '',
    task.parent_title ?? '',
  ]
    .join(' ')
    .toLowerCase();
  return haystack.includes(query);
}

const optionId = (taskId: string) => `plan-all-option-${taskId}`;

/**
 * KT-30C (C1) — the searchable, virtualised "all tasks" list. A dumb,
 * self-contained view: it owns virtualization, local search, the Active/Later
 * grouping and the listbox a11y (roving `aria-activedescendant`, headers never
 * focusable). Row VISUALS come from `renderTask`; selection + the dependency
 * neighbourhood live in the parent. DOM stays bounded at 200/1000 rows.
 */
export function PlanAllTasksView({
  active,
  later,
  query,
  onQueryChange,
  selectedTaskId,
  onSelect,
  renderTask,
  t,
}: PlanAllTasksViewProps) {
  const normalisedQuery = query.trim().toLowerCase();

  // Flatten to a single virtualisable list: a group header precedes its rows,
  // and a group with no match (after search) is dropped entirely.
  const flatRows = useMemo<FlatRow[]>(() => {
    const out: FlatRow[] = [];
    const pushGroup = (
      group: 'active' | 'later',
      relations: PlanningDiscussionRelation[],
      label: string,
    ) => {
      const kept = normalisedQuery
        ? relations.filter((relation) => matchesQuery(relation, normalisedQuery))
        : relations;
      if (kept.length === 0) return;
      out.push({ kind: 'header', group, label });
      for (const relation of kept) out.push({ kind: 'row', group, relation });
    };
    pushGroup('active', active, t('planning.allGroupActive'));
    pushGroup('later', later, t('planning.allGroupLater'));
    return out;
  }, [active, later, normalisedQuery, t]);

  // Positions of the SELECTABLE rows in `flatRows` — headers are skipped by
  // keyboard navigation and are never `option`s.
  const selectablePositions = useMemo(
    () =>
      flatRows.reduce<number[]>((positions, row, index) => {
        if (row.kind === 'row') positions.push(index);
        return positions;
      }, []),
    [flatRows],
  );

  const scrollRef = useRef<HTMLDivElement>(null);
  // `useVirtualizer` is a third-party hook the React 19 compiler plugin cannot
  // analyse (cf. TD-20260509-react19-effect-rules); the warning is a known
  // false positive for TanStack hooks, not a rules-of-hooks violation.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: flatRows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 60,
    overscan: 8,
  });

  // The selectable rows' task ids, in visible order (headers excluded).
  const selectableTaskIds = useMemo(
    () =>
      selectablePositions.map(
        (pos) => (flatRows[pos] as Extract<FlatRow, { kind: 'row' }>).relation.task.id,
      ),
    [flatRows, selectablePositions],
  );

  // Roving pointer tracked BY IDENTITY, not index: if the active task leaves
  // the visible set — even when the result COUNT is unchanged — fall back to
  // the first visible option. Never touches the parent-owned `selectedTaskId`
  // (the detail panel may stay open).
  const [activeTaskId, setActiveTaskId] = useState<string | null>(null);
  useEffect(() => {
    setActiveTaskId((prev) =>
      prev && selectableTaskIds.includes(prev) ? prev : (selectableTaskIds[0] ?? null),
    );
  }, [selectableTaskIds]);

  const activeIndex = activeTaskId ? selectableTaskIds.indexOf(activeTaskId) : -1;

  const moveActive = useCallback(
    (next: number) => {
      if (selectableTaskIds.length === 0) return;
      const clamped = Math.max(0, Math.min(next, selectableTaskIds.length - 1));
      setActiveTaskId(selectableTaskIds[clamped]);
      virtualizer.scrollToIndex(selectablePositions[clamped], { align: 'auto' });
    },
    [selectablePositions, selectableTaskIds, virtualizer],
  );

  const onKeyDown = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      switch (event.key) {
        case 'ArrowDown':
          event.preventDefault();
          moveActive(activeIndex + 1);
          break;
        case 'ArrowUp':
          event.preventDefault();
          moveActive(activeIndex - 1);
          break;
        case 'Home':
          event.preventDefault();
          moveActive(0);
          break;
        case 'End':
          event.preventDefault();
          moveActive(selectableTaskIds.length - 1);
          break;
        case 'Enter':
        case ' ':
          if (activeTaskId) {
            event.preventDefault();
            onSelect(activeTaskId);
          }
          break;
        default:
          break;
      }
    },
    [activeIndex, activeTaskId, moveActive, onSelect, selectableTaskIds.length],
  );

  const virtualItems = virtualizer.getVirtualItems();

  return (
    <div className="plan-all">
      <input
        type="search"
        className="plan-all-search"
        value={query}
        placeholder={t('planning.allSearchPlaceholder')}
        aria-label={t('planning.allSearchPlaceholder')}
        onChange={(event) => onQueryChange(event.target.value)}
      />
      {selectablePositions.length === 0 ? (
        <p className="plan-all-empty">{t('planning.allEmpty')}</p>
      ) : (
        <div
          ref={scrollRef}
          className="plan-all-scroll"
          role="listbox"
          tabIndex={0}
          aria-label={t('planning.allView')}
          aria-activedescendant={activeTaskId ? optionId(activeTaskId) : undefined}
          onKeyDown={onKeyDown}
        >
          <div
            className="plan-all-sizer"
            style={{ height: `${virtualizer.getTotalSize()}px` }}
          >
            {virtualItems.map((virtualItem) => {
              const row = flatRows[virtualItem.index];
              // `key` must be passed to the JSX element directly, never via a
              // spread (React warns otherwise) — so it stays out of `common`.
              const common = {
                'data-index': virtualItem.index,
                ref: virtualizer.measureElement,
                style: {
                  position: 'absolute' as const,
                  top: 0,
                  left: 0,
                  width: '100%',
                  transform: `translateY(${virtualItem.start}px)`,
                },
              };
              if (row.kind === 'header') {
                return (
                  <div
                    key={virtualItem.key}
                    {...common}
                    role="presentation"
                    className="plan-all-group"
                  >
                    {row.label}
                  </div>
                );
              }
              const taskId = row.relation.task.id;
              const selected = taskId === selectedTaskId;
              return (
                <div
                  key={virtualItem.key}
                  {...common}
                  role="option"
                  id={optionId(taskId)}
                  aria-selected={selected}
                  className={`plan-all-row${selected ? ' is-selected' : ''}`}
                  onClick={() => {
                    // Sync the roving pointer to the clicked row so a following
                    // Enter/Arrow starts from here, not the previous active row.
                    setActiveTaskId(taskId);
                    onSelect(taskId);
                  }}
                >
                  {renderTask(row.relation, { selected })}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
