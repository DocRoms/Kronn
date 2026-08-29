import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Menu, MoreHorizontal, Search, Star, X } from 'lucide-react';
import './CollectionShell.css';

export type CollectionItemId = string;

export interface CollectionFilter<TItem> {
  id: string;
  label: string;
  matches: (item: TItem) => boolean;
}

/** State owned by the surface embedding a collection shell. This keeps URL or
 * local-storage persistence out of the shared primitive. */
export interface CollectionPersistence {
  query: string;
  onQueryChange: (query: string) => void;
  activeFilterId?: string | null;
  onActiveFilterIdChange?: (filterId: string | null) => void;
  favoritesOnly: boolean;
  onFavoritesOnlyChange: (favoritesOnly: boolean) => void;
}

export interface CollectionAction<TItem> {
  id: string;
  label: string;
  onSelect: (items: TItem[]) => void;
  disabled?: (items: TItem[]) => boolean;
}

/** Props for a custom row's native focusable control. The caller owns any
 * list-item wrapper; the shell deliberately does not override the control's
 * native button role. */
export interface CollectionRowProps {
  className: string;
  'aria-current'?: 'true';
  onClick: () => void;
}

export interface CollectionSearchContext {
  value: string;
  inputRef: React.RefObject<HTMLInputElement | null>;
  onChange: (query: string) => void;
  onSubmit?: () => void;
  clear: () => void;
}

/** Context handed to `renderList` — the already-filtered items plus the
 * selection helpers a caller needs to keep custom (grouped, nested) markup
 * wired to the shell's shared query/favorites/selection state. */
export interface CollectionListContext<TItem> {
  visibleItems: TItem[];
  selectItem: (item: TItem) => void;
  isSelected: (item: TItem) => boolean;
  canMultiSelect: boolean;
  isMultiSelected: (item: TItem) => boolean;
  toggleMultiSelection: (id: CollectionItemId) => void;
  /** Props for the one focusable control representing an item in a custom
   * list. Spread these onto the row button so grouped/nested renderers keep
   * the shell's activation, current-item state, and roving keyboard target. */
  getRowProps: (item: TItem) => CollectionRowProps;
}

export interface CollectionShellSlots<TItem> {
  renderItem?: (item: TItem, state: { selected: boolean; multiSelected: boolean }) => React.ReactNode;
  renderDetail: (item: TItem | null) => React.ReactNode;
  renderEmpty?: () => React.ReactNode;
  sidebarHeaderEnd?: React.ReactNode;
  /** Rendered above the shared search/controls row — lets a domain page keep
   * its own title/count/bulk-toolbar without a local shell reimplementation. */
  beforeSidebarHeader?: React.ReactNode;
  /** Rendered immediately after the shared search row. Useful for a domain
   * selector that must retain its own full-width layout. */
  afterSidebarHeader?: React.ReactNode;
  /** Replaces the standard compact search field while retaining the shell's
   * query state, focus shortcut and optional submit behaviour. */
  renderSearch?: (context: CollectionSearchContext) => React.ReactNode;
  /** Full override of the list body. Receives the already query/favorites/
   * filter-narrowed items so a caller can render grouped or nested markup
   * (project trees, resource kinds…) while still sharing the filtering and
   * selection logic owned by this component. Falls back to the default flat
   * row rendering when absent. */
  renderList?: (context: CollectionListContext<TItem>) => React.ReactNode;
  /** Rendered below the list, inside the sidebar (hints, shortcuts…). */
  sidebarFooter?: React.ReactNode;
}

export interface CollectionShellProps<TItem> {
  ariaLabel: string;
  items: TItem[];
  getId: (item: TItem) => CollectionItemId;
  getLabel: (item: TItem) => string;
  isFavorite?: (item: TItem) => boolean;
  onToggleFavorite?: (item: TItem) => void;
  filters?: CollectionFilter<TItem>[];
  /** A domain-owned filter whose control is rendered in a slot. */
  itemFilter?: (item: TItem) => boolean;
  /** Set false when the query is submitted to a remote search instead of
   * narrowing the local collection on every keystroke. */
  filterQuery?: boolean;
  persistence: CollectionPersistence;
  selectedId: CollectionItemId | null;
  onSelect: (id: CollectionItemId) => void;
  selectedIds?: ReadonlySet<CollectionItemId>;
  onSelectedIdsChange?: (ids: Set<CollectionItemId>) => void;
  actions?: CollectionAction<TItem>[];
  slots: CollectionShellSlots<TItem>;
  isMobile?: boolean;
  /** For surfaces whose detail pane is already composed by the caller. */
  sidebarOnly?: boolean;
  /** Domain CSS hook for a sidebar that keeps pre-existing visual treatment. */
  sidebarClassName?: string;
  /** Enables the established page-wide `/` shortcut in addition to the
   * sidebar-local shortcut. */
  globalSearchShortcut?: boolean;
  /** Uses a domain-owned search field while retaining the shell's shortcut
   * handling (for example, a search that is submitted to the server). */
  searchInputRef?: React.RefObject<HTMLInputElement | null>;
  /** Shows a clear affordance that changes only the search query. */
  showSearchClear?: boolean;
  /** Disables slash/roving-row shortcuts while an alternate sidebar mode is
   * active. */
  shortcutsEnabled?: boolean;
  /** Hide the generic chip bar when a surface owns its filtering controls. */
  showControls?: boolean;
  sidebarOpen?: boolean;
  onSidebarOpenChange?: (open: boolean) => void;
  onSearchSubmit?: () => void;
  labels: {
    search: string;
    favorites: string;
    clearFilters: string;
    moreActions: string;
    openCollection: string;
    closeCollection: string;
    selectItem: string;
  };
}

/**
 * Typed, domain-neutral master/detail primitive. Callers provide their item
 * rendering and persistence policy while this component owns the shared
 * filtering, selection, responsive sidebar and context-menu behaviour.
 */
export function CollectionShell<TItem>({
  ariaLabel, items, getId, getLabel, isFavorite, onToggleFavorite, filters = [], itemFilter, filterQuery = true,
  persistence, selectedId, onSelect, selectedIds, onSelectedIdsChange, actions = [],
  slots, isMobile = false, sidebarOnly = false, sidebarClassName = '', globalSearchShortcut = false, searchInputRef, showSearchClear = false, shortcutsEnabled = true, showControls = true, sidebarOpen = true, onSidebarOpenChange, onSearchSubmit, labels,
}: CollectionShellProps<TItem>) {
  const internalSearchRef = useRef<HTMLInputElement>(null);
  const searchRef = searchInputRef ?? internalSearchRef;
  const menuRef = useRef<HTMLDivElement>(null);
  const menuTriggerRef = useRef<HTMLButtonElement>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const canMultiSelect = selectedIds != null && onSelectedIdsChange != null;
  const activeFilter = filters.find(filter => filter.id === persistence.activeFilterId);

  const closeMenu = useCallback((restoreTriggerFocus = false) => {
    setMenuOpen(false);
    if (restoreTriggerFocus) requestAnimationFrame(() => menuTriggerRef.current?.focus());
  }, []);
  const focusSearch = useCallback(() => {
    searchRef.current?.focus();
  }, [searchRef]);

  const visibleItems = useMemo(() => {
    const query = persistence.query.trim().toLocaleLowerCase();
    return items.filter(item => {
      if (itemFilter && !itemFilter(item)) return false;
      if (persistence.favoritesOnly && !isFavorite?.(item)) return false;
      if (activeFilter && !activeFilter.matches(item)) return false;
      return !filterQuery || !query || getLabel(item).toLocaleLowerCase().includes(query);
    });
  }, [activeFilter, filterQuery, getLabel, isFavorite, itemFilter, items, persistence.favoritesOnly, persistence.query]);

  const actionItems = useMemo(() => {
    if (!canMultiSelect || (selectedIds?.size ?? 0) === 0) return selectedId == null
      ? []
      : items.filter(item => getId(item) === selectedId);
    return items.filter(item => selectedIds?.has(getId(item)));
  }, [canMultiSelect, getId, items, selectedId, selectedIds]);

  useEffect(() => {
    if (!menuOpen) return;
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not([disabled])')?.focus();
    const close = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) closeMenu(true);
    };
    const closeFromKeyboard = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      closeMenu(true);
    };
    window.addEventListener('pointerdown', close);
    window.addEventListener('keydown', closeFromKeyboard);
    return () => {
      window.removeEventListener('pointerdown', close);
      window.removeEventListener('keydown', closeFromKeyboard);
    };
  }, [closeMenu, menuOpen]);

  useEffect(() => {
    if (!globalSearchShortcut || !shortcutsEnabled) return;
    const handleGlobalSearchShortcut = (event: KeyboardEvent) => {
      const target = event.target;
      if (event.key !== '/' || (target instanceof HTMLElement && target.matches('input, textarea, select, [contenteditable="true"]'))) return;
      event.preventDefault();
      focusSearch();
    };
    window.addEventListener('keydown', handleGlobalSearchShortcut);
    return () => window.removeEventListener('keydown', handleGlobalSearchShortcut);
  }, [focusSearch, globalSearchShortcut, shortcutsEnabled]);

  const selectItem = (item: TItem) => {
    onSelect(getId(item));
    if (isMobile) onSidebarOpenChange?.(false);
  };

  const toggleMultiSelection = (id: CollectionItemId) => {
    if (!selectedIds || !onSelectedIdsChange) return;
    const next = new Set(selectedIds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    onSelectedIdsChange(next);
  };

  const onSidebarKeyDown = (event: React.KeyboardEvent<HTMLElement>) => {
    if (!shortcutsEnabled) return;
    const target = event.target as HTMLElement;
    const textControl = target.matches('input, textarea, select, [contenteditable="true"]');
    if (event.key === '/' && !textControl) {
      event.preventDefault();
      searchRef.current?.focus();
      return;
    }
    if (textControl || !['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    const rows = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('.collection-shell-row-button'))
      .filter(row => !row.disabled && !row.closest('[hidden], [aria-hidden="true"]')
        && window.getComputedStyle(row).display !== 'none'
        && window.getComputedStyle(row).visibility !== 'hidden');
    if (rows.length === 0) return;
    const current = rows.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === 'Home' ? 0
      : event.key === 'End' ? rows.length - 1
        : current < 0 ? (event.key === 'ArrowDown' ? 0 : rows.length - 1)
          : (current + (event.key === 'ArrowDown' ? 1 : -1) + rows.length) % rows.length;
    event.preventDefault();
    rows[next]?.focus();
  };

  const listContext: CollectionListContext<TItem> = {
    visibleItems,
    selectItem,
    isSelected: item => selectedId === getId(item),
    canMultiSelect,
    isMultiSelected: item => canMultiSelect && (selectedIds?.has(getId(item)) ?? false),
    toggleMultiSelection,
    getRowProps: item => ({
      className: 'collection-shell-row-button',
      'aria-current': selectedId === getId(item) ? 'true' : undefined,
      onClick: () => selectItem(item),
    }),
  };

  const searchContext: CollectionSearchContext = {
    value: persistence.query,
    inputRef: searchRef,
    onChange: persistence.onQueryChange,
    onSubmit: onSearchSubmit,
    clear: () => persistence.onQueryChange(''),
  };

  const sidebar = (
    <aside className={`collection-shell-sidebar ${sidebarClassName}`.trim()} data-mobile={isMobile} aria-label={ariaLabel} onKeyDown={onSidebarKeyDown}>
      {slots.beforeSidebarHeader}
      {slots.renderSearch ? slots.renderSearch(searchContext) : <header className="collection-shell-header">
        <div className="collection-shell-search">
          <Search size={15} aria-hidden="true" />
          <input ref={searchRef} value={persistence.query} onChange={event => persistence.onQueryChange(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') onSearchSubmit?.(); }} aria-label={labels.search} placeholder={labels.search} />
          {showSearchClear && persistence.query && <button type="button" className="collection-shell-icon" onClick={() => persistence.onQueryChange('')} aria-label={labels.clearFilters}><X size={15} /></button>}
        </div>
        {isMobile && <button type="button" className="collection-shell-icon" onClick={() => onSidebarOpenChange?.(false)} aria-label={labels.closeCollection}><X size={17} /></button>}
        {slots.sidebarHeaderEnd}
      </header>}
      {slots.afterSidebarHeader}
      {showControls && <div className="collection-shell-controls">
        {isFavorite && <button type="button" className="collection-shell-filter" data-active={persistence.favoritesOnly} aria-pressed={persistence.favoritesOnly} onClick={() => persistence.onFavoritesOnlyChange(!persistence.favoritesOnly)}><Star size={14} />{labels.favorites}</button>}
        {filters.map(filter => <button key={filter.id} type="button" className="collection-shell-filter" data-active={filter.id === persistence.activeFilterId} aria-pressed={filter.id === persistence.activeFilterId} onClick={() => persistence.onActiveFilterIdChange?.(filter.id === persistence.activeFilterId ? null : filter.id)}>{filter.label}</button>)}
        {(persistence.query || persistence.favoritesOnly || persistence.activeFilterId) && <button type="button" className="collection-shell-clear" onClick={() => { persistence.onQueryChange(''); persistence.onFavoritesOnlyChange(false); persistence.onActiveFilterIdChange?.(null); }}>{labels.clearFilters}</button>}
      </div>}
      {slots.renderList ? slots.renderList(listContext) : <div className="collection-shell-list" role="list">
          {visibleItems.map(item => {
            const id = getId(item);
            const multiSelected = selectedIds?.has(id) ?? false;
            const selected = selectedId === id;
            return <div key={id} className="collection-shell-row" data-selected={selected} data-multi-selected={multiSelected} role="listitem">
              {canMultiSelect && <input type="checkbox" aria-label={`${getLabel(item)} ${labels.selectItem}`} checked={multiSelected} onChange={() => toggleMultiSelection(id)} />}
              <button type="button" className="collection-shell-row-button" aria-current={selected ? 'true' : undefined} onClick={() => selectItem(item)}>{slots.renderItem?.(item, { selected, multiSelected }) ?? getLabel(item)}</button>
              {isFavorite && onToggleFavorite && <button type="button" className="collection-shell-favorite" aria-label={`${labels.favorites} · ${getLabel(item)}`} aria-pressed={isFavorite(item)} onClick={() => onToggleFavorite(item)}><Star size={14} fill={isFavorite(item) ? 'currentColor' : 'none'} /></button>}
            </div>;
          })}
          {visibleItems.length === 0 && <div className="collection-shell-empty">{slots.renderEmpty?.()}</div>}
        </div>}
      {slots.sidebarFooter}
    </aside>
  );

  if (sidebarOnly) return sidebar;

  return <section className="collection-shell" data-mobile={isMobile}>
    {(!isMobile || sidebarOpen) && sidebar}
    <div className="collection-shell-detail">
      {isMobile && !sidebarOpen && <button type="button" className="collection-shell-open" onClick={() => onSidebarOpenChange?.(true)} aria-label={labels.openCollection}><Menu size={18} /></button>}
      {actions.length > 0 && <div className="collection-shell-actions" ref={menuRef}>
        <button ref={menuTriggerRef} type="button" className="collection-shell-icon" onClick={() => setMenuOpen(open => !open)} aria-label={labels.moreActions} aria-expanded={menuOpen}><MoreHorizontal size={17} /></button>
        {menuOpen && <div className="collection-shell-menu" role="menu" aria-label={labels.moreActions}>{actions.map(action => <button key={action.id} type="button" role="menuitem" disabled={action.disabled?.(actionItems)} onClick={() => { action.onSelect(actionItems); closeMenu(true); }}>{action.label}</button>)}</div>}
      </div>}
      {slots.renderDetail(selectedId == null ? null : items.find(item => getId(item) === selectedId) ?? null)}
    </div>
  </section>;
}
