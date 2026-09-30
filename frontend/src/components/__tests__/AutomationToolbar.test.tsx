import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { useState } from 'react';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { I18nProvider } from '../../lib/I18nContext';
import { NO_AUTOMATION_FILTERS, type AutomationFilters } from '../../lib/automationFilters';
import type { AutomationSort } from '../../lib/automationSort';
import {
  AutomationToolbarPanel,
  AutomationToolbarToggle,
  type AutomationSearchPanel,
  type AutomationToolbarState,
} from '../AutomationToolbar';

// vitest blanks CSS, so the stylesheet is read as text.
const toolbarCss = readFileSync(join(import.meta.dirname, '..', 'AutomationToolbar.css'), 'utf-8');

const counts = {
  kinds: { all: 9, workflows: 4, quickPrompts: 3, quickApis: 1, quickExecs: 1 },
  states: { all: 9, favorites: 2, active: 8, inactive: 1 },
};

function Harness({ initial = NO_AUTOMATION_FILTERS, spies }: {
  initial?: AutomationFilters;
  spies: { onKind: (kind: string) => void; onClear: () => void };
}) {
  const [filters, setFilters] = useState(initial);
  const [panel, setPanel] = useState<AutomationSearchPanel>(null);
  const [sort, setSort] = useState<AutomationSort>('name');
  const [sortReversed, setSortReversed] = useState(false);
  const toolbar: AutomationToolbarState = {
    filters,
    kindCounts: counts.kinds,
    stateCounts: counts.states,
    projects: [{ id: 'p-alpha', name: 'Alpha' }],
    panel,
    setPanel,
    sort,
    setSort,
    sortReversed,
    setSortReversed,
    onKindChange: kind => { spies.onKind(kind); setFilters(current => ({ ...current, kind })); },
    onStateChange: state => setFilters(current => ({ ...current, state })),
    onProjectChange: projectId => setFilters(current => ({ ...current, projectId })),
    onClear: () => { spies.onClear(); setFilters(current => ({ ...NO_AUTOMATION_FILTERS, query: current.query })); },
  };
  return (
    <I18nProvider>
      <div data-testid="toggles"><AutomationToolbarToggle toolbar={toolbar} /></div>
      <div data-testid="panels"><AutomationToolbarPanel toolbar={toolbar} /></div>
    </I18nProvider>
  );
}

function renderToolbar(initial?: AutomationFilters) {
  const spies = { onKind: vi.fn(), onClear: vi.fn() };
  render(<Harness initial={initial} spies={spies} />);
  const toggles = screen.getByTestId('toggles');
  const panels = screen.getByTestId('panels');
  return {
    spies,
    panels,
    filterButton: within(toggles).getByRole('button', { name: 'Filtrer les automatisations' }),
    sortButton: within(toggles).getByRole('button', { name: 'Trier les automatisations' }),
  };
}

afterEach(cleanup);

describe('AutomationToolbar', () => {
  it('offers a Filter and a Sort icon, both closed until asked for', () => {
    const { panels, filterButton, sortButton } = renderToolbar();
    expect(filterButton).toHaveAttribute('aria-expanded', 'false');
    expect(sortButton).toHaveAttribute('aria-expanded', 'false');
    expect(filterButton).toHaveAttribute('data-tour-id', 'automation-filters');
    expect(panels).toBeEmptyDOMElement();
  });

  it('unfolds Type (with counts), State (with counts) and Project as full-width selects', () => {
    const { panels, filterButton } = renderToolbar();
    fireEvent.click(filterButton);
    expect(filterButton).toHaveAttribute('aria-expanded', 'true');
    expect(filterButton).toHaveAttribute('aria-controls', 'automation-filter-options');

    const type = within(panels).getByRole('combobox', { name: 'Filtre par type d’automatisation' });
    expect(type).toHaveAttribute('data-tour-id', 'automation-filter-type');
    expect(within(type).getAllByRole('option').map(option => option.textContent)).toEqual([
      'Tous (9)', 'Workflows (4)', 'Quick APIs (1)', 'Quick Prompts (3)', 'Quick Execs (CLI) (1)',
    ]);
    const state = within(panels).getByRole('combobox', { name: 'Filtre par état d’automatisation' });
    expect(within(state).getAllByRole('option').map(option => option.textContent)).toEqual([
      'Tous les états (9)', 'Favoris (2)', 'Actives (8)', 'Inactives (1)',
    ]);
    const project = within(panels).getByRole('combobox', { name: 'Filtrer les automatisations par projet' });
    expect(within(project).getAllByRole('option').map(option => option.textContent)).toEqual([
      'Tous les projets', 'Sans projet', 'Alpha',
    ]);
    // Same stack as the Plugins panel: one column, one select per row.
    expect(panels.querySelectorAll('.automation-filter-stack > .automation-filter-field')).toHaveLength(3);
  });

  it('reports each choice, and keeps the icon lit while a filter is set', () => {
    const { panels, filterButton, spies } = renderToolbar();
    expect(filterButton).toHaveAttribute('data-active', 'false');
    fireEvent.click(filterButton);
    fireEvent.click(filterButton);
    expect(filterButton).toHaveAttribute('data-active', 'false');
    fireEvent.click(filterButton);

    fireEvent.change(within(panels).getByRole('combobox', { name: 'Filtre par type d’automatisation' }), { target: { value: 'quickPrompts' } });
    expect(spies.onKind).toHaveBeenCalledWith('quickPrompts');
    fireEvent.change(within(panels).getByRole('combobox', { name: 'Filtre par état d’automatisation' }), { target: { value: 'inactive' } });
    fireEvent.change(within(panels).getByRole('combobox', { name: 'Filtrer les automatisations par projet' }), { target: { value: 'p-alpha' } });

    // Folded again, the icon still says a filter is in force.
    fireEvent.click(filterButton);
    expect(panels).toBeEmptyDOMElement();
    expect(filterButton).toHaveAttribute('data-active', 'true');
  });

  it('offers "Effacer les filtres" only while a filter is set, and resets all three', () => {
    const { panels, filterButton, spies } = renderToolbar();
    fireEvent.click(filterButton);
    expect(within(panels).queryByRole('button', { name: 'Effacer les filtres' })).toBeNull();

    fireEvent.change(within(panels).getByRole('combobox', { name: 'Filtre par état d’automatisation' }), { target: { value: 'favorites' } });
    fireEvent.change(within(panels).getByRole('combobox', { name: 'Filtrer les automatisations par projet' }), { target: { value: 'p-alpha' } });
    fireEvent.click(within(panels).getByRole('button', { name: 'Effacer les filtres' }));

    expect(spies.onClear).toHaveBeenCalledTimes(1);
    expect(within(panels).getByRole('combobox', { name: 'Filtre par état d’automatisation' })).toHaveValue('all');
    expect(within(panels).getByRole('combobox', { name: 'Filtrer les automatisations par projet' })).toHaveValue('all');
    expect(within(panels).queryByRole('button', { name: 'Effacer les filtres' })).toBeNull();
  });

  it('shows one panel at a time: Filter or Sort', () => {
    const { panels, filterButton, sortButton } = renderToolbar();
    fireEvent.click(filterButton);
    fireEvent.click(sortButton);
    expect(filterButton).toHaveAttribute('aria-expanded', 'false');
    expect(sortButton).toHaveAttribute('aria-expanded', 'true');
    expect(within(panels).queryByRole('combobox', { name: 'Filtre par type d’automatisation' })).toBeNull();
    expect(within(panels).getByRole('combobox', { name: 'Trier les automatisations' })).toBeInTheDocument();
  });

  it('sorts by name, last modification or type, and lights the icon once the order is not the default', () => {
    const { panels, sortButton } = renderToolbar();
    expect(sortButton).toHaveAttribute('data-active', 'false');
    fireEvent.click(sortButton);
    const sort = within(panels).getByRole('combobox', { name: 'Trier les automatisations' });
    expect(within(sort).getAllByRole('option').map(option => option.textContent)).toEqual([
      'Nom', 'Dernière modification', 'Type d’automatisation',
    ]);

    fireEvent.change(sort, { target: { value: 'kind' } });
    expect(sortButton).toHaveAttribute('data-active', 'true');
    fireEvent.change(sort, { target: { value: 'name' } });
    expect(sortButton).toHaveAttribute('data-active', 'true'); // still open

    fireEvent.click(within(panels).getByRole('button', { name: 'Inverser l’ordre' }));
    fireEvent.click(sortButton);
    expect(sortButton).toHaveAttribute('data-active', 'true'); // reversed
  });

  describe('400 px stylesheet', () => {
    it('stacks the selects in one shrinkable column, so the panel never scrolls sideways', () => {
      expect(toolbarCss).toMatch(/\.automation-filter-stack \{[^}]*display: grid[^}]*grid-template-columns: minmax\(0, 1fr\)[^}]*min-width: 0/);
      expect(toolbarCss).toMatch(/\.automation-filter-field select \{[^}]*box-sizing: border-box[^}]*width: 100%[^}]*min-width: 0[^}]*max-width: 100%/);
      expect(toolbarCss).toMatch(/\.automation-sidebar \.collection-shell-search-options \{[^}]*min-width: 0[^}]*overflow-x: clip/);
      expect(toolbarCss).not.toMatch(/flex-wrap: nowrap|white-space: nowrap|width: \d{3,}px/);
    });

    it('uses design tokens only', () => {
      expect(toolbarCss).not.toMatch(/#[0-9a-fA-F]{3,8}\b|rgba?\(|var\(--(?!kr-)/);
    });
  });
});
