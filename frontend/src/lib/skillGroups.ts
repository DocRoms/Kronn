// Kronn's built-in skills versus the ones the user made (KT-1140). The
// project card and the Automation page both group and badge skills from here.
import type { Skill } from '../types/generated';
import type { ResourceRow } from './repositoryResourceRows';

export type SkillGroup = 'kronn' | 'mine';
export type SkillBadge = 'project' | 'repository' | 'unsynced';

/** The order both screens list the groups in. */
export const SKILL_GROUP_ORDER: readonly SkillGroup[] = ['kronn', 'mine'];
const BADGE_ORDER: readonly SkillBadge[] = ['project', 'repository', 'unsynced'];

/** What decides a skill's group and badges, whichever screen lists it. */
export interface SkillTraits {
  /** Shipped with Kronn (vendored third-party skills included). */
  builtin: boolean;
  /** A custom skill scoped to one project. */
  projectOwned: boolean;
  /** Held by a repository only, with no catalog counterpart. */
  repository: boolean;
  /** Kronn and the repository hold different versions. */
  unsynced: boolean;
}

/** The i18n key of a group's heading, the same on both screens. */
export function skillGroupLabelKey(group: SkillGroup): string {
  return `skills.group.${group}`;
}

export function skillGroupOf(traits: SkillTraits): SkillGroup {
  return traits.builtin ? 'kronn' : 'mine';
}

export function skillBadgesOf(traits: SkillTraits): SkillBadge[] {
  const has: Record<SkillBadge, boolean> = {
    project: traits.projectOwned,
    repository: traits.repository,
    unsynced: traits.unsynced,
  };
  return BADGE_ORDER.filter(badge => has[badge]);
}

/** The non-empty groups in `SKILL_GROUP_ORDER`, each keeping the input order. */
export function groupSkills<T>(
  items: readonly T[],
  traitsOf: (item: T) => SkillTraits,
): Array<{ group: SkillGroup; items: T[] }> {
  return SKILL_GROUP_ORDER
    .map(group => ({ group, items: items.filter(item => skillGroupOf(traitsOf(item)) === group) }))
    .filter(entry => entry.items.length > 0);
}

const UNSYNCED_STATES: ReadonlySet<ResourceRow['state']> = new Set(['repository_newer', 'kronn_newer', 'conflict']);

/** Kronn and the repository hold different versions, on one project. */
export function isUnsyncedState(state: ResourceRow['state'] | undefined): boolean {
  return state !== undefined && UNSYNCED_STATES.has(state);
}

/** A catalog skill, or one only a repository holds, as the Automation page sees it.
 *  `syncStates` holds one state per project using it: out of sync if any project is. */
export function catalogSkillTraits(
  skill: Pick<Skill, 'is_builtin' | 'project_id'>,
  repository?: unknown,
  syncStates: readonly (ResourceRow['state'] | undefined)[] = [],
): SkillTraits {
  return {
    builtin: skill.is_builtin,
    projectOwned: Boolean(skill.project_id),
    repository: Boolean(repository),
    unsynced: syncStates.some(isUnsyncedState),
  };
}

/** A skill row of the project card's "AI & automation" tab. */
export function resourceRowSkillTraits(row: Pick<ResourceRow, 'builtin' | 'scope' | 'state'>): SkillTraits {
  return {
    builtin: row.builtin,
    projectOwned: row.scope === 'project',
    repository: row.scope === 'referenced' || row.scope === 'absent',
    unsynced: isUnsyncedState(row.state),
  };
}
