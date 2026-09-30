// What the discussion skill pickers (new discussion, discussion settings) list,
// and in which order (KT-923). Same reading as the Automation sidebar (KT-921):
// the skills the discussion's project *uses* come first, the ones already
// ticked next, and the rest of the catalog waits by category, folded away.
// Pure, so both pickers and the tests read one definition.
import type { Project, ProjectUsedSkill, Skill, SkillCategory } from '../types/generated';
import { automationSkillEntries, isRepositorySkillId } from './automationSkills';
import type { AutomationSkillEntry } from './automationSkills';

type PickerProject = Pick<Project, 'id' | 'name' | 'default_skill_ids'>;

/** Categories in the order the picker lists them. */
export const SKILL_CATEGORY_ORDER: readonly SkillCategory[] = ['Language', 'Domain', 'Business'];

export interface SkillCategoryGroup {
  category: SkillCategory;
  entries: AutomationSkillEntry[];
}

export interface SkillPickerModel {
  /** Used by the discussion's project — attached, referenced or published. Empty
   *  for a discussion with no project. */
  used: AutomationSkillEntry[];
  /** Ticked, and not already listed above. */
  ticked: AutomationSkillEntry[];
  /** Ticked ids nothing answers to any more (a skill gone from the repository,
   *  or belonging to another project): shown so they can be unticked. */
  missing: string[];
  /** The rest of the catalog, by category. With a project it waits behind
   *  "Voir les skills disponibles"; without one it is the whole picker. */
  available: SkillCategoryGroup[];
  availableCount: number;
  /** Whether the available skills sit behind a fold: only when there is a
   *  project whose own skills come first. */
  foldAvailable: boolean;
  /** Every skill the picker can show, whatever the query. */
  total: number;
}

export interface SkillPickerInput {
  catalog: readonly Skill[];
  project: PickerProject | null;
  usedSkills: readonly ProjectUsedSkill[];
  selectedIds: readonly string[];
  query: string;
  /** Whether the lists were read: until then a ticked id is not "missing", it
   *  is simply not loaded yet. */
  loaded: boolean;
}

/** Alphabetical by the name the list shows. */
function byName(left: AutomationSkillEntry, right: AutomationSkillEntry): number {
  return left.skill.name.localeCompare(right.skill.name, undefined, { sensitivity: 'base' });
}

/** A skill matches a query on its name, its description and its id (the folder
 *  name for a repository skill), and on the folder it lives in. */
export function skillMatchesQuery(entry: AutomationSkillEntry, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return [entry.skill.name, entry.skill.description, entry.id, entry.repository?.root ?? '']
    .some(text => text.toLowerCase().includes(needle));
}

export function skillPickerModel(input: SkillPickerInput): SkillPickerModel {
  const { catalog, project, usedSkills, query, loaded } = input;
  const ticked = new Set(input.selectedIds);
  // Only what this project uses: another project's repository skills have no
  // business in its discussion.
  const entries = automationSkillEntries(
    catalog,
    project ? [project] : [],
    project ? usedSkills.filter(used => used.project_id === project.id) : [],
  );
  const shown = entries.filter(entry => skillMatchesQuery(entry, query));

  const used = project ? shown.filter(entry => entry.used).sort(byName) : [];
  const usedIds = new Set(used.map(entry => entry.id));
  const tickedEntries = project
    ? shown.filter(entry => ticked.has(entry.id) && !usedIds.has(entry.id)).sort(byName)
    : [];
  const listedAbove = new Set([...usedIds, ...tickedEntries.map(entry => entry.id)]);
  const rest = shown.filter(entry => !listedAbove.has(entry.id));

  const known = new Set(entries.map(entry => entry.id));
  const needle = query.trim().toLowerCase();
  const missing = loaded
    ? input.selectedIds.filter(id => !known.has(id) && id.toLowerCase().includes(needle))
    : [];

  // A skill whose category this build does not know is listed under Domain
  // rather than left out: the catalog decides what exists, not this list.
  const categoryOf = (entry: AutomationSkillEntry): SkillCategory => (
    SKILL_CATEGORY_ORDER.includes(entry.skill.category) ? entry.skill.category : 'Domain'
  );
  const available = SKILL_CATEGORY_ORDER
    .map(category => ({
      category,
      entries: rest.filter(entry => categoryOf(entry) === category).sort(byName),
    }))
    .filter(group => group.entries.length > 0);

  return {
    used,
    ticked: tickedEntries,
    missing,
    available,
    availableCount: rest.length,
    foldAvailable: project !== null,
    total: entries.length,
  };
}

/** `ids` without the repository skills of any other project than `projectId`
 *  (`''` for none): a skill only one repository holds means nothing in another
 *  project's discussion. Catalog skills are kept. */
export function withoutForeignRepositorySkills(ids: readonly string[], projectId: string): string[] {
  const own = `repository:${projectId}:`;
  return ids.filter(id => !isRepositorySkillId(id) || (projectId !== '' && id.startsWith(own)));
}

/** The label of an id nothing answers to: the folder name of a repository
 *  skill, the id itself otherwise. */
export function missingSkillLabel(id: string): string {
  if (!isRepositorySkillId(id)) return id;
  return id.slice(id.lastIndexOf(':') + 1) || id;
}
