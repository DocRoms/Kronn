import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { useState } from 'react';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { I18nProvider } from '../../lib/I18nContext';
import {
  AUTOMATION_NO_PROJECT,
  NO_AUTOMATION_FILTERS,
  type AutomationFilters,
  type AutomationGroupBy,
} from '../../lib/automationFilters';
import type { AutomationSort } from '../../lib/automationSort';
import {
  AutomationSidebarControls,
  AutomationSortMenuItems,
  type AutomationControlsState,
} from '../AutomationSidebarControls';

// vitest blanks CSS, so the stylesheet is read as text.
const controlsCss = readFileSync(join(import.meta.dirname, '..', 'AutomationSidebarControls.css'), 'utf-8');

const counts = {
  kinds: { all: 11, workflows: 4, quickPrompts: 3, quickApis: 1, quickExecs: 1, skills: 2 },
  projects: { all: 11, 'p-alpha': 5, 'p-beta': 0, 'p-gamma': 2, [AUTOMATION_NO_PROJECT]: 4 },
};
const projects = [
  { id: 'p-alpha', name: 'Alpha' },
  { id: 'p-beta', name: 'Beta' },
  { id: 'p-gamma', name: 'Gamma' },
];

function Harness({ initial = NO_AUTOMATION_FILTERS, spies }: {
  initial?: AutomationFilters;
  spies: { onKind: (kind: string) => void; onGroupBy: (groupBy: string) => void; onClear: () => void; onSidebarKey: () => void };
}) {
  const [filters, setFilters] = useState(initial);
  const [groupBy, setGroupBy] = useState<AutomationGroupBy>('kind');
  const controls: AutomationControlsState = {
    groupBy,
    onGroupByChange: next => { spies.onGroupBy(next); setGroupBy(next); },
    filters,
    kindCounts: counts.kinds,
    projectCounts: counts.projects,
    projects,
    onKindChange: kind => { spies.onKind(kind); setFilters(current => ({ ...current, kind })); },
    onPinnedChange: pinned => setFilters(current => ({ ...current, pinned })),
    onActiveChange: active => setFilters(current => ({ ...current, active })),
    onRecentChange: recent => setFilters(current => ({ ...current, recent })),
    onProjectChange: projectId => setFilters(current => ({ ...current, projectId })),
    onClear: () => { spies.onClear(); setFilters(current => ({ ...NO_AUTOMATION_FILTERS, query: current.query })); },
  };
  return (
    <I18nProvider>
      {/* Stands for the sidebar, whose own key handler walks the list rows. */}
      <div data-testid="controls" onKeyDown={spies.onSidebarKey}><AutomationSidebarControls controls={controls} /></div>
    </I18nProvider>
  );
}

function renderControls(initial?: AutomationFilters) {
  const spies = { onKind: vi.fn(), onGroupBy: vi.fn(), onClear: vi.fn(), onSidebarKey: vi.fn() };
  render(<Harness initial={initial} spies={spies} />);
  const root = screen.getByTestId('controls');
  return {
    spies,
    root,
    kindChip: () => within(root).getByRole('button', { name: /^Type d’automatisation : / }),
    projectChip: () => within(root).getByRole('button', { name: 'Filtrer les automatisations par projet' }),
    toggle: (name: string) => within(root).getByRole('button', { name }),
    clear: () => within(root).queryByRole('button', { name: 'Effacer les filtres' }),
  };
}

afterEach(cleanup);

describe('AutomationSidebarControls — Group by', () => {
  it('offers Type, Project and None in one segmented control, Type pressed first', () => {
    const { root, spies } = renderControls();
    const segmented = within(root).getByRole('group', { name: 'Grouper par' });
    expect(within(segmented).getAllByRole('button').map(button => button.textContent)).toEqual(['Type', 'Projet', 'Aucun']);
    expect(within(segmented).getByRole('button', { name: 'Type' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(segmented).getByRole('button', { name: 'Projet' })).toHaveAttribute('aria-pressed', 'false');

    fireEvent.click(within(segmented).getByRole('button', { name: 'Projet' }));
    expect(spies.onGroupBy).toHaveBeenCalledWith('project');
    expect(within(segmented).getByRole('button', { name: 'Projet' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(segmented).getByRole('button', { name: 'Type' })).toHaveAttribute('aria-pressed', 'false');
    fireEvent.click(within(segmented).getByRole('button', { name: 'Aucun' }));
    expect(spies.onGroupBy).toHaveBeenLastCalledWith('none');
    expect(within(segmented).getByRole('button', { name: 'Aucun' })).toHaveAttribute('aria-pressed', 'true');
  });
});

describe('AutomationSidebarControls — chips', () => {
  it('starts on "Tout", with the toggles off, "+ Projet" offered and nothing to clear', () => {
    const { root, kindChip, projectChip, toggle, clear } = renderControls();
    const chips = root.querySelector('.automation-chips') as HTMLElement;
    expect(chips).toHaveAttribute('data-tour-id', 'automation-filters');
    expect(kindChip()).toHaveTextContent('Tout');
    expect(kindChip()).toHaveAttribute('data-tour-id', 'automation-filter-type');
    expect(kindChip()).toHaveAttribute('data-value', 'all');
    expect(kindChip()).toHaveAttribute('aria-expanded', 'false');
    for (const name of ['Épinglés', 'Actifs', 'Récents']) {
      expect(toggle(name)).toHaveAttribute('aria-pressed', 'false');
    }
    expect(projectChip()).toHaveTextContent('Projet');
    expect(clear()).toBeNull();
    expect(within(root).queryByRole('listbox')).toBeNull();
  });

  it('opens the list of types with their counts, and picks one', () => {
    const { root, spies, kindChip, clear } = renderControls();
    fireEvent.click(kindChip());
    expect(kindChip()).toHaveAttribute('aria-expanded', 'true');
    const list = within(root).getByRole('listbox', { name: 'Filtre par type d’automatisation' });
    expect(within(list).getAllByRole('option').map(option => option.getAttribute('aria-label'))).toEqual([
      'Tout (11)', 'Workflows (4)', 'Quick Prompts (3)', 'Quick APIs (1)', 'Quick Execs (CLI) (1)', 'Skills (2)',
    ]);
    expect(within(list).getByRole('option', { name: 'Tout (11)' })).toHaveAttribute('aria-selected', 'true');

    fireEvent.click(within(list).getByRole('option', { name: 'Skills (2)' }));
    expect(spies.onKind).toHaveBeenCalledWith('skills');
    expect(within(root).queryByRole('listbox')).toBeNull();
    expect(kindChip()).toHaveTextContent('Skills');
    expect(kindChip()).toHaveAttribute('data-value', 'skills');
    expect(kindChip()).toHaveAttribute('data-active', 'true');
    // A type other than "Tout" is a filter in force.
    expect(clear()).not.toBeNull();
  });

  it('closes the list on Escape, back on the chip, and on a click elsewhere', () => {
    const { root, kindChip } = renderControls();
    fireEvent.click(kindChip());
    expect(within(root).getByRole('listbox')).toBeInTheDocument();
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(within(root).queryByRole('listbox')).toBeNull();
    expect(kindChip()).toHaveFocus();

    fireEvent.click(kindChip());
    expect(within(root).getByRole('listbox')).toBeInTheDocument();
    fireEvent.pointerDown(document.body);
    expect(within(root).queryByRole('listbox')).toBeNull();

    // The chip itself opens and closes it.
    fireEvent.click(kindChip());
    fireEvent.click(kindChip());
    expect(within(root).queryByRole('listbox')).toBeNull();
  });

  it('walks the types with the arrow keys without leaking them to the sidebar rows', () => {
    const { root, spies, kindChip } = renderControls();
    fireEvent.click(kindChip());
    const options = within(root).getAllByRole('option');
    // The chosen type takes the focus when the list opens.
    expect(options[0]).toHaveFocus();
    fireEvent.keyDown(options[0], { key: 'ArrowDown' });
    expect(options[1]).toHaveFocus();
    fireEvent.keyDown(options[1], { key: 'End' });
    expect(options[options.length - 1]).toHaveFocus();
    fireEvent.keyDown(options[options.length - 1], { key: 'ArrowDown' });
    expect(options[0]).toHaveFocus();
    fireEvent.keyDown(options[0], { key: 'ArrowUp' });
    expect(options[options.length - 1]).toHaveFocus();
    fireEvent.keyDown(options[options.length - 1], { key: 'Home' });
    expect(options[0]).toHaveFocus();
    expect(spies.onSidebarKey).not.toHaveBeenCalled();
    // Other keys still reach the sidebar.
    fireEvent.keyDown(options[0], { key: 'a' });
    expect(spies.onSidebarKey).toHaveBeenCalledTimes(1);
  });

  it('keeps Pinned, Active and Recent as independent toggles that stack', () => {
    const { toggle, clear, kindChip } = renderControls();
    fireEvent.click(toggle('Épinglés'));
    expect(toggle('Épinglés')).toHaveAttribute('aria-pressed', 'true');
    expect(toggle('Actifs')).toHaveAttribute('aria-pressed', 'false');
    expect(clear()).not.toBeNull();
    fireEvent.click(toggle('Actifs'));
    fireEvent.click(toggle('Récents'));
    for (const name of ['Épinglés', 'Actifs', 'Récents']) {
      expect(toggle(name)).toHaveAttribute('aria-pressed', 'true');
    }
    // Pressing again lifts just that one.
    fireEvent.click(toggle('Actifs'));
    expect(toggle('Actifs')).toHaveAttribute('aria-pressed', 'false');
    expect(toggle('Épinglés')).toHaveAttribute('aria-pressed', 'true');
    expect(kindChip()).toHaveTextContent('Tout');
  });

  it('offers "Effacer les filtres" only while a filter is set, and resets every chip', () => {
    const { root, spies, toggle, kindChip, projectChip, clear } = renderControls();
    expect(clear()).toBeNull();

    fireEvent.click(toggle('Récents'));
    fireEvent.click(kindChip());
    fireEvent.click(within(root).getByRole('option', { name: 'Workflows (4)' }));
    fireEvent.click(projectChip());
    fireEvent.click(within(root).getByRole('option', { name: 'Alpha (5)' }));
    fireEvent.click(clear() as HTMLElement);

    expect(spies.onClear).toHaveBeenCalledTimes(1);
    expect(kindChip()).toHaveTextContent('Tout');
    expect(toggle('Récents')).toHaveAttribute('aria-pressed', 'false');
    expect(projectChip()).toBeInTheDocument();
    expect(clear()).toBeNull();
  });

  it('does not count the search as a filter: clearing the chips leaves it alone', () => {
    const { clear } = renderControls({ ...NO_AUTOMATION_FILTERS, query: 'alpha' });
    expect(clear()).toBeNull();
  });
});

describe('AutomationSidebarControls — project chip', () => {
  it('lists the projects that hold something, with "Sans projet" first and their counts', () => {
    const { root, projectChip } = renderControls();
    fireEvent.click(projectChip());
    const list = within(root).getByRole('listbox', { name: 'Filtrer les automatisations par projet' });
    // Beta holds nothing given the other filters: it is not offered.
    expect(within(list).getAllByRole('option').map(option => option.getAttribute('aria-label'))).toEqual([
      'Sans projet (4)', 'Alpha (5)', 'Gamma (2)',
    ]);
  });

  it('shows the chosen project as a removable chip, and "+ Projet" again once removed', () => {
    const { root, projectChip, clear } = renderControls();
    fireEvent.click(projectChip());
    fireEvent.click(within(root).getByRole('option', { name: 'Alpha (5)' }));

    expect(within(root).queryByRole('button', { name: 'Filtrer les automatisations par projet' })).toBeNull();
    const chip = within(root).getByRole('button', { name: 'Alpha' });
    expect(chip.closest('.automation-chip')).toHaveAttribute('data-active', 'true');
    expect(clear()).not.toBeNull();

    // The name reopens the list (the chosen project is marked), the cross removes it.
    fireEvent.click(chip);
    expect(within(root).getByRole('option', { name: 'Alpha (5)' })).toHaveAttribute('aria-selected', 'true');
    fireEvent.keyDown(window, { key: 'Escape' });
    fireEvent.click(within(root).getByRole('button', { name: 'Retirer le filtre projet Alpha' }));
    expect(within(root).queryByRole('button', { name: 'Alpha' })).toBeNull();
    expect(projectChip()).toBeInTheDocument();
    expect(clear()).toBeNull();
  });

  it('reads "Sans projet" as a project too', () => {
    const { root, projectChip } = renderControls();
    fireEvent.click(projectChip());
    fireEvent.click(within(root).getByRole('option', { name: 'Sans projet (4)' }));
    expect(within(root).getByRole('button', { name: 'Sans projet' })).toBeInTheDocument();
    expect(within(root).getByRole('button', { name: 'Retirer le filtre projet Sans projet' })).toBeInTheDocument();
  });

  it('keeps a chosen project in the list even when it now holds nothing', () => {
    const { root, projectChip } = renderControls({ ...NO_AUTOMATION_FILTERS, projectId: 'p-beta' });
    fireEvent.click(within(root).getByRole('button', { name: 'Beta' }));
    expect(within(root).getAllByRole('option').map(option => option.getAttribute('aria-label'))).toContain('Beta (0)');
    expect(() => projectChip()).toThrow();
  });
});

describe('AutomationSortMenuItems', () => {
  function SortHarness({ onSort }: { onSort: (sort: AutomationSort) => void }) {
    const [sort, setSort] = useState<AutomationSort>('name');
    const [reversed, setReversed] = useState(false);
    return (
      <I18nProvider>
        <AutomationSortMenuItems
          sort={sort}
          onSortChange={next => { onSort(next); setSort(next); }}
          reversed={reversed}
          onReversedChange={setReversed}
        />
      </I18nProvider>
    );
  }

  it('sorts by name, last modification or last opening, and reverses the order', () => {
    const onSort = vi.fn();
    render(<SortHarness onSort={onSort} />);
    const group = screen.getByRole('group', { name: 'Trier les automatisations' });
    const radios = within(group).getAllByRole('menuitemradio');
    expect(radios.map(radio => radio.textContent)).toEqual(['Nom', 'Dernière modification', 'Dernière ouverture']);
    expect(radios.map(radio => radio.getAttribute('aria-checked'))).toEqual(['true', 'false', 'false']);

    fireEvent.click(within(group).getByRole('menuitemradio', { name: 'Dernière ouverture' }));
    expect(onSort).toHaveBeenCalledWith('opened');
    expect(within(group).getByRole('menuitemradio', { name: 'Dernière ouverture' })).toHaveAttribute('aria-checked', 'true');
    expect(within(group).getByRole('menuitemradio', { name: 'Nom' })).toHaveAttribute('aria-checked', 'false');

    const reverse = within(group).getByRole('menuitemcheckbox', { name: 'Inverser l’ordre' });
    expect(reverse).toHaveAttribute('aria-checked', 'false');
    fireEvent.click(reverse);
    expect(within(group).getByRole('menuitemcheckbox', { name: 'Rétablir l’ordre par défaut' })).toHaveAttribute('aria-checked', 'true');
  });
});

describe('400 px stylesheet', () => {
  it('keeps the block of controls in one shrinkable column, so a wide child cannot widen it', () => {
    expect(controlsCss).toMatch(/\.automation-controls \{[^}]*position: relative[^}]*display: grid[^}]*grid-template-columns: minmax\(0, 1fr\)[^}]*min-width: 0/);
  });

  it('wraps the chips and gives the segmented control the full width', () => {
    expect(controlsCss).toMatch(/\.automation-chips \{[^}]*display: flex[^}]*flex-wrap: wrap[^}]*min-width: 0/);
    expect(controlsCss).toMatch(/\.automation-segmented \{[^}]*display: grid[^}]*flex: 1[^}]*grid-template-columns: repeat\(3, minmax\(0, 1fr\)\)[^}]*min-width: 0/);
    expect(controlsCss).toMatch(/\.automation-groupby \{[^}]*display: flex[^}]*min-width: 0/);
  });

  it('opens a menu over the width of its block, never wider, and cuts a long project name', () => {
    expect(controlsCss).toMatch(/\.automation-chip-menu \{[^}]*position: absolute[^}]*left: var\(--kr-sp-3\)[^}]*right: var\(--kr-sp-3\)[^}]*overflow-x: hidden/);
    expect(controlsCss).toMatch(/\.automation-chip-project-name \{[^}]*min-width: 0[^}]*text-overflow: ellipsis/);
    expect(controlsCss).toMatch(/\.automation-chip,\n\.automation-chip-clear \{[^}]*max-width: 100%/);
  });

  it('never forces a row to stay on one line or to a fixed width', () => {
    expect(controlsCss).not.toMatch(/flex-wrap: nowrap|width: \d{3,}px|min-width: \d{3,}px/);
  });

  it('uses design tokens only', () => {
    expect(controlsCss).not.toMatch(/#[0-9a-fA-F]{3,8}\b|rgba?\(|var\(--(?!kr-)/);
  });
});
