import { describe, expect, it } from 'vitest';
import { link, listing, resource } from '../../components/__tests__/repositoryResourceFixtures';
import {
  dependencyClosure,
  linkedCount,
  linkedToggleNote,
  namesOf,
  rowGraph,
  toggleLinked,
  transferLinks,
} from '../repositoryResourceLinks';
import { allRows, buildRows } from '../repositoryResourceRows';

const nightly = resource({ id: 'wf-nightly', name: 'Nightly triage', kind: 'workflow', status: 'kronn_only' });
const review = resource({ id: 'qp-review', name: 'Review', kind: 'quick_prompt', status: 'kronn_only' });
const fetch = resource({ id: 'qa-fetch', name: 'Fetch', kind: 'quick_api', status: 'kronn_only' });
const board = resource({ id: 'page-board', name: 'Board', kind: 'artifact', status: 'kronn_only' });
const lint = resource({ id: 'qe-lint', name: 'Lint', kind: 'quick_exec', status: 'kronn_only' });
const shared = resource({ id: 'wf-shared', name: 'Shared', kind: 'workflow', status: 'up_to_date' });

/** nightly → review, fetch, board, shared; shared → lint (already in the repository). */
const chain = () => {
  const withEdges = [
    { ...nightly, uses: [link(review), link(fetch), link(board), link(shared), link({ id: 'ghost', name: 'ghost', kind: 'quick_prompt' }, true)] },
    { ...review, used_by: [link(nightly)] },
    { ...fetch, used_by: [link(nightly)] },
    { ...board, used_by: [link(nightly)] },
    { ...shared, uses: [link(lint)], used_by: [link(nightly)] },
    { ...lint, used_by: [link(shared)] },
  ];
  return rowGraph(allRows(buildRows(listing({ resources: withEdges }))));
};

const key = (target: { kind: string; id: string }) => `${target.kind}:${target.id}`;
const tickable = (graph: ReturnType<typeof chain>) => (dep: string) => {
  const row = graph.get(dep);
  return row?.state === 'kronn_only' && !row.suggested;
};

describe('resource links', () => {
  describe('dependency closure', () => {
    it('follows references through resources that are already written, and skips missing ones', () => {
      const graph = chain();
      expect(dependencyClosure(graph, key(nightly))).toEqual([
        key(review), key(fetch), key(board), key(shared), key(lint),
      ]);
      expect(dependencyClosure(graph, key(review))).toEqual([]);
    });

    it('walks a loop once and never lists the resource itself', () => {
      const first = resource({ id: 'a', name: 'A', kind: 'workflow', status: 'kronn_only' });
      const second = resource({ id: 'b', name: 'B', kind: 'workflow', status: 'kronn_only' });
      const third = resource({ id: 'c', name: 'C', kind: 'workflow', status: 'kronn_only' });
      const graph = rowGraph(allRows(buildRows(listing({
        resources: [
          { ...first, uses: [link(second), link(first)] },
          { ...second, uses: [link(third)] },
          { ...third, uses: [link(first)] },
        ],
      }))));
      expect(dependencyClosure(graph, key(first))).toEqual([key(second), key(third)]);
      expect(dependencyClosure(graph, key(third))).toEqual([key(first), key(second)]);
    });

    it('counts each linked resource once, whichever way the link goes', () => {
      const graph = chain();
      expect(linkedCount(graph.get(key(nightly))!)).toBe(5);
      expect(linkedCount(graph.get(key(shared))!)).toBe(2);
      expect(linkedCount(graph.get(key(review))!)).toBe(1);
      const loop = resource({ id: 'l', name: 'Loop', kind: 'workflow', status: 'kronn_only', uses: [link(review)], used_by: [link(review)] });
      expect(linkedCount(allRows(buildRows(listing({ resources: [loop] })))[0])).toBe(1);
    });
  });

  describe('ticking', () => {
    it('ticks what a resource needs, through resources it cannot tick itself', () => {
      const graph = chain();
      const result = toggleLinked(graph, new Set(), key(nightly), tickable(graph));
      // `shared` is already written and cannot be ticked; `lint`, behind it, can.
      expect(result.added).toEqual([key(review), key(fetch), key(board), key(lint)]);
      expect([...result.checked].sort()).toEqual([key(nightly), ...result.added].sort());
      expect(result.blockedBy).toEqual([]);
    });

    it('does not tick twice what is already ticked', () => {
      const graph = chain();
      const result = toggleLinked(graph, new Set([key(review)]), key(nightly), tickable(graph));
      expect(result.added).toEqual([key(fetch), key(board), key(lint)]);
    });

    it('refuses to untick what a ticked resource still needs, and says which', () => {
      const graph = chain();
      const ticked = toggleLinked(graph, new Set(), key(nightly), tickable(graph)).checked;

      const refused = toggleLinked(graph, ticked, key(review), tickable(graph));
      expect(refused.blockedBy).toEqual([key(nightly)]);
      expect(refused.checked).toEqual(ticked);
      expect(namesOf(graph, refused.blockedBy)).toBe('Nightly triage');
      // Behind an already written resource, it is still needed.
      expect(toggleLinked(graph, ticked, key(lint), tickable(graph)).blockedBy).toEqual([key(nightly)]);

      const released = toggleLinked(graph, ticked, key(nightly), tickable(graph));
      expect(released.blockedBy).toEqual([]);
      expect(released.checked.has(key(nightly))).toBe(false);
      // What came along stays ticked, and can now be unticked in turn.
      expect(released.checked.has(key(review))).toBe(true);
      expect(toggleLinked(graph, released.checked, key(review), tickable(graph)).blockedBy).toEqual([]);
    });

    it('unticks one of two resources that need each other together with the other', () => {
      const first = resource({ id: 'a', name: 'A', kind: 'workflow', status: 'kronn_only' });
      const second = resource({ id: 'b', name: 'B', kind: 'workflow', status: 'kronn_only' });
      const graph = rowGraph(allRows(buildRows(listing({
        resources: [{ ...first, uses: [link(second)] }, { ...second, uses: [link(first)] }],
      }))));
      const ticked = toggleLinked(graph, new Set(), key(first), tickable(graph));
      expect(ticked.added).toEqual([key(second)]);

      const unticked = toggleLinked(graph, ticked.checked, key(second), tickable(graph));
      expect(unticked.blockedBy).toEqual([]);
      expect(unticked.removed).toEqual([key(first)]);
      expect(unticked.checked.size).toBe(0);
    });

    it('keeps a loop ticked while something outside it still needs it', () => {
      const first = resource({ id: 'a', name: 'A', kind: 'workflow', status: 'kronn_only' });
      const second = resource({ id: 'b', name: 'B', kind: 'workflow', status: 'kronn_only' });
      const outer = resource({ id: 'o', name: 'Outer', kind: 'workflow', status: 'kronn_only' });
      const graph = rowGraph(allRows(buildRows(listing({
        resources: [
          { ...first, uses: [link(second)] },
          { ...second, uses: [link(first)] },
          { ...outer, uses: [link(second)] },
        ],
      }))));
      const ticked = toggleLinked(graph, new Set(), key(outer), tickable(graph)).checked;
      expect(ticked).toEqual(new Set([key(outer), key(second), key(first)]));

      for (const inLoop of [first, second]) {
        const refused = toggleLinked(graph, ticked, key(inLoop), tickable(graph));
        expect(refused.blockedBy).toEqual([key(outer)]);
        expect(refused.checked).toEqual(ticked);
      }
    });

    it('words what happened: what came along, what was refused and by whom', () => {
      const graph = chain();
      const t = (text: string, ...args: Array<string | number>) => (args.length ? `${text}:${args.join('|')}` : text);
      const added = toggleLinked(graph, new Set(), key(nightly), tickable(graph));
      expect(linkedToggleNote(t, graph, key(nightly), added)).toEqual({
        tone: 'info',
        text: 'projects.repositoryResources.links.added.other:4',
      });
      const refused = toggleLinked(graph, added.checked, key(review), tickable(graph));
      expect(linkedToggleNote(t, graph, key(review), refused)).toEqual({
        tone: 'warning',
        text: 'projects.repositoryResources.links.blocked:Review|projects.repositoryResources.links.requiredBy:Nightly triage',
      });
      const plain = toggleLinked(graph, new Set(), key(review), tickable(graph));
      expect(linkedToggleNote(t, graph, key(review), plain)).toBeNull();
    });
  });

  describe('a transfer of one resource', () => {
    it('lists what still has to be written, dependencies first, and what stays as it is', () => {
      const graph = chain();
      const links = transferLinks(graph, graph.get(key(nightly))!, 'repository');
      expect(links.pending.map(row => row.name)).toEqual(['Lint', 'Board', 'Fetch', 'Review']);
      expect(links.settled.map(row => row.name)).toEqual(['Shared']);
      expect(links.missing.map(item => item.id)).toEqual(['ghost']);
    });

    it('lists what still has to be loaded when the repository is the newer side', () => {
      const late = resource({ id: 'wf-late', name: 'Late', kind: 'workflow', status: 'repository_newer' });
      const needed = resource({ id: 'repository:quick_prompt:ask', name: 'Ask', kind: 'quick_prompt', status: 'repository_only' });
      const done = resource({ id: 'qp-done', name: 'Done', kind: 'quick_prompt', status: 'up_to_date' });
      const graph = rowGraph(allRows(buildRows(listing({
        resources: [{ ...late, uses: [link(needed), link(done)] }, needed, done],
      }))));
      const links = transferLinks(graph, graph.get(key(late))!, 'kronn');
      expect(links.pending.map(row => row.name)).toEqual(['Ask']);
      expect(links.settled.map(row => row.name)).toEqual(['Done']);
      expect(links.missing).toEqual([]);
      expect(transferLinks(graph, graph.get(key(late))!, 'repository').pending).toEqual([]);
    });

    it('has nothing to announce for a resource that uses nothing', () => {
      const graph = chain();
      expect(transferLinks(graph, graph.get(key(review))!, 'repository')).toEqual({ pending: [], settled: [], missing: [] });
    });
  });
});
