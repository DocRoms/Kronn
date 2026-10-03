import type { ResourceRow, RowLink, SyncState } from './repositoryResourceRows';

/** Rows by key: the graph a resource's `uses` links walk. */
export type RowGraph = ReadonlyMap<string, ResourceRow>;

export const rowGraph = (rows: ResourceRow[]): Map<string, ResourceRow> => (
  new Map(rows.map(row => [row.key, row]))
);

/** How many distinct resources are linked to this one, either way. */
export function linkedCount(row: ResourceRow): number {
  return new Set([...row.uses, ...row.usedBy].map(link => link.key)).size;
}

/** Everything `key` needs, transitively, in the order it is reached. A
 *  reference to a missing resource names nothing selectable, and a loop is
 *  walked once: `key` itself is never part of its own closure. */
export function dependencyClosure(graph: RowGraph, key: string): string[] {
  const seen = new Set<string>([key]);
  const queue = [key];
  for (let index = 0; index < queue.length; index += 1) {
    for (const link of graph.get(queue[index])?.uses ?? []) {
      if (link.missing || !graph.has(link.key) || seen.has(link.key)) continue;
      seen.add(link.key);
      queue.push(link.key);
    }
  }
  return queue.slice(1);
}

export interface LinkedToggle {
  checked: Set<string>;
  /** Dependencies checked along with the item. */
  added: string[];
  /** Items of the same loop unchecked along with it. */
  removed: string[];
  /** Checked items that still need it: the toggle was refused. */
  blockedBy: string[];
}

/** Checked items that need `key` and cannot do without it. Two items that
 *  need each other form a loop and stand or fall together, so a loop mate is
 *  not a requirer. */
function requirers(
  graph: RowGraph,
  checked: ReadonlySet<string>,
  key: string,
  toggleable: (key: string) => boolean,
): { blockers: string[]; mates: string[] } {
  const own = new Set(dependencyClosure(graph, key));
  const blockers: string[] = [];
  const mates: string[] = [];
  for (const candidate of checked) {
    if (candidate === key || !toggleable(candidate)) continue;
    if (!dependencyClosure(graph, candidate).includes(key)) continue;
    (own.has(candidate) ? mates : blockers).push(candidate);
  }
  return { blockers, mates };
}

/** Checks or unchecks `key` so the selection stays closed under "uses".
 *  Checking adds the whole dependency closure that can be checked. Unchecking
 *  is refused while a checked item still needs it — `blockedBy` names them —
 *  and takes its loop mates with it. `toggleable` says which items a person
 *  can check at all; the others are left as they are. */
export function toggleLinked(
  graph: RowGraph,
  checked: ReadonlySet<string>,
  key: string,
  toggleable: (key: string) => boolean,
): LinkedToggle {
  const next = new Set(checked);
  if (!checked.has(key)) {
    next.add(key);
    const added = dependencyClosure(graph, key).filter(dep => !next.has(dep) && toggleable(dep));
    added.forEach(dep => next.add(dep));
    return { checked: next, added, removed: [], blockedBy: [] };
  }
  const { blockers, mates } = requirers(graph, checked, key, toggleable);
  if (blockers.length > 0) return { checked: next, added: [], removed: [], blockedBy: blockers };
  next.delete(key);
  mates.forEach(mate => next.delete(mate));
  return { checked: next, added: [], removed: mates, blockedBy: [] };
}

/** The names of the rows behind some keys, for a sentence. */
export const namesOf = (graph: RowGraph, keys: string[]): string => (
  keys.map(key => graph.get(key)?.name ?? key).join(', ')
);

export interface LinkNote {
  tone: 'info' | 'warning';
  text: string;
}

type Translate = (key: string, ...args: (string | number)[]) => string;

/** The sentence a toggle earns: what came along, or why it was refused. */
export function linkedToggleNote(
  t: Translate,
  graph: RowGraph,
  key: string,
  toggle: LinkedToggle,
): LinkNote | null {
  if (toggle.blockedBy.length > 0) {
    return {
      tone: 'warning',
      text: t(
        'projects.repositoryResources.links.blocked',
        graph.get(key)?.name ?? key,
        t('projects.repositoryResources.links.requiredBy', namesOf(graph, toggle.blockedBy)),
      ),
    };
  }
  if (toggle.added.length > 0) {
    return {
      tone: 'info',
      text: toggle.added.length === 1
        ? t('projects.repositoryResources.links.added.one')
        : t('projects.repositoryResources.links.added.other', toggle.added.length),
    };
  }
  if (toggle.removed.length > 0) {
    return {
      tone: 'info',
      text: toggle.removed.length === 1
        ? t('projects.repositoryResources.links.removed.one')
        : t('projects.repositoryResources.links.removed.other', toggle.removed.length),
    };
  }
  return null;
}

export type TransferSide = 'repository' | 'kronn';

/** States in which a linked item still has to move for the side being written. */
const PENDING_STATES: Record<TransferSide, SyncState[]> = {
  repository: ['kronn_only', 'kronn_newer'],
  kronn: ['repository_only', 'repository_newer'],
};

export interface TransferLinks {
  /** Linked items the transfer would also have to move, dependencies first. */
  pending: ResourceRow[];
  /** Linked items the transfer leaves alone: already aligned, or holding a
   *  decision of their own. */
  settled: ResourceRow[];
  /** References that lead to nothing the project holds. */
  missing: RowLink[];
}

/** What writing (`repository`) or loading (`kronn`) one resource brings with
 *  it: its dependency closure, split by whether each item still has to move. */
export function transferLinks(graph: RowGraph, row: ResourceRow, side: TransferSide): TransferLinks {
  const closure = dependencyClosure(graph, row.key)
    .flatMap(key => graph.get(key) ?? [])
    .reverse();
  const isPending = (item: ResourceRow) => (
    PENDING_STATES[side].includes(item.state) && !item.suggested
  );
  const missing = new Map<string, RowLink>();
  for (const item of [row, ...closure]) {
    for (const link of item.uses) if (link.missing) missing.set(link.key, link);
  }
  return {
    pending: closure.filter(isPending),
    settled: closure.filter(item => !isPending(item)),
    missing: [...missing.values()],
  };
}
