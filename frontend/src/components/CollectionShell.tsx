import { useEffect, useMemo, useRef, useState } from 'react';
import { Menu, Search, Star, X } from 'lucide-react';
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

export interface CollectionShellSlots<TItem> {
  renderItem?: (item: TItem, state: { selected: boolean; multiSelected: boolean }) => React.ReactNode;
  renderDetail: (item: TItem | null) => React.ReactNode;
  renderEmpty?: () => React.ReactNode;
  sidebarHeaderEnd?: React.ReactNode;
}

export interface CollectionShellProps<TItem> {
  ariaLabel: string;
  items: TItem[];
  getId: (item: TItem) => CollectionItemId;
  getLabel: (item: TItem) => string;
  isFavorite?: (item: TItem) => boolean;
  onToggleFavorite?: (item: TItem) => void;
  filters?: CollectionFilter<TItem>[];
  persistence: CollectionPersistence;
  selectedId: CollectionItemId | null;
  onSelect: (id: CollectionItemId) => void;
  selectedIds?: ReadonlySet<CollectionItemId>;
  onSelectedIdsChange?: (ids: Set<CollectionItemId>) => void;
  actions?: CollectionAction<TItem>[];
  slots: CollectionShellSlots<TItem>;
  isMobile?: boolean;
  sidebarOpen?: boolean;
  onSidebarOpenChange?: (open: boolean) => void;
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
  ariaLabel, items, getId, getLabel, isFavorite, onToggleFavorite, filters = [],
  persistence, selectedId, onSelect, selectedIds, onSelectedIdsChange, actions = [],
  slots, isMobile = false, sidebarOpen = true, onSidebarOpenChange, labels,
}: CollectionShellProps<TItem>) {
  const searchRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuTriggerRef = useRef<HTMLButtonElement>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const canMultiSelect = selectedIds != null && onSelectedIdsChange != null;
  const activeFilter = filters.find(filter => filter.id === persistence.activeFilterId);

  const visibleItems = useMemo(() => {
    const query = persistence.query.trim().toLocaleLowerCase();
    return items.filter(item => {
      if (persistence.favoritesOnly && !isFavorite?.(item)) return false;
      if (activeFilter && !activeFilter.matches(item)) return false;
      return !query || getLabel(item).toLocaleLowerCase().includes(query);
    });
  }, [activeFilter, getLabel, isFavorite, items, persistence.favoritesOnly, persistence.query]);

  const actionItems = useMemo(() => {
    if (!canMultiSelect || (selectedIds?.size ?? 0) === 0) return selectedId == null
      ? []
      : items.filter(item => getId(item) === selectedId);
    return items.filter(item => selectedIds?.has(getId(item)));
  }, [canMultiSelect, getId, items, selectedId, selectedIds]);

  useEffect(() => {
    if (!menuOpen) return;
    menuRef.current?.querySelector<HTMLButtonElement>('button:not([disabled])')?.focus();
    const close = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    const closeFromKeyboard = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      setMenuOpen(false);
      requestAnimationFrame(() => menuTriggerRef.current?.focus());
    };
    window.addEventListener('pointerdown', close);
    window.addEventListener('keydown', closeFromKeyboard);
    return () => {
      window.removeEventListener('pointerdown', close);
      window.removeEventListener('keydown', closeFromKeyboard);
    };
  }, [menuOpen]);

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
    const target = event.target as HTMLElement;
    const textControl = target.matches('input, textarea, select, [contenteditable="true"]');
    if (event.key === '/' && !textControl) {
      event.preventDefault();
      searchRef.current?.focus();
      return;
    }
    if (textControl || (event.key !== 'ArrowDown' && event.key !== 'ArrowUp')) return;
    const rows = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('.collection-shell-row-button'));
    if (rows.length === 0) return;
    const current = rows.indexOf(document.activeElement as HTMLButtonElement);
    const direction = event.key === 'ArrowDown' ? 1 : -1;
    const next = current < 0 ? (direction > 0 ? 0 : rows.length - 1) : (current + direction + rows.length) % rows.length;
    event.preventDefault();
    rows[next]?.focus();
  };

  const sidebar = (
    <aside className="collection-shell-sidebar" data-mobile={isMobile} aria-label={ariaLabel} onKeyDown={onSidebarKeyDown}>
      <header className="collection-shell-header">
        <div className="collection-shell-search">
          <Search size={15} aria-hidden="true" />
          <input ref={searchRef} value={persistence.query} onChange={event => persistence.onQueryChange(event.target.value)} aria-label={labels.search} placeholder={labels.search} />
        </div>
        {isMobile && <button type="button" className="collection-shell-icon" onClick={() => onSidebarOpenChange?.(false)} aria-label={labels.closeCollection}><X size={17} /></button>}
        {slots.sidebarHeaderEnd}
      </header>
      <div className="collection-shell-controls">
        {isFavorite && <button type="button" className="collection-shell-filter" data-active={persistence.favoritesOnly} aria-pressed={persistence.favoritesOnly} onClick={() => persistence.onFavoritesOnlyChange(!persistence.favoritesOnly)}><Star size={14} />{labels.favorites}</button>}
        {filters.map(filter => <button key={filter.id} type="button" className="collection-shell-filter" data-active={filter.id === persistence.activeFilterId} aria-pressed={filter.id === persistence.activeFilterId} onClick={() => persistence.onActiveFilterIdChange?.(filter.id === persistence.activeFilterId ? null : filter.id)}>{filter.label}</button>)}
        {(persistence.query || persistence.favoritesOnly || persistence.activeFilterId) && <button type="button" className="collection-shell-clear" onClick={() => { persistence.onQueryChange(''); persistence.onFavoritesOnlyChange(false); persistence.onActiveFilterIdChange?.(null); }}>{labels.clearFilters}</button>}
      </div>
      <div className="collection-shell-list" role="list">
        {visibleItems.map(item => {
          const id = getId(item);
          const multiSelected = selectedIds?.has(id) ?? false;
          const selected = selectedId === id;
          return <div key={id} className="collection-shell-row" data-selected={selected} data-multi-selected={multiSelected} role="listitem">
            {canMultiSelect && <input type="checkbox" aria-label={`${getLabel(item)} ${labels.selectItem}`} checked={multiSelected} onChange={() => toggleMultiSelection(id)} />}
            <button type="button" className="collection-shell-row-button" onClick={() => selectItem(item)}>{slots.renderItem?.(item, { selected, multiSelected }) ?? getLabel(item)}</button>
            {isFavorite && onToggleFavorite && <button type="button" className="collection-shell-favorite" aria-label={`${labels.favorites} · ${getLabel(item)}`} aria-pressed={isFavorite(item)} onClick={() => onToggleFavorite(item)}><Star size={14} fill={isFavorite(item) ? 'currentColor' : 'none'} /></button>}
          </div>;
        })}
        {visibleItems.length === 0 && <div className="collection-shell-empty">{slots.renderEmpty?.()}</div>}
      </div>
    </aside>
  );

  return <section className="collection-shell" data-mobile={isMobile}>
    {(!isMobile || sidebarOpen) && sidebar}
    <div className="collection-shell-detail">
      {isMobile && !sidebarOpen && <button type="button" className="collection-shell-open" onClick={() => onSidebarOpenChange?.(true)} aria-label={labels.openCollection}><Menu size={18} /></button>}
      {actions.length > 0 && <div className="collection-shell-actions" ref={menuRef}>
        <button ref={menuTriggerRef} type="button" className="collection-shell-icon" onClick={() => setMenuOpen(open => !open)} aria-label={labels.moreActions} aria-expanded={menuOpen}><Menu size={17} /></button>
        {menuOpen && <div className="collection-shell-menu" role="menu" aria-label={labels.moreActions}>{actions.map(action => <button key={action.id} type="button" role="menuitem" disabled={action.disabled?.(actionItems)} onClick={() => { action.onSelect(actionItems); setMenuOpen(false); }}>{action.label}</button>)}</div>}
      </div>}
      {slots.renderDetail(selectedId == null ? null : items.find(item => getId(item) === selectedId) ?? null)}
    </div>
  </section>;
}
