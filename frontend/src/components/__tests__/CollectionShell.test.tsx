import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { CollectionShell } from '../CollectionShell';

type Item = { id: string; name: string; favorite: boolean; archived?: boolean };
const items: Item[] = [{ id: 'one', name: 'One', favorite: true }, { id: 'two', name: 'Two', favorite: false, archived: true }];
const labels = { search: 'Search', favorites: 'Favorites', clearFilters: 'Clear filters', moreActions: 'More actions', openCollection: 'Open collection', closeCollection: 'Close collection', selectItem: 'selected' };

function Fixture({ mobile = false }: { mobile?: boolean }) {
  const [query, setQuery] = useState('');
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [filter, setFilter] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>('one');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [sidebarOpen, setSidebarOpen] = useState(true);
  return <CollectionShell<Item>
    ariaLabel="Test collection" items={items} getId={item => item.id} getLabel={item => item.name} isFavorite={item => item.favorite}
    filters={[{ id: 'archived', label: 'Archived', matches: item => !!item.archived }]}
    persistence={{ query, onQueryChange: setQuery, favoritesOnly, onFavoritesOnlyChange: setFavoritesOnly, activeFilterId: filter, onActiveFilterIdChange: setFilter }}
    selectedId={selectedId} onSelect={setSelectedId} selectedIds={selectedIds} onSelectedIdsChange={setSelectedIds}
    actions={[{ id: 'archive', label: 'Archive selected', onSelect: selected => setSelectedId(selected.map(item => item.id).join(',')) }]}
    isMobile={mobile} sidebarOpen={sidebarOpen} onSidebarOpenChange={setSidebarOpen} labels={labels}
    slots={{ renderDetail: item => <p>Detail: {item?.name ?? 'none'}</p>, renderEmpty: () => <>Nothing found</> }}
  />;
}

describe('CollectionShell', () => {
  it('filters, favorites, and renders the shared empty state', () => {
    render(<Fixture />);
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
    await waitFor(() => expect(screen.getByRole('menuitem', { name: 'Archive selected' })).toHaveFocus());
    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it('restores menu-trigger focus after an action or outside pointer dismissal', async () => {
    render(<Fixture />);
    const trigger = screen.getByRole('button', { name: 'More actions' });

    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Archive selected' }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.getByText('Detail: One')).toBeInTheDocument();

    fireEvent.click(trigger);
    fireEvent.pointerDown(document.body);
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.queryByRole('menuitem', { name: 'Archive selected' })).toBeNull();
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

  it('closes the mobile sidebar after selecting an item and lets it reopen', () => {
    render(<Fixture mobile />);
    fireEvent.click(screen.getByRole('button', { name: 'Two' }));
    expect(screen.queryByRole('complementary', { name: 'Test collection' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Open collection' }));
    expect(screen.getByRole('complementary', { name: 'Test collection' })).toBeInTheDocument();
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
