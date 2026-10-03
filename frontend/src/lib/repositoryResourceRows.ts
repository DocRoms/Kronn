import type {
  ProjectRepositoryResource,
  ProjectRepositoryResourceKind,
  ProjectRepositoryResourceStatus,
  ProjectRepositoryResources,
  ProjectRepositorySkill,
  RepositoryResourceFieldDiff,
  RepositoryResourceFileDiff,
  RequiredSecretStatus,
} from '../types/generated';
import type { ProjectRepositoryResourcesTab } from './projectRepositoryResourcesTab';

export type RowGroup = ProjectRepositoryResourcesTab;

/** `catalog` = a Kronn catalog skill unrelated to the repository's stack;
 *  everything else is the backend status, always defined. */
export type SyncState = ProjectRepositoryResourceStatus | 'catalog';
export type Presence = 'repository' | 'kronn' | 'both';
export type KronnScope = 'catalog' | 'attached' | 'referenced' | 'absent' | 'suggested';
export type PresenceFilter = 'all' | Presence;

export type PrimaryAction =
  | 'use_native'
  | 'import'
  | 'publish'
  | 'update_kronn'
  | 'update_repository'
  | 'compare'
  | 'approve'
  | 'attach'
  | 'view';

export interface ResourceRow {
  key: string;
  group: RowGroup;
  kind: ProjectRepositoryResourceKind;
  id: string;
  slug: string;
  name: string;
  description: string;
  state: SyncState;
  presence: Presence;
  /** Repository files that exist; empty when the resource is not written yet. */
  paths: string[];
  /** The path shown in the Dépôt column: the main existing file, else the target. */
  displayPath: string;
  pathExists: boolean;
  targetPath: string;
  origins: string[];
  pathsDiverge: boolean;
  scope: KronnScope;
  builtin: boolean;
  /** A catalog skill that is not attached to the project yet: found in a
   *  repository folder, proposed for the stack, or simply in the catalog. */
  attachOnly: boolean;
  /** Proposed from the detected stack; never an item to process. */
  suggested: boolean;
  /** The detected file that triggered the suggestion (`Dockerfile`…). */
  suggestedReason?: string;
  repositoryFingerprint?: string;
  kronnFingerprint?: string;
  primary: PrimaryAction;
  approvalRequired: boolean;
  approved: boolean;
  requiredSecrets: RequiredSecretStatus[];
  writePreview: string[];
  diff?: string;
  fileDiffs: RepositoryResourceFileDiff[];
  fieldDiff: RepositoryResourceFieldDiff[];
  repositoryUpdatedAt?: string;
  repositoryUpdatedBy?: string;
  kronnUpdatedAt?: string;
  alignedAt?: string;
  level?: ProjectRepositoryResource['level'];
}

export interface RepositoryRows {
  skills: ResourceRow[];
  catalog: ResourceRow[];
  automation: ResourceRow[];
  artifacts: ResourceRow[];
}

export const EXECUTABLE_KINDS: ProjectRepositoryResourceKind[] = [
  'workflow',
  'quick_prompt',
  'quick_exec',
  'quick_api',
];

export const resourceKey = (kind: ProjectRepositoryResourceKind, id: string) => `${kind}:${id}`;

/** Label of the folder a repository path lives in: `kronn/` or `.claude`… */
export function originLabel(path: string): string {
  if (path.startsWith('kronn/')) return 'kronn/';
  const first = path.split('/')[0];
  return first || path;
}

/** How much of a path's end stays visible when its middle is ellipsized. */
const PATH_TAIL = 18;

/** Head and tail of a path so CSS can ellipsize the middle: the start
 *  (`kronn/workflows/`) shrinks with an ellipsis while the end of the file
 *  name always stays visible. A short file name stays whole in the tail. */
export function splitPath(path: string): { head: string; tail: string } {
  const slash = path.lastIndexOf('/');
  const file = path.slice(Math.max(slash, 0));
  if (file.length <= PATH_TAIL) return { head: path.slice(0, path.length - file.length), tail: file };
  return { head: path.slice(0, path.length - PATH_TAIL), tail: path.slice(-PATH_TAIL) };
}

function mainPath(kind: ProjectRepositoryResourceKind, paths: string[]): string | undefined {
  if (kind === 'artifact') return paths.find(path => path.endsWith('/index.html')) ?? paths[0];
  return paths[0];
}

function presenceOf(state: SyncState, referenced: boolean): Presence {
  if (state === 'repository_only') return 'repository';
  if (state === 'native_skill') return referenced ? 'both' : 'repository';
  if (state === 'kronn_only' || state === 'catalog') return 'kronn';
  return 'both';
}

function primaryOf(state: SyncState, attachOnly: boolean): PrimaryAction {
  switch (state) {
    case 'repository_only': return attachOnly ? 'attach' : 'import';
    case 'native_skill': return 'use_native';
    case 'kronn_only': return attachOnly ? 'attach' : 'publish';
    case 'repository_newer': return 'update_kronn';
    case 'kronn_newer': return 'update_repository';
    case 'conflict': return 'compare';
    case 'approval_required': return 'approve';
    case 'catalog': return 'attach';
    default: return 'view';
  }
}

function resourceRow(resource: ProjectRepositoryResource): ResourceRow {
  const state: SyncState = resource.status;
  const declared = resource.repository_paths;
  const paths = state === 'kronn_only' ? [] : declared;
  const targetPath = mainPath(resource.kind, declared) ?? '';
  const displayPath = mainPath(resource.kind, paths) ?? targetPath;
  return {
    key: resourceKey(resource.kind, resource.id),
    group: resource.kind === 'artifact' ? 'artifacts' : 'automation',
    kind: resource.kind,
    id: resource.id,
    slug: resource.slug,
    name: resource.name,
    description: '',
    state,
    presence: presenceOf(state, false),
    paths,
    displayPath,
    pathExists: paths.length > 0,
    targetPath,
    origins: [...new Set(paths.map(originLabel))],
    pathsDiverge: false,
    scope: resource.id.startsWith('repository:') ? 'absent' : 'attached',
    builtin: false,
    attachOnly: false,
    suggested: false,
    primary: primaryOf(state, false),
    approvalRequired: resource.approval_required,
    approved: resource.approved,
    requiredSecrets: resource.required_secrets ?? [],
    writePreview: resource.write_preview ?? [],
    diff: resource.diff,
    fileDiffs: resource.file_diffs ?? [],
    fieldDiff: resource.field_diff ?? [],
    repositoryUpdatedAt: resource.repository_updated_at,
    repositoryUpdatedBy: resource.repository_updated_by,
    kronnUpdatedAt: resource.kronn_updated_at,
    alignedAt: resource.aligned_at,
    repositoryFingerprint: resource.repository_fingerprint,
    kronnFingerprint: resource.kronn_fingerprint,
    level: resource.level,
  };
}

function skillRow(skill: ProjectRepositorySkill, available: boolean): ResourceRow {
  const nativeFolder = skill.status === 'repository_only' && !skill.id.startsWith('repository:');
  const attachOnly = available || skill.suggested || nativeFolder;
  let state: SyncState;
  if (available) state = 'catalog';
  else if (skill.status === 'native_skill' && skill.referenced) state = 'up_to_date';
  else state = skill.status;
  // A skill only in Kronn has no repository file to show: attached, it is
  // written to its publication path; suggested or in the catalog, it is not
  // written at all until someone attaches it.
  const paths = state === 'kronn_only' || available ? [] : skill.repository_paths;
  const targetPath = skill.publication_path;
  const linked = skill.provenance !== 'repository';
  let scope: KronnScope = 'absent';
  if (available) scope = 'catalog';
  else if (skill.suggested) scope = 'suggested';
  else if (skill.referenced) scope = 'referenced';
  else if (linked) scope = 'attached';
  else if (nativeFolder) scope = 'catalog';
  const shownPath = available || skill.suggested ? '' : (paths[0] ?? targetPath);
  return {
    key: `skill:${skill.id}`,
    group: 'skills',
    kind: 'skill',
    id: skill.id,
    slug: skill.slug,
    name: skill.name,
    description: skill.description,
    state,
    presence: presenceOf(state, skill.referenced),
    paths,
    displayPath: shownPath,
    pathExists: paths.length > 0,
    targetPath,
    origins: [...new Set(paths.map(originLabel))],
    pathsDiverge: skill.repository_paths_diverge,
    scope,
    builtin: Boolean(skill.is_builtin),
    attachOnly,
    suggested: skill.suggested,
    suggestedReason: skill.suggested_reason,
    primary: state === 'up_to_date' && skill.referenced ? 'view' : primaryOf(state, attachOnly),
    approvalRequired: skill.approval_required,
    approved: skill.approved,
    requiredSecrets: skill.required_secrets ?? [],
    writePreview: skill.write_preview ?? [],
    diff: skill.diff,
    fileDiffs: skill.file_diffs ?? [],
    fieldDiff: [],
    repositoryUpdatedAt: skill.repository_updated_at,
    repositoryUpdatedBy: skill.repository_updated_by,
    kronnUpdatedAt: skill.kronn_updated_at,
    alignedAt: skill.aligned_at,
    repositoryFingerprint: skill.repository_fingerprint,
    kronnFingerprint: skill.kronn_fingerprint,
    level: 'usable_without_kronn',
  };
}

const byName = (left: ResourceRow, right: ResourceRow) => left.name.localeCompare(right.name);

export function buildRows(data: ProjectRepositoryResources): RepositoryRows {
  const resources = data.resources.map(resourceRow);
  return {
    skills: data.skills_present.map(skill => skillRow(skill, false)),
    catalog: data.skills_available.map(skill => skillRow(skill, true)),
    automation: resources.filter(row => row.group === 'automation').sort(byName),
    artifacts: resources.filter(row => row.group === 'artifacts').sort(byName),
  };
}

export const allRows = (rows: RepositoryRows): ResourceRow[] => [
  ...rows.skills,
  ...rows.catalog,
  ...rows.automation,
  ...rows.artifacts,
];

/** Ids of the skills already attached to the project — the list a
 *  `default-skills` update must resend, since it replaces the whole set. */
export const attachedSkillIds = (data: ProjectRepositoryResources): string[] => (
  data.skills_present
    .filter(skill => skill.provenance !== 'repository' && !skill.suggested)
    .map(skill => skill.id)
);

export type AttentionReason = 'conflict' | 'approval' | 'late' | 'new';

const ATTENTION_RANK: Record<AttentionReason, number> = {
  conflict: 0,
  approval: 1,
  late: 2,
  new: 3,
};

/** Only what asks for a sync or safety decision. A skill the stack merely
 *  suggests, or a catalog skill not attached yet, is never one. */
export function attentionReason(row: ResourceRow): AttentionReason | null {
  switch (row.state) {
    case 'conflict': return 'conflict';
    case 'approval_required': return 'approval';
    case 'repository_newer':
    case 'kronn_newer': return 'late';
    case 'native_skill': return 'new';
    case 'repository_only': return row.attachOnly ? null : 'new';
    default: return null;
  }
}

export interface AttentionItem {
  row: ResourceRow;
  reason: AttentionReason;
}

/** Rows that need a decision, most urgent first: two versions, then waiting
 *  for approval, then out of date, then newly found in the repository. Each
 *  sub-tab has its own list; without `group`, every sub-tab's. */
export function attentionItems(rows: RepositoryRows, group?: RowGroup): AttentionItem[] {
  return allRows(rows)
    .filter(row => group === undefined || row.group === group)
    .flatMap(row => {
      const reason = attentionReason(row);
      return reason ? [{ row, reason }] : [];
    })
    .sort((left, right) => (
      ATTENTION_RANK[left.reason] - ATTENTION_RANK[right.reason] || byName(left.row, right.row)
    ));
}

export const attentionCount = (data: ProjectRepositoryResources): number => (
  attentionItems(buildRows(data)).length
);

export function matchesPresence(row: ResourceRow, filter: PresenceFilter): boolean {
  return filter === 'all' || row.presence === filter;
}

export function matchesQuery(row: ResourceRow, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return [row.name, row.slug, row.targetPath, ...row.paths]
    .some(value => value.toLowerCase().includes(needle));
}

export type AlignDirection = 'to_kronn' | 'to_repository';

export interface AlignLine {
  row: ResourceRow;
  direction: AlignDirection;
}

/** What "Align all" may move: only what is behind on one side, in the
 *  direction of the newer copy. Two versions and approvals each need their own
 *  decision; something present on one side only is written by selecting it. */
export function alignLines(rows: RepositoryRows, group?: RowGroup): AlignLine[] {
  return [...rows.skills, ...rows.automation, ...rows.artifacts]
    .filter(row => group === undefined || row.group === group)
    .flatMap((row): AlignLine[] => {
      if (row.approvalRequired) return [];
      if (row.state === 'repository_newer') return [{ row, direction: 'to_kronn' }];
      if (row.state === 'kronn_newer') return [{ row, direction: 'to_repository' }];
      return [];
    });
}

/** Rows "Align all" leaves aside because each holds a decision of its own. */
export const alignExcludedCount = (rows: RepositoryRows, group?: RowGroup): number => (
  [...rows.skills, ...rows.automation, ...rows.artifacts]
    .filter(row => (group === undefined || row.group === group)
      && (row.state === 'conflict' || row.state === 'approval_required'))
    .length
);

export const writesRepository = (action: PrimaryAction): boolean => (
  action === 'publish' || action === 'update_repository'
);

export const isExecutable = (kind: ProjectRepositoryResourceKind): boolean => (
  EXECUTABLE_KINDS.includes(kind)
);
