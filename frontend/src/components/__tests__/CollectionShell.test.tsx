import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { useState } from 'react';
import { CollectionShell } from '../CollectionShell';
import { usePersistentIdSet } from '../../hooks/usePersistentIdSet';

type Item = { id: string; name: string; favorite: boolean; archived?: boolean };
const items: Item[] = [{ id: 'one', name: 'One', favorite: true }, { id: 'two', name: 'Two', favorite: false, archived: true }];
const labels = { search: 'Search', favorites: 'Favorites', clearFilters: 'Clear filters', moreActions: 'More actions', openCollection: 'Open collection', closeCollection: 'Close collection', selectItem: 'selected' };
const collectionShellCss = readFileSync(resolve(process.cwd(), 'src/components/CollectionShell.css'), 'utf8');

function Fixture({ mobile = false, ariaLabel = 'Test collection', persistentFavorites = false }: { mobile?: boolean; ariaLabel?: string; persistentFavorites?: boolean }) {
  const [query, setQuery] = useState('');
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [filter, setFilter] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>('one');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const { ids: persistedFavoriteIds, toggle: togglePersistedFavorite } = usePersistentIdSet(`kronn:test-collection-favorites:${ariaLabel}`, items.map(item => item.id), true);
  return <CollectionShell<Item>
    ariaLabel={ariaLabel} items={items} getId={item => item.id} getLabel={item => item.name}
    isFavorite={item => persistentFavorites ? persistedFavoriteIds.has(item.id) : item.favorite}
    onToggleFavorite={persistentFavorites ? item => togglePersistedFavorite(item.id) : undefined}
    filters={[{ id: 'archived', label: 'Archived', matches: item => !!item.archived }]}
    persistence={{ query, onQueryChange: setQuery, favoritesOnly, onFavoritesOnlyChange: setFavoritesOnly, activeFilterId: filter, onActiveFilterIdChange: setFilter }}
    selectedId={selectedId} onSelect={setSelectedId} selectedIds={selectedIds} onSelectedIdsChange={setSelectedIds}
    actions={[{ id: 'archive', label: 'Archive selected', onSelect: selected => setSelectedId(selected.map(item => item.id).join(',')) }]}
    isMobile={mobile} sidebarOpen={sidebarOpen} onSidebarOpenChange={setSidebarOpen} labels={labels}
    slots={{ renderDetail: item => <p>Detail: {item?.name ?? 'none'}</p>, renderEmpty: () => <>Nothing found</> }}
  />;
}

describe('CollectionShell', () => {
  it('defines the canonical separated gradient surface for right-pane headers', () => {
    const rule = collectionShellCss.match(/\.collection-detail-header\s*\{([^}]*)\}/)?.[1] ?? '';

    expect(rule).toContain('border-bottom: 1px solid var(--kr-border-light)');
    expect(rule).toContain('linear-gradient(135deg');
    expect(rule).toContain('var(--kr-accent) 7%');
    expect(rule).toContain('var(--kr-bg-card)');
  });

  it('supports the shared keyboard, multi-selection, persistence, and Escape contract', async () => {
    const ariaLabel = 'Test collection';
    render(<Fixture ariaLabel={ariaLabel} persistentFavorites />);

    const sidebar = screen.getByRole('complementary', { name: ariaLabel });
    fireEvent.keyDown(sidebar, { key: '/' });
    expect(screen.getByRole('textbox', { name: 'Search' })).toHaveFocus();
    fireEvent.keyDown(sidebar, { key: 'ArrowDown' });
    expect(screen.getByRole('button', { name: 'One' })).toHaveFocus();

    fireEvent.click(screen.getByRole('checkbox', { name: 'One selected' }));
    expect(screen.getByRole('checkbox', { name: 'One selected' })).toBeChecked();

    fireEvent.click(screen.getByRole('button', { name: 'Favorites · One' }));
    expect(localStorage.getItem(`kronn:test-collection-favorites:${ariaLabel}`)).toBe(JSON.stringify(['one']));
    fireEvent.click(screen.getByRole('button', { name: 'Favorites' }));
    expect(screen.queryByRole('button', { name: 'Two' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Clear filters' }));
    expect(screen.getByRole('button', { name: 'Two' })).toBeInTheDocument();

    const trigger = screen.getByRole('button', { name: 'More actions' });
    fireEvent.click(trigger);
    expect(screen.getByRole('menu', { name: 'More actions' })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('filters, favorites, and renders the shared empty state', () => {
    render(<Fixture />);
    expect(screen.getByRole('list')).toBeInTheDocument();
    expect(screen.getAllByRole('listitem')).toHaveLength(2);
    fireEvent.click(screen.getByRole('button', { name: 'Favorites' }));
    expect(screen.getByRole('button', { name: 'One' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Two' })).toBeNull();
    fireEvent.change(screen.getByRole('textbox', { name: 'Search' }), { target: { value: 'none' } });
    expect(screen.getByText('Nothing found')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Clear filters' }));
    expect(screen.getByRole('button', { name: 'Two' })).toBeInTheDocument();
  });

  it('supports multi-selection and restores menu-trigger focus after Escape', async () => {
    render(<Fixture />);
    fireEvent.click(screen.getByRole('checkbox', { name: 'One selected' }));
    fireEvent.click(screen.getByRole('checkbox', { name: 'Two selected' }));
    const trigger = screen.getByRole('button', { name: 'More actions' });
    fireEvent.click(trigger);
    expect(trigger).toHaveAttribute('aria-haspopup', 'menu');
    await waitFor(() => expect(screen.getByRole('menuitem', { name: 'Archive selected' })).toHaveFocus());
    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it('renders the canonical title row, enters bulk mode from the ellipsis, and collapses on desktop', async () => {
    function HeaderFixture() {
      const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
      const [sidebarOpen, setSidebarOpen] = useState(true);
      const [result, setResult] = useState('');
      return <>
        <output>{result}</output>
        <CollectionShell<Item>
          ariaLabel="Managed collection" title="Projects" titleCount={items.length}
          items={items} getId={item => item.id} getLabel={item => item.name}
          persistence={{ query: '', onQueryChange: () => {}, favoritesOnly: false, onFavoritesOnlyChange: () => {} }}
          selectedId="one" onSelect={() => {}} selectedIds={selectedIds} onSelectedIdsChange={setSelectedIds}
          actions={[{ id: 'delete', label: 'Delete selection', onSelect: selected => setResult(selected.map(item => item.id).join(',')) }]}
          sidebarOpen={sidebarOpen} onSidebarOpenChange={setSidebarOpen}
          labels={{ ...labels, selectMultiple: 'Select multiple', cancelSelection: 'Cancel selection', selectedCount: count => `${count} selected` }}
          slots={{ renderDetail: () => null }}
        />
      </>;
    }
    render(<HeaderFixture />);
    expect(screen.getByText('Projects')).toHaveTextContent('Projects · 2');
    fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Select multiple' }));
    fireEvent.click(screen.getByRole('checkbox', { name: 'One selected' }));
    fireEvent.click(screen.getByRole('checkbox', { name: 'Two selected' }));
    expect(screen.getByText('2 selected')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Delete selection' }));
    await waitFor(() => expect(screen.getByText('one,two', { selector: 'output' })).toBeInTheDocument());
    expect(screen.queryByRole('checkbox')).toBeNull();
    const collapse = screen.getByRole('button', { name: 'Close collection' });
    expect(collapse).toHaveClass('collection-shell-collapse-button');
    fireEvent.click(collapse);
    expect(screen.queryByRole('complementary', { name: 'Managed collection' })).toBeNull();
    const rail = screen.getByRole('button', { name: 'Open collection' });
    expect(rail).toHaveClass('collection-shell-sidebar-rail');
    await waitFor(() => expect(rail).toHaveFocus());
    fireEvent.click(rail);
    expect(screen.getByRole('complementary', { name: 'Managed collection' })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Close collection' })).toHaveFocus());
  });

  it('uses the same full-height rail when the caller composes the detail beside a sidebar-only shell', async () => {
    function SidebarOnlyFixture() {
      const [sidebarOpen, setSidebarOpen] = useState(true);
      return <div className="host-layout">
        <CollectionShell<Item>
          sidebarOnly
          ariaLabel="Sidebar-only collection"
          title="Pages"
          items={items}
          getId={item => item.id}
          getLabel={item => item.name}
          persistence={{ query: '', onQueryChange: () => {}, favoritesOnly: false, onFavoritesOnlyChange: () => {} }}
          selectedId="one"
          onSelect={() => {}}
          sidebarOpen={sidebarOpen}
          onSidebarOpenChange={setSidebarOpen}
          labels={labels}
          slots={{ renderDetail: () => null }}
        />
        <main>External detail</main>
      </div>;
    }

    render(<SidebarOnlyFixture />);
    fireEvent.click(screen.getByRole('button', { name: 'Close collection' }));
    expect(screen.queryByRole('complementary', { name: 'Sidebar-only collection' })).toBeNull();
    const rail = screen.getByRole('button', { name: 'Open collection' });
    expect(rail).toHaveClass('collection-shell-sidebar-rail');
    await waitFor(() => expect(rail).toHaveFocus());
    fireEvent.click(rail);
    expect(screen.getByRole('complementary', { name: 'Sidebar-only collection' })).toBeInTheDocument();
  });

  it('removes deleted ids from an active bulk selection after the list refreshes', () => {
    function RefreshingFixture() {
      const [currentItems, setCurrentItems] = useState(items);
      const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set(['one', 'two', 'deleted']));
      return <>
        <button type="button" onClick={() => setCurrentItems([items[0]])}>Refresh</button>
        <output>{[...selectedIds].sort().join(',')}</output>
        <CollectionShell<Item>
          ariaLabel="Refreshing collection" items={currentItems} getId={item => item.id} getLabel={item => item.name}
          persistence={{ query: '', onQueryChange: () => {}, favoritesOnly: false, onFavoritesOnlyChange: () => {} }}
          selectedId={null} onSelect={() => {}} selectedIds={selectedIds} onSelectedIdsChange={setSelectedIds} labels={labels}
          slots={{ renderDetail: () => null }}
        />
      </>;
    }
    render(<RefreshingFixture />);
    expect(screen.getByText('one,two')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    expect(screen.getByText('one', { selector: 'output' })).toBeInTheDocument();
  });

  it('restores menu-trigger focus after an action or outside pointer dismissal', async () => {
    render(<Fixture />);
    const trigger = screen.getByRole('button', { name: 'More actions' });

    fireEvent.click(trigger);
    expect(trigger).toHaveAttribute('aria-controls');
    fireEvent.click(screen.getByRole('menuitem', { name: 'Archive selected' }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.getByText('Detail: One')).toBeInTheDocument();

    fireEvent.click(trigger);
    fireEvent.pointerDown(document.body);
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.queryByRole('menuitem', { name: 'Archive selected' })).toBeNull();
  });

  it('moves through enabled action-menu items with Arrow keys, Home, and End', async () => {
    function MenuFixture() {
      const [query, setQuery] = useState('');
      const [favoritesOnly, setFavoritesOnly] = useState(false);
      return <CollectionShell<Item>
        ariaLabel="Menu collection" items={items} getId={item => item.id} getLabel={item => item.name}
        persistence={{ query, onQueryChange: setQuery, favoritesOnly, onFavoritesOnlyChange: setFavoritesOnly }}
        selectedId="one" onSelect={() => {}}
        actions={[
          { id: 'first', label: 'First action', onSelect: () => {} },
          { id: 'disabled', label: 'Disabled action', disabled: () => true, onSelect: () => {} },
          { id: 'last', label: 'Last action', onSelect: () => {} },
        ]}
        labels={labels} slots={{ renderDetail: () => null }}
      />;
    }
    render(<MenuFixture />);
    fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
    const menu = screen.getByRole('menu', { name: 'More actions' });
    expect(screen.getByRole('menuitem', { name: 'First action' })).toHaveFocus();
    fireEvent.keyDown(menu, { key: 'End' });
    expect(screen.getByRole('menuitem', { name: 'Last action' })).toHaveFocus();
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    expect(screen.getByRole('menuitem', { name: 'First action' })).toHaveFocus();
    fireEvent.keyDown(menu, { key: 'Home' });
    expect(screen.getByRole('menuitem', { name: 'First action' })).toHaveFocus();
  });

  it('supports the shared slash and arrow-key sidebar shortcuts', () => {
    render(<Fixture />);
    const sidebar = screen.getByRole('complementary', { name: 'Test collection' });
    fireEvent.keyDown(sidebar, { key: '/' });
    expect(screen.getByRole('textbox', { name: 'Search' })).toHaveFocus();
    fireEvent.keyDown(sidebar, { key: 'ArrowDown' });
    expect(screen.getByRole('button', { name: 'One' })).toHaveFocus();
    fireEvent.keyDown(sidebar, { key: 'ArrowDown' });
    expect(screen.getByRole('button', { name: 'Two' })).toHaveFocus();
  });

  it('uses the Discussions-style search zone with inline clear and compact actions', () => {
    function SearchFixture() {
      const [query, setQuery] = useState('draft');
      return <CollectionShell<Item>
        ariaLabel="Search collection" items={items} getId={item => item.id} getLabel={item => item.name}
        persistence={{ query, onQueryChange: setQuery, favoritesOnly: false, onFavoritesOnlyChange: () => {} }}
        selectedId={null} onSelect={() => {}} showSearchClear labels={labels}
        slots={{
          sidebarHeaderEnd: <button type="button" className="collection-shell-search-action">Filters</button>,
          renderDetail: () => null,
        }}
      />;
    }
    render(<SearchFixture />);
    const search = screen.getByRole('textbox', { name: 'Search' });
    expect(search).toHaveClass('collection-shell-search-input');
    expect(search).toHaveAttribute('aria-keyshortcuts', '/');
    expect(search.closest('.collection-shell-header')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Filters' }).parentElement).toHaveClass('collection-shell-search-actions');
    fireEvent.click(screen.getByRole('button', { name: 'Clear filters' }));
    expect(search).toHaveValue('');
    expect(screen.queryByRole('button', { name: 'Clear filters' })).toBeNull();
  });

  it('closes the mobile sidebar after selecting an item and restores focus to its opener', async () => {
    render(<Fixture mobile />);
    fireEvent.click(screen.getByRole('button', { name: 'Two' }));
    expect(screen.queryByRole('complementary', { name: 'Test collection' })).toBeNull();
    const opener = screen.getByRole('button', { name: 'Open collection' });
    await waitFor(() => expect(opener).toHaveFocus());
    fireEvent.click(opener);
    expect(screen.getByRole('complementary', { name: 'Test collection' })).toBeInTheDocument();
  });

  it('lets an open action menu consume Escape before closing the mobile sidebar', async () => {
    render(<Fixture mobile />);
    const trigger = screen.getByRole('button', { name: 'More actions' });
    fireEvent.click(trigger);

    expect(screen.getByRole('menu', { name: 'More actions' })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: 'Escape' });

    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.queryByRole('menu', { name: 'More actions' })).toBeNull();
    expect(screen.getByRole('complementary', { name: 'Test collection' })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Open collection' })).toHaveFocus());
  });

  it('closes an open mobile sidebar on Escape and restores focus to its opener', async () => {
    render(<Fixture mobile />);
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('complementary', { name: 'Test collection' })).toBeNull();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Open collection' })).toHaveFocus());
  });

  it('keeps explicit accent focus indicators for the search and every shared button control', () => {
    expect(collectionShellCss).toMatch(/\.collection-shell\s*\{[^}]*position:\s*relative/);
    expect(collectionShellCss).toMatch(/\.collection-shell-titlebar \.collection-shell-icon\s*\{[^}]*border:\s*1px solid var\(--kr-border\)/);
    expect(collectionShellCss).toMatch(/\.collection-shell-titlebar \.collection-shell-primary-action[^}]*\{[^}]*background:\s*var\(--kr-accent\)/);
    expect(collectionShellCss).toMatch(/\.collection-shell-header\s*\{[^}]*padding:\s*10px 12px 8px[^}]*background:\s*var\(--kr-bg-surface\)/);
    expect(collectionShellCss).toMatch(/\.collection-shell-search\s*\{[^}]*min-height:\s*36px[^}]*border:\s*1px solid var\(--kr-border-light\)[^}]*background:\s*var\(--kr-bg-input\)/);
    expect(collectionShellCss).toMatch(/\.collection-shell-search:focus-within\s*\{[^}]*border-color:\s*var\(--kr-accent\)[^}]*box-shadow:\s*0 0 0 2px rgba\(var\(--kr-accent-rgb\), 0\.12\)/);
    expect(collectionShellCss).toMatch(/\.collection-shell-search-action\s*\{[^}]*min-height:\s*36px[^}]*background:\s*var\(--kr-accent-bg\)/);
    expect(collectionShellCss).toMatch(/\.collection-shell-search-action-icon\s*\{[^}]*width:\s*36px[^}]*padding:\s*0/);
    expect(collectionShellCss).toMatch(/\.collection-shell-open:focus-visible[^{]*\{[^}]*outline:\s*2px solid var\(--kr-accent\)/);
    expect(collectionShellCss).toMatch(/\.collection-shell-collapse-button:focus-visible\s*\{[^}]*outline:\s*2px solid var\(--kr-accent\)/);
    expect(collectionShellCss).toMatch(/\.collection-shell-sidebar-rail:focus-visible\s*\{[^}]*outline:\s*2px solid var\(--kr-accent\)/);
  });

  it('lets a caller render custom grouped list markup from the filtered items, plus header/footer slots', () => {
    function GroupedFixture() {
      const [query, setQuery] = useState('');
      const [favoritesOnly, setFavoritesOnly] = useState(false);
      const [selectedId, setSelectedId] = useState<string | null>('one');
      return <CollectionShell<Item>
        ariaLabel="Grouped collection" items={items} getId={item => item.id} getLabel={item => item.name} isFavorite={item => item.favorite}
        persistence={{ query, onQueryChange: setQuery, favoritesOnly, onFavoritesOnlyChange: setFavoritesOnly }}
        selectedId={selectedId} onSelect={setSelectedId} labels={labels}
        slots={{
          renderDetail: item => <p>Detail: {item?.name ?? 'none'}</p>,
          beforeSidebarHeader: <div>Custom title row</div>,
          sidebarFooter: <div>Custom footer</div>,
          renderList: ({ visibleItems, getRowProps, isSelected }) => (
            <ul>
              {visibleItems.map(item => (
                <li key={item.id} data-selected={isSelected(item)}>
                  <button type="button" {...getRowProps(item)}>Grouped {item.name}</button>
                </li>
              ))}
            </ul>
          ),
        }}
      />;
    }
    render(<GroupedFixture />);
    expect(screen.getByText('Custom title row')).toBeInTheDocument();
    expect(screen.getByText('Custom footer')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Grouped One' }).closest('.collection-shell-list')).toBeNull();
    const oneRow = screen.getByRole('button', { name: 'Grouped One' });
    expect(oneRow.closest('li')).toHaveAttribute('data-selected', 'true');
    fireEvent.click(screen.getByRole('button', { name: 'Grouped Two' }));
    expect(screen.getByText('Detail: Two')).toBeInTheDocument();
  });

  it('keeps grouped custom rows in the shared keyboard navigation contract', () => {
    function GroupedFixture() {
      const [query, setQuery] = useState('');
      const [favoritesOnly, setFavoritesOnly] = useState(false);
      const [selectedId, setSelectedId] = useState<string | null>(null);
      return <CollectionShell<Item>
        ariaLabel="Keyboard grouped collection" items={items} getId={item => item.id} getLabel={item => item.name}
        persistence={{ query, onQueryChange: setQuery, favoritesOnly, onFavoritesOnlyChange: setFavoritesOnly }}
        selectedId={selectedId} onSelect={setSelectedId} labels={labels}
        slots={{
          renderDetail: () => null,
          renderList: ({ visibleItems, getRowProps }) => <div>{visibleItems.map(item => (
            <section key={item.id}><button type="button" {...getRowProps(item)}>Grouped {item.name}</button></section>
          ))}</div>,
        }}
      />;
    }
    render(<GroupedFixture />);
    const sidebar = screen.getByRole('complementary', { name: 'Keyboard grouped collection' });
    fireEvent.keyDown(sidebar, { key: 'End' });
    expect(screen.getByRole('button', { name: 'Grouped Two' })).toHaveFocus();
    fireEvent.keyDown(sidebar, { key: 'Home' });
    expect(screen.getByRole('button', { name: 'Grouped One' })).toHaveFocus();
    fireEvent.keyDown(sidebar, { key: 'ArrowDown' });
    expect(screen.getByRole('button', { name: 'Grouped Two' })).toHaveFocus();
  });

  it('keeps a custom row a native button while excluding disabled and hidden rows from keyboard navigation', () => {
    function GroupedFixture() {
      const [query, setQuery] = useState('');
      const [favoritesOnly, setFavoritesOnly] = useState(false);
      const [selectedId, setSelectedId] = useState<string | null>('one');
      return <CollectionShell<Item>
        ariaLabel="Accessible grouped collection" items={items} getId={item => item.id} getLabel={item => item.name}
        persistence={{ query, onQueryChange: setQuery, favoritesOnly, onFavoritesOnlyChange: setFavoritesOnly }}
        selectedId={selectedId} onSelect={setSelectedId} labels={labels}
        slots={{
          renderDetail: () => null,
          renderList: ({ visibleItems, getRowProps }) => <ul>
            <li><button type="button" {...getRowProps(visibleItems[0])}>Enabled row</button></li>
            <li><button type="button" className="collection-shell-row-button" disabled>Disabled row</button></li>
            <li hidden><button type="button" className="collection-shell-row-button">Hidden row</button></li>
          </ul>,
        }}
      />;
    }
    render(<GroupedFixture />);
    const sidebar = screen.getByRole('complementary', { name: 'Accessible grouped collection' });
    const enabled = screen.getByRole('button', { name: 'Enabled row' });
    expect(enabled).toHaveAttribute('aria-current', 'true');
    fireEvent.keyDown(sidebar, { key: 'End' });
    expect(enabled).toHaveFocus();
  });

  it('can retain a page-wide slash shortcut when requested', () => {
    function GlobalShortcutFixture() {
      const [query, setQuery] = useState('');
      const [favoritesOnly, setFavoritesOnly] = useState(false);
      return <CollectionShell<Item>
        ariaLabel="Global shortcut collection" items={items} getId={item => item.id} getLabel={item => item.name}
        persistence={{ query, onQueryChange: setQuery, favoritesOnly, onFavoritesOnlyChange: setFavoritesOnly }}
        selectedId={null} onSelect={() => {}} globalSearchShortcut labels={labels}
        slots={{ renderDetail: () => null }}
      />;
    }
    render(<GlobalShortcutFixture />);
    fireEvent.keyDown(window, { key: '/' });
    expect(screen.getByRole('textbox', { name: 'Search' })).toHaveFocus();
  });
});
