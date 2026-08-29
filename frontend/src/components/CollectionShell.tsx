import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { ChevronLeft, ChevronRight, ListChecks, Loader2, Menu, MoreHorizontal, Search, Star, X } from 'lucide-react';
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
  icon?: React.ReactNode;
  danger?: boolean;
  onSelect: (items: TItem[]) => void | Promise<void>;
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

/**
 * Keeps transient bulk selection safe when a collection refreshes or deletes
 * an item for consumers that opt into the selectedIds/onSelectedIdsChange
 * contract, so those consumers cannot send an invisible/deleted item to a
 * bulk action.
 */
function pruneCollectionSelection(
  selectedIds: ReadonlySet<CollectionItemId>,
  availableIds: ReadonlySet<CollectionItemId>,
): Set<CollectionItemId> {
  return new Set([...selectedIds].filter(id => availableIds.has(id)));
}

function sameCollectionSelection(
  left: ReadonlySet<CollectionItemId>,
  right: ReadonlySet<CollectionItemId>,
): boolean {
  return left.size === right.size && [...left].every(id => right.has(id));
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
  /** Enables the canonical Discussions-style sidebar title row. */
  title?: React.ReactNode;
  titleCount?: number;
  /** Domain actions such as create/import that remain visible outside bulk mode. */
  headerActions?: React.ReactNode;
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
    selectMultiple?: string;
    cancelSelection?: string;
    selectedCount?: (count: number) => string;
  };
}

interface CollectionSidebarCollapseButtonProps {
  isMobile?: boolean;
  label: string;
  onCollapse: () => void;
  focusOnMount?: boolean;
}

/** The single close/collapse control used by every collection sidebar. */
export function CollectionSidebarCollapseButton({
  isMobile = false,
  label,
  onCollapse,
  focusOnMount = false,
}: CollectionSidebarCollapseButtonProps) {
  return <button
    type="button"
    className="collection-shell-collapse-button"
    onClick={onCollapse}
    aria-label={label}
    title={label}
    autoFocus={focusOnMount}
  >
    {isMobile ? <X size={16} /> : <ChevronLeft size={16} />}
  </button>;
}

interface CollectionSidebarRailProps {
  label: string;
  onOpen: () => void;
  className?: string;
  focusOnMount?: boolean;
}

/** Discussions-style full-height desktop rail, kept keyboard accessible. */
export function CollectionSidebarRail({
  label,
  onOpen,
  className = '',
  focusOnMount = false,
}: CollectionSidebarRailProps) {
  return <button
    type="button"
    className={`collection-shell-sidebar-rail ${className}`.trim()}
    onClick={onOpen}
    aria-label={label}
    title={label}
    autoFocus={focusOnMount}
  >
    <ChevronRight size={16} />
  </button>;
}

/**
 * Typed, domain-neutral master/detail primitive. Callers provide their item
 * rendering and persistence policy while this component owns the shared
 * filtering, selection, responsive sidebar and context-menu behaviour.
 */
export function CollectionShell<TItem>({
  ariaLabel, items, getId, getLabel, isFavorite, onToggleFavorite, filters = [], itemFilter, filterQuery = true,
  persistence, selectedId, onSelect, selectedIds, onSelectedIdsChange, actions = [],
  title, titleCount, headerActions, slots, isMobile = false, sidebarOnly = false, sidebarClassName = '', globalSearchShortcut = false, searchInputRef, showSearchClear = false, shortcutsEnabled = true, showControls = true, sidebarOpen = true, onSidebarOpenChange, onSearchSubmit, labels,
}: CollectionShellProps<TItem>) {
  const internalSearchRef = useRef<HTMLInputElement>(null);
  const searchRef = searchInputRef ?? internalSearchRef;
  const menuRef = useRef<HTMLDivElement>(null);
  const menuTriggerRef = useRef<HTMLButtonElement>(null);
  const menuId = useId();
  const [menuOpen, setMenuOpen] = useState(false);
  const [selectionMode, setSelectionMode] = useState(false);
  const [actionBusy, setActionBusy] = useState(false);
  const [focusRailOnMount, setFocusRailOnMount] = useState(false);
  const [focusCollapseOnMount, setFocusCollapseOnMount] = useState(false);
  const canMultiSelect = selectedIds != null && onSelectedIdsChange != null;
  const managedSelection = title != null && canMultiSelect;
  const multiSelectionVisible = canMultiSelect && (!managedSelection || selectionMode);
  const activeFilter = filters.find(filter => filter.id === persistence.activeFilterId);

  useEffect(() => {
    if (!selectedIds || !onSelectedIdsChange) return;
    const availableIds = new Set(items.map(getId));
    const next = pruneCollectionSelection(selectedIds, availableIds);
    if (!sameCollectionSelection(selectedIds, next)) onSelectedIdsChange(next);
  }, [getId, items, onSelectedIdsChange, selectedIds]);

  const closeMenu = useCallback((restoreTriggerFocus = false) => {
    setMenuOpen(false);
    if (restoreTriggerFocus) requestAnimationFrame(() => menuTriggerRef.current?.focus());
  }, []);
  const focusSearch = useCallback(() => {
    searchRef.current?.focus();
  }, [searchRef]);
  const collapseSidebar = useCallback(() => {
    setFocusRailOnMount(true);
    setFocusCollapseOnMount(false);
    onSidebarOpenChange?.(false);
  }, [onSidebarOpenChange]);
  const openSidebar = useCallback(() => {
    setFocusRailOnMount(false);
    setFocusCollapseOnMount(true);
    onSidebarOpenChange?.(true);
  }, [onSidebarOpenChange]);

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
    if (!multiSelectionVisible || (selectedIds?.size ?? 0) === 0) return selectedId == null
      ? []
      : items.filter(item => getId(item) === selectedId);
    return items.filter(item => selectedIds?.has(getId(item)));
  }, [getId, items, multiSelectionVisible, selectedId, selectedIds]);

  const leaveSelectionMode = useCallback(() => {
    setSelectionMode(false);
    onSelectedIdsChange?.(new Set());
  }, [onSelectedIdsChange]);

  const runAction = useCallback(async (action: CollectionAction<TItem>) => {
    if (actionBusy || action.disabled?.(actionItems)) return;
    setActionBusy(true);
    try {
      await action.onSelect(actionItems);
      if (managedSelection) leaveSelectionMode();
    } catch {
      // Consumers own domain error reporting. Keep selection active so the
      // user can retry instead of also leaking an unhandled promise.
    } finally {
      setActionBusy(false);
    }
  }, [actionBusy, actionItems, leaveSelectionMode, managedSelection]);

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

  useEffect(() => {
    if (!isMobile || !sidebarOpen) return;
    const closeMobileSidebar = (event: KeyboardEvent) => {
      // A menu owns Escape while it is open so it can restore focus to its
      // trigger before the sidebar itself is dismissed.
      if (event.key === 'Escape' && !menuOpen) onSidebarOpenChange?.(false);
    };
    window.addEventListener('keydown', closeMobileSidebar);
    return () => window.removeEventListener('keydown', closeMobileSidebar);
  }, [isMobile, menuOpen, onSidebarOpenChange, sidebarOpen]);

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

  const onMenuKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    const items = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'))
      .filter(item => !item.disabled);
    if (items.length === 0) return;
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === 'Home' ? 0
      : event.key === 'End' ? items.length - 1
        : current < 0 ? (event.key === 'ArrowDown' ? 0 : items.length - 1)
          : (current + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
    event.preventDefault();
    items[next]?.focus();
  };

  const listContext: CollectionListContext<TItem> = {
    visibleItems,
    selectItem,
    isSelected: item => selectedId === getId(item),
    canMultiSelect: multiSelectionVisible,
    isMultiSelected: item => multiSelectionVisible && (selectedIds?.has(getId(item)) ?? false),
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
      {title != null && <div className="collection-shell-titlebar" data-selection-mode={selectionMode}>
        <strong className="collection-shell-title">
          {selectionMode
            ? (labels.selectedCount?.(selectedIds?.size ?? 0) ?? `${selectedIds?.size ?? 0} ${labels.selectItem}`)
            : <>{title}{titleCount != null && <span className="collection-shell-title-count"> · {titleCount}</span>}</>}
        </strong>
        <div className="collection-shell-title-actions">
          {selectionMode ? <>
            {actions.map(action => <button
              key={action.id}
              type="button"
              className="collection-shell-icon"
              data-danger={action.danger || undefined}
              disabled={actionBusy || action.disabled?.(actionItems)}
              onClick={() => void runAction(action)}
              aria-label={action.label}
              title={action.label}
            >{actionBusy ? <Loader2 size={15} className="spin" /> : action.icon ?? action.label}</button>)}
            <button type="button" className="collection-shell-icon" disabled={actionBusy} onClick={leaveSelectionMode} aria-label={labels.cancelSelection ?? labels.closeCollection} title={labels.cancelSelection ?? labels.closeCollection}><X size={16} /></button>
          </> : <>
            {headerActions}
            {canMultiSelect && <div className="collection-shell-title-menu" ref={menuRef}>
              <button ref={menuTriggerRef} type="button" className="collection-shell-icon" onClick={() => setMenuOpen(open => !open)} aria-label={labels.moreActions} aria-haspopup="menu" aria-expanded={menuOpen} aria-controls={menuOpen ? menuId : undefined}><MoreHorizontal size={17} /></button>
              {menuOpen && <div id={menuId} className="collection-shell-menu" role="menu" aria-label={labels.moreActions} onKeyDown={onMenuKeyDown}>
                <button type="button" role="menuitem" onClick={() => { setSelectionMode(true); closeMenu(false); }}><ListChecks size={14} />{labels.selectMultiple ?? labels.selectItem}</button>
              </div>}
            </div>}
            {onSidebarOpenChange && <CollectionSidebarCollapseButton
              isMobile={isMobile}
              label={labels.closeCollection}
              onCollapse={collapseSidebar}
              focusOnMount={focusCollapseOnMount}
            />}
          </>}
        </div>
      </div>}
      {slots.beforeSidebarHeader}
      {slots.renderSearch ? slots.renderSearch(searchContext) : <header className="collection-shell-header">
        <div className="collection-shell-search">
          <Search size={13} className="collection-shell-search-icon" aria-hidden="true" />
          <input className="collection-shell-search-input" ref={searchRef} value={persistence.query} onChange={event => persistence.onQueryChange(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') onSearchSubmit?.(); }} aria-label={labels.search} aria-keyshortcuts="/" placeholder={labels.search} />
          {showSearchClear && persistence.query && <button type="button" className="collection-shell-search-clear" onClick={() => persistence.onQueryChange('')} aria-label={labels.clearFilters} title={labels.clearFilters}><X size={10} /></button>}
        </div>
        {(slots.sidebarHeaderEnd || (title == null && isMobile)) && <div className="collection-shell-search-actions">
          {slots.sidebarHeaderEnd}
          {title == null && isMobile && <button type="button" className="collection-shell-icon" onClick={() => onSidebarOpenChange?.(false)} aria-label={labels.closeCollection}><X size={17} /></button>}
        </div>}
      </header>}
      {slots.afterSidebarHeader}
      {showControls && <div className="collection-shell-controls">
        {isFavorite && <button type="button" className="collection-shell-filter" data-active={persistence.favoritesOnly} aria-pressed={persistence.favoritesOnly} onClick={() => persistence.onFavoritesOnlyChange(!persistence.favoritesOnly)}><Star size={14} />{labels.favorites}</button>}
        {filters.map(filter => <button key={filter.id} type="button" className="collection-shell-filter" data-active={filter.id === persistence.activeFilterId} aria-pressed={filter.id === persistence.activeFilterId} onClick={() => persistence.onActiveFilterIdChange?.(filter.id === persistence.activeFilterId ? null : filter.id)}>{filter.label}</button>)}
        {((!showSearchClear && persistence.query) || persistence.favoritesOnly || persistence.activeFilterId) && <button type="button" className="collection-shell-clear" onClick={() => { persistence.onQueryChange(''); persistence.onFavoritesOnlyChange(false); persistence.onActiveFilterIdChange?.(null); }}>{labels.clearFilters}</button>}
      </div>}
      {slots.renderList ? slots.renderList(listContext) : <ul className="collection-shell-list">
          {visibleItems.map(item => {
            const id = getId(item);
            const multiSelected = selectedIds?.has(id) ?? false;
            const selected = selectedId === id;
            return <li key={id} className="collection-shell-row" data-selected={selected} data-multi-selected={multiSelected}>
              {multiSelectionVisible && <input type="checkbox" aria-label={`${getLabel(item)} ${labels.selectItem}`} checked={multiSelected} onChange={() => toggleMultiSelection(id)} />}
              <button type="button" className="collection-shell-row-button" aria-current={selected ? 'true' : undefined} onClick={() => selectItem(item)}>{slots.renderItem?.(item, { selected, multiSelected }) ?? getLabel(item)}</button>
              {isFavorite && onToggleFavorite && <button type="button" className="collection-shell-favorite" aria-label={`${labels.favorites} · ${getLabel(item)}`} aria-pressed={isFavorite(item)} onClick={() => onToggleFavorite(item)}><Star size={14} fill={isFavorite(item) ? 'currentColor' : 'none'} /></button>}
            </li>;
          })}
          {visibleItems.length === 0 && <li className="collection-shell-empty">{slots.renderEmpty?.()}</li>}
        </ul>}
      {slots.sidebarFooter}
    </aside>
  );

  if (sidebarOnly) return sidebarOpen
    ? sidebar
    : <CollectionSidebarRail label={labels.openCollection} onOpen={openSidebar} focusOnMount={focusRailOnMount} />;

  return <section className="collection-shell" data-mobile={isMobile}>
    {sidebarOpen
      ? sidebar
      : !isMobile && <CollectionSidebarRail label={labels.openCollection} onOpen={openSidebar} focusOnMount={focusRailOnMount} />}
    <div className="collection-shell-detail">
      {!sidebarOpen && isMobile && <button type="button" className="collection-shell-open" onClick={openSidebar} aria-label={labels.openCollection}><Menu size={18} /></button>}
      {title == null && actions.length > 0 && <div className="collection-shell-actions" ref={menuRef}>
        <button ref={menuTriggerRef} type="button" className="collection-shell-icon" onClick={() => setMenuOpen(open => !open)} aria-label={labels.moreActions} aria-haspopup="menu" aria-expanded={menuOpen} aria-controls={menuOpen ? menuId : undefined}><MoreHorizontal size={17} /></button>
        {menuOpen && <div id={menuId} className="collection-shell-menu" role="menu" aria-label={labels.moreActions} onKeyDown={onMenuKeyDown}>{actions.map(action => <button key={action.id} type="button" role="menuitem" disabled={action.disabled?.(actionItems)} onClick={() => { void runAction(action); closeMenu(true); }}>{action.label}</button>)}</div>}
      </div>}
      {slots.renderDetail(selectedId == null ? null : items.find(item => getId(item) === selectedId) ?? null)}
    </div>
  </section>;
}
