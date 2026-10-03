import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import { CollectionProjectTree } from '../CollectionProjectTree';
import type { Project } from '../../types/generated';

interface Row { id: string; projectId: string | null; projectIds?: string[] }

const projects = [
  { id: 'alpha', name: 'Alpha', path: '/work/alpha', repo_url: null },
  { id: 'beta', name: 'Beta', path: '/work/beta', repo_url: null },
] as unknown as Project[];

const labels = { noProject: 'No project', local: 'Local' };

function renderTree(items: Row[], getProjectIds?: (row: Row) => readonly string[]) {
  render(
    <CollectionProjectTree<Row>
      projects={projects}
      items={items}
      getProjectId={row => row.projectId}
      getProjectIds={getProjectIds}
      isItemActive={() => false}
      collapsedGroups={new Set()}
      onToggleGroup={() => {}}
      renderGroup={({ items: group }) => group.map(row => <p key={row.id} data-testid="row">{row.id}</p>)}
      labels={labels}
      noProjectIcon={null}
    />,
  );
}

const groupOf = (name: string) => screen.getByRole('button', { name: new RegExp(name) }).parentElement as HTMLElement;
const rowsIn = (group: HTMLElement) => within(group).queryAllByTestId('row').map(row => row.textContent);

afterEach(cleanup);

describe('CollectionProjectTree', () => {
  it('files each item under its one project, as before', () => {
    renderTree([
      { id: 'a', projectId: 'alpha' },
      { id: 'b', projectId: null },
    ]);
    expect(rowsIn(groupOf('No project'))).toEqual(['b']);
    expect(rowsIn(groupOf('Alpha'))).toEqual(['a']);
    expect(screen.queryByRole('button', { name: /Beta/ })).toBeNull();
  });

  it('files an item under every project it lists, and under "no project" when it lists none', () => {
    renderTree(
      [
        { id: 'shared', projectId: null, projectIds: ['alpha', 'beta'] },
        { id: 'solo', projectId: null, projectIds: ['alpha'] },
        { id: 'orphan', projectId: null, projectIds: [] },
      ],
      row => row.projectIds ?? [],
    );
    expect(rowsIn(groupOf('Alpha'))).toEqual(['shared', 'solo']);
    expect(rowsIn(groupOf('Beta'))).toEqual(['shared']);
    expect(rowsIn(groupOf('No project'))).toEqual(['orphan']);
    // The group counters count what each folder holds.
    expect(within(groupOf('Alpha')).getAllByText('2')[0]).toBeInTheDocument();
  });
});
