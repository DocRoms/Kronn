// Skills on the global Automation page (KT-914). A skill is not an automation:
// it has no run, no variables and no server-side favorite or modification date.
// What the page needs from it lives here, pure, so the sidebar, the sheet and
// the tests read one definition.
import type { Project, ProjectUsedSkill, Skill } from '../types/generated';

export const SKILL_FAVORITES_STORAGE_KEY = 'kronn:automationSkillFavorites';

/** A skill carries no modification date. Every skill shares this one, which
 *  ranks it after every dated automation when the list is sorted by last
 *  change, without ever breaking the order of the others (an unparseable date
 *  would). */
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

/** A skill is "Variabilisé" when it declares Claude Code `arguments` (KT-906). */
export function isVariabilizedSkill(skill: Pick<Skill, 'arguments'>): boolean {
  return (skill.arguments?.length ?? 0) > 0;
}

/** Where the skill comes from: the built-in catalog, a third-party skill
 *  vendored into it, or one the user wrote. */
export function skillOrigin(skill: Pick<Skill, 'is_builtin' | 'external'>): 'kronn' | 'personal' | 'external' {
  return skill.external ? 'external' : skill.is_builtin ? 'kronn' : 'personal';
}

/** Id of a skill only a project's repository holds. Scoped by project: the same
 *  folder name in two repositories is two skills, with two SKILL.md. */
export function repositorySkillId(projectId: string, slug: string): string {
  return `repository:${projectId}:${slug}`;
}

/** The body without a leading frontmatter: the backend writes it from the fields. */
export function skillBody(content: string): string {
  return content.replace(/^---\s*\n[\s\S]*?\n---\s*\n?/, '');
}

/** A skill scoped to a project is offered there only; a global one everywhere. */
export function offeredToProject(skill: Pick<Skill, 'project_id'>, projectId: string | null | undefined): boolean {
  return !skill.project_id || skill.project_id === projectId;
}

/** The skills only `projectId`'s repository holds, as a skill picker lists them. */
export function repositorySkillsOf(usedSkills: readonly ProjectUsedSkill[], projectId: string): Skill[] {
  return usedSkills
    .filter(used => used.project_id === projectId && !used.skill_id)
    .map(repositorySkill);
}

/** The ids already picked that `offered` does not list, as skills a picker can
 *  show to remove them: another project's, or one that no longer exists. */
export function pickedButNotOffered(
  pickedIds: readonly string[],
  offered: readonly Skill[],
  catalog: readonly Skill[],
  usedSkills: readonly ProjectUsedSkill[],
): Skill[] {
  const listed = new Set(offered.map(skill => skill.id));
  const known = new Map<string, Skill>([
    ...usedSkills.filter(used => !used.skill_id).map(used => {
      const skill = repositorySkill(used);
      return [skill.id, skill] as const;
    }),
    ...catalog.map(skill => [skill.id, skill] as const),
  ]);
  return pickedIds
    .filter(id => !listed.has(id))
    .map(id => known.get(id) ?? {
      id,
      name: isRepositorySkillId(id) ? id.slice(id.lastIndexOf(':') + 1) || id : id,
      description: '',
      icon: '❔',
      category: 'Domain',
      content: '',
      is_builtin: false,
      token_estimate: 0,
    });
}

export function isRepositorySkillId(id: string): boolean {
  return id.startsWith('repository:');
}

/** Where a repository-held skill lives, and why the project uses it. */
export interface RepositorySkillOrigin {
  projectId: string;
  /** The native skill folder holding it (`.agents/skills`). */
  root: string;
  /** Repository-relative path of its SKILL.md. */
  relativePath: string;
  /** "Use in Kronn" pointed at it. */
  referenced: boolean;
  /** `kronn.lock` lists it: Kronn wrote it into the repository. */
  published: boolean;
}

/** A skill as the Automation sidebar lists it: from the Kronn catalog, or one
 *  only a repository holds. */
export interface AutomationSkillEntry {
  /** The catalog skill id, or `repositorySkillId`. */
  id: string;
  skill: Skill;
  /** Every project using it, whichever way (attached, referenced, published). */
  projectIds: string[];
  /** Set for a skill only a repository holds. */
  repository?: RepositorySkillOrigin;
  /** Used by at least one project. The others are only *available*: the
   *  sidebar folds them away instead of listing the whole catalog. */
  used: boolean;
}

function repositorySkill(used: ProjectUsedSkill): Skill {
  // A repository skill has no catalog row: the sheet reads its SKILL.md from
  // the repository, and the rest of a catalog skill's fields do not apply.
  return {
    id: repositorySkillId(used.project_id, used.slug),
    name: used.name || used.slug,
    description: '',
    icon: '📂',
    category: 'Domain',
    content: '',
    is_builtin: false,
    token_estimate: 0,
  };
}

/**
 * The skills the sidebar lists, with the projects using each of them.
 *
 * A project uses a skill when it lists it among its default skills, when
 * "Use in Kronn" points it at a native skill of its repository (KT-897), or
 * when Kronn published it into the repository (`kronn.lock`). A catalog skill
 * used by no project is kept, marked `used: false`. A native skill the catalog
 * does not hold is listed under its project only; a project the page does not
 * know cannot use anything.
 */
export function automationSkillEntries(
  catalog: readonly Skill[],
  projects: readonly ProjectSkills[],
  usedSkills: readonly ProjectUsedSkill[],
): AutomationSkillEntry[] {
  const known = new Set(projects.map(project => project.id));
  const publishedBy = new Map<string, Set<string>>();
  const repositoryOnly: ProjectUsedSkill[] = [];
  for (const used of usedSkills) {
    if (!known.has(used.project_id)) continue;
    if (used.skill_id) {
      const ids = publishedBy.get(used.skill_id) ?? new Set<string>();
      ids.add(used.project_id);
      publishedBy.set(used.skill_id, ids);
    } else {
      repositoryOnly.push(used);
    }
  }
  const entries: AutomationSkillEntry[] = catalog.map(skill => {
    const using = new Set([
      ...projectsUsingSkill(skill.id, projects).map(project => project.id),
      ...(publishedBy.get(skill.id) ?? []),
      ...(skill.project_id ? [skill.project_id] : []),
    ]);
    // Kept in the order the page received the projects.
    const projectIds = projects.filter(project => using.has(project.id)).map(project => project.id);
    return { id: skill.id, skill, projectIds, used: projectIds.length > 0 };
  });
  for (const used of repositoryOnly) {
    const skill = repositorySkill(used);
    entries.push({
      id: skill.id,
      skill,
      projectIds: [used.project_id],
      repository: {
        projectId: used.project_id,
        root: used.root,
        relativePath: used.relative_path,
        referenced: used.referenced,
        published: used.published,
      },
      used: true,
    });
  }
  return entries;
}
