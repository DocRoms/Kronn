// Skills on the global Automation page (KT-914). A skill is not an automation:
// it has no run, no variables and no server-side favorite or modification date.
// What the page needs from it lives here, pure, so the sidebar, the sheet and
// the tests read one definition.
import type { Project, Skill } from '../types/generated';

export const SKILL_FAVORITES_STORAGE_KEY = 'kronn:automationSkillFavorites';

/** A skill carries no modification date. Every skill shares this one, which
 *  ranks it after every dated automation in "Recent" without ever breaking the
 *  order of the others (an unparseable date would). */
export const SKILL_UPDATED_AT = '1970-01-01T00:00:00.000Z';

type ProjectSkills = Pick<Project, 'id' | 'name' | 'default_skill_ids'>;

/** The projects that list this skill among their default skills, in the order
 *  the page received them. */
export function projectsUsingSkill<P extends ProjectSkills>(skillId: string, projects: readonly P[]): P[] {
  return projects.filter(project => (project.default_skill_ids ?? []).includes(skillId));
}

/** A skill's favorites live in this browser: the backend has no pin for a
 *  skill, and the star must still do what it does on every other row. */
export function readSkillFavorites(): Set<string> {
  try {
    const parsed = JSON.parse(localStorage.getItem(SKILL_FAVORITES_STORAGE_KEY) ?? '[]') as unknown;
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((value): value is string => typeof value === 'string'));
  } catch {
    return new Set();
  }
}

export function writeSkillFavorites(ids: ReadonlySet<string>): void {
  try {
    localStorage.setItem(SKILL_FAVORITES_STORAGE_KEY, JSON.stringify([...ids]));
  } catch {
    // localStorage may be unavailable in private/restricted browser modes.
  }
}

/** Where the skill comes from: the built-in catalog, a third-party skill
 *  vendored into it, or one the user wrote. */
export function skillOrigin(skill: Pick<Skill, 'is_builtin' | 'external'>): 'kronn' | 'personal' | 'external' {
  return skill.external ? 'external' : skill.is_builtin ? 'kronn' : 'personal';
}
