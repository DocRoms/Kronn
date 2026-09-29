import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { I18nProvider } from '../../lib/I18nContext';
import { NO_AUTOMATION_FILTERS, type AutomationFilters } from '../../lib/automationFilters';
import { AutomationFilterBar } from '../AutomationFilterBar';
import { FilterFold } from '../FilterFold';

// vitest blanks CSS, so the stylesheets are read as text.
const stylesheet = (name: string) => readFileSync(join(import.meta.dirname, '..', name), 'utf-8');
const barCss = stylesheet('AutomationFilterBar.css');
const foldCss = stylesheet('FilterFold.css');
const narrow = (css: string) => css.slice(css.indexOf('@media (max-width: 640px)'));

const stubViewport = (matches: boolean) => vi.stubGlobal('matchMedia', vi.fn().mockImplementation((query: string) => ({
  matches, media: query, addEventListener: vi.fn(), removeEventListener: vi.fn(),
})));

const counts = {
  kinds: { all: 9, workflows: 4, quickPrompts: 3, quickApis: 1, quickExecs: 1 },
  states: { all: 9, favorites: 2, active: 8, inactive: 1 },
};

function renderBar(filters: AutomationFilters = NO_AUTOMATION_FILTERS) {
  const handlers = {
    onQueryChange: vi.fn(),
    onKindChange: vi.fn(),
    onStateChange: vi.fn(),
    onProjectChange: vi.fn(),
    onClear: vi.fn(),
  };
  render(
    <I18nProvider>
      <AutomationFilterBar
        filters={filters}
        kindCounts={counts.kinds}
        stateCounts={counts.states}
        projects={[{ id: 'p-alpha', name: 'Alpha' }]}
        {...handlers}
      />
    </I18nProvider>,
  );
  return { bar: screen.getByRole('search', { name: 'Filtres des automatisations' }), ...handlers };
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('AutomationFilterBar', () => {
  it('offers search, type, state and project in one bar, in the order of the tour', () => {
    const { bar } = renderBar();
    expect(within(bar).getByRole('textbox', { name: 'Rechercher une automatisation…' })).toHaveAttribute('aria-keyshortcuts', '/');

    const types = within(bar).getByRole('group', { name: 'Filtre par type d’automatisation' });
    expect(within(types).getAllByRole('button').map(button => button.textContent)).toEqual([
      'Tous (9)', 'Workflows (4)', 'Quick APIs (1)', 'Quick Prompts (3)', 'Quick Execs (CLI) (1)',
    ]);
    expect(within(types).getAllByRole('button').map(button => button.dataset.tourId)).toEqual([
      undefined,
      'automation-kind-workflow',
      'automation-kind-quick-api',
      'automation-kind-quick-prompt',
      'automation-kind-quick-exec',
    ]);
    const states = within(bar).getByRole('group', { name: 'Filtre par état d’automatisation' });
    expect(within(states).getAllByRole('button').map(button => button.textContent)).toEqual([
      'Tous les états (9)', 'Favoris (2)', 'Actives (8)', 'Inactives (1)',
    ]);
    const project = within(bar).getByRole('combobox', { name: 'Filtrer les automatisations par projet' });
    expect(within(project).getAllByRole('option').map(option => option.textContent)).toEqual([
      'Tous les projets', 'Sans projet', 'Alpha',
    ]);
  });

  it('reports each choice and marks the pressed chips', () => {
    const { bar, onKindChange, onStateChange, onProjectChange, onQueryChange } = renderBar({
      ...NO_AUTOMATION_FILTERS, kind: 'quickPrompts', state: 'favorites',
    });
    expect(within(bar).getByRole('button', { name: 'Quick Prompts (3)' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(bar).getByRole('button', { name: 'Favoris (2)' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(bar).getByRole('button', { name: 'Tous (9)' })).toHaveAttribute('aria-pressed', 'false');

    fireEvent.click(within(bar).getByRole('button', { name: 'Workflows (4)' }));
    expect(onKindChange).toHaveBeenCalledWith('workflows');
    fireEvent.click(within(bar).getByRole('button', { name: 'Tous (9)' }));
    expect(onKindChange).toHaveBeenCalledWith('all');
    fireEvent.click(within(bar).getByRole('button', { name: 'Inactives (1)' }));
    expect(onStateChange).toHaveBeenCalledWith('inactive');
    fireEvent.change(within(bar).getByRole('combobox'), { target: { value: 'p-alpha' } });
    expect(onProjectChange).toHaveBeenCalledWith('p-alpha');
    fireEvent.change(within(bar).getByRole('textbox'), { target: { value: 'nightly' } });
    expect(onQueryChange).toHaveBeenCalledWith('nightly');
  });

  it('offers to clear the filters only while one is set, and the search has its own clear', () => {
    cleanup();
    expect(within(renderBar().bar).queryByRole('button', { name: 'Effacer les filtres' })).toBeNull();
    cleanup();

    const { bar, onClear, onQueryChange } = renderBar({ ...NO_AUTOMATION_FILTERS, projectId: 'p-alpha', query: 'x' });
    fireEvent.click(within(bar).getByRole('button', { name: 'Effacer les filtres' }));
    expect(onClear).toHaveBeenCalledTimes(1);
    fireEvent.click(within(bar).getByRole('button', { name: 'Effacer la recherche' }));
    expect(onQueryChange).toHaveBeenCalledWith('');
  });

  it('folds type, state and project behind "Filtres (n)" on a narrow screen, the search staying in view', () => {
    stubViewport(true);
    const { bar } = renderBar({ ...NO_AUTOMATION_FILTERS, kind: 'workflows', projectId: 'p-alpha' });
    expect(within(bar).getByRole('textbox', { name: 'Rechercher une automatisation…' })).toBeInTheDocument();
    expect(within(bar).queryByRole('group')).toBeNull();
    expect(within(bar).queryByRole('combobox')).toBeNull();
    const fold = within(bar).getByRole('button', { name: 'Filtres (2)' });
    expect(fold).toHaveAttribute('aria-expanded', 'false');

    fireEvent.click(fold);
    expect(within(bar).getAllByRole('group')).toHaveLength(2);
    expect(within(bar).getByRole('combobox')).toHaveValue('p-alpha');
    expect(within(bar).getByRole('button', { name: 'Effacer les filtres' })).toBeInTheDocument();
  });

  describe('400 px stylesheet', () => {
    it('is one wrapping row, so a flex-basis can never turn into a height', () => {
      expect(barCss).toMatch(/\.automation-filterbar \{[^}]*display: flex[^}]*flex-wrap: wrap/);
      expect(barCss).not.toMatch(/flex-direction: column/);
      expect(barCss).toMatch(/\.automation-filterbar-search \{[^}]*flex: 1 1 220px[^}]*min-width: 0/);
    });

    it('gives the search the full width and every control 44 px under 640 px', () => {
      const rules = narrow(barCss);
      expect(rules).toMatch(/\.automation-filterbar-search \{[^}]*flex-basis: 100%[^}]*min-height: 44px/);
      expect(rules).toMatch(/\.collection-shell-filter \{[^}]*min-height: 44px/);
      expect(rules).toMatch(/\.automation-project-filter \{[^}]*width: 100%[^}]*min-height: 44px/);
    });

    it('lets nothing outgrow the viewport', () => {
      expect(barCss).toMatch(/\.automation-filterbar-group \{[^}]*flex-wrap: wrap[^}]*min-width: 0/);
      expect(barCss).toMatch(/\.automation-project-filter \{[^}]*max-width: 100%/);
      expect(foldCss).toMatch(/\.kr-filter-fold \{[^}]*width: 100%[^}]*min-width: 0/);
      expect(foldCss).toMatch(/\.kr-filter-fold-panel \{[^}]*min-width: 0/);
    });

    it('uses design tokens only', () => {
      for (const css of [barCss, foldCss]) {
        expect(css).not.toMatch(/#[0-9a-fA-F]{3,8}\b|rgba?\(|var\(--(?!kr-)/);
      }
    });
  });
});

describe('FilterFold', () => {
  it('renders its filters in place on a wide screen, with no button of its own', () => {
    stubViewport(false);
    render(<FilterFold label="Filtres" activeCount={3}><button type="button">Chip</button></FilterFold>);
    expect(screen.getByRole('button', { name: 'Chip' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Filtres/ })).toBeNull();
  });

  it('names the number of set filters on the button, and none when there is none', () => {
    stubViewport(true);
    const { rerender } = render(<FilterFold label="Filtres" activeCount={0}><button type="button">Chip</button></FilterFold>);
    expect(screen.getByRole('button', { name: 'Filtres' })).not.toHaveAttribute('data-active');
    rerender(<FilterFold label="Filtres" activeCount={3}><button type="button">Chip</button></FilterFold>);
    expect(screen.getByRole('button', { name: 'Filtres (3)' })).toHaveAttribute('data-active', 'true');
  });
});
