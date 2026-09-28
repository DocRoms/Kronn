import { isExecutable, type ResourceRow } from './repositoryResourceRows';

export type TransferKind =
  | 'publish'
  | 'update_repository'
  | 'import'
  | 'update_kronn'
  | 'use_native'
  | 'copy_native'
  | 'attach'
  | 'publish_selected';

export interface TransferPlan {
  kind: TransferKind;
  rows: ResourceRow[];
}

export type EffectTone = 'write' | 'commit' | 'activation' | 'loss';

export interface EffectLine {
  tone: EffectTone;
  key: string;
  args: Array<string | number>;
}

export interface EffectContext {
  kronnExists: boolean;
  /** Native file the action reads, when the row has several copies. */
  nativePath?: string;
  formatDate: (iso?: string) => string;
}

const PREFIX = 'projects.repositoryResources.effect.';
const line = (tone: EffectTone, key: string, ...args: Array<string | number>): EffectLine => (
  { tone, key: `${PREFIX}${key}`, args }
);

export const writesToRepository = (kind: TransferKind): boolean => (
  kind === 'publish' || kind === 'update_repository' || kind === 'publish_selected'
);

/** Every path a repository write would touch, shared scaffold included. */
export function writtenPaths(rows: ResourceRow[]): string[] {
  return [...new Set(rows.flatMap(row => (row.writePreview.length > 0 ? row.writePreview : [row.targetPath])))]
    .filter(Boolean)
    .sort();
}

/** The sentence(s) shown before a transfer: what is written and where,
 *  whether a commit is left to do, what runs, and what gets replaced. */
export function describeTransfer(plan: TransferPlan, context: EffectContext): EffectLine[] {
  const [row] = plan.rows;
  if (writesToRepository(plan.kind)) {
    const lines = [line('write', 'write', writtenPaths(plan.rows).join(', '))];
    if (!context.kronnExists) lines.push(line('write', 'createsFolder'));
    lines.push(line('commit', 'commitTodo'), line('activation', 'publishNoActivation'));
    lines.push(plan.kind === 'update_repository'
      ? line('loss', 'lossRepository', context.formatDate(row.repositoryUpdatedAt))
      : line('loss', 'lossNone'));
    return lines;
  }
  if (plan.kind === 'import' || plan.kind === 'update_kronn') {
    const path = row.displayPath;
    return [
      line('write', 'repositoryUntouched'),
      line('write', plan.kind === 'import' ? 'kronnCreates' : 'kronnReplaces', row.name, path),
      line('commit', 'commitNone'),
      line('activation', isExecutable(row.kind) ? 'needsApproval' : 'noExecution'),
      plan.kind === 'update_kronn'
        ? line('loss', 'lossKronn', context.formatDate(row.kronnUpdatedAt))
        : line('loss', 'lossNone'),
    ];
  }
  const nativePath = context.nativePath ?? row.displayPath;
  if (plan.kind === 'use_native') {
    return [
      line('write', 'usePath', nativePath),
      line('commit', 'commitNone'),
      line('activation', 'useTracked'),
      line('loss', 'lossNone'),
    ];
  }
  if (plan.kind === 'copy_native') {
    return [
      line('write', 'copyPath', nativePath),
      line('commit', 'commitNone'),
      line('activation', 'copyActivation'),
      line('loss', 'lossNone'),
    ];
  }
  return [
    line('write', 'attach', row.name),
    line('commit', 'attachCommit'),
    line('activation', 'attachActivation'),
    line('loss', 'lossNone'),
  ];
}
