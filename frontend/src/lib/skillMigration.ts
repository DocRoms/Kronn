import type {
  ProjectSkillRoot,
  SkillMigrationPlan,
  SkillMigrationResolution,
} from '../types/generated';

/** The one folder every agent reads skills from, and so where they gather. */
export const SKILLS_TARGET_ROOT = '.agents/skills';

/** The folders "Migrate everything to .agents/skills" gathers skills from —
 *  the same list the backend reads, `kronn/skills` (where Kronn used to write
 *  them) included. */
export const MIGRATION_SOURCE_ROOTS = [
  '.claude/skills',
  '.gemini/skills',
  '.codex/skills',
  '.github/skills',
  '.opencode/skill',
  '.opencode/skills',
  '.cursor/skills',
  'kronn/skills',
];

/** How many skills sit in a folder other than `.agents/skills`: what the tab
 *  offers to bring together. */
export function migratableSkillCount(roots: ProjectSkillRoot[]): number {
  return roots
    .filter(root => MIGRATION_SOURCE_ROOTS.includes(root.path))
    .reduce((total, root) => total + root.skill_count, 0);
}

/** What the user picked for each conflict: slug → the folder whose version is
 *  kept. A conflict absent from it has no choice yet. */
export type MigrationChoices = Record<string, string>;

/** The choices as the request wants them — only for conflicts the plan still
 *  lists, and only for a folder one of its versions actually holds. */
export function resolutionsOf(plan: SkillMigrationPlan, choices: MigrationChoices): SkillMigrationResolution[] {
  return plan.conflicts.flatMap(conflict => {
    const keep = choices[conflict.slug];
    const listed = conflict.versions.some(version => version.paths.includes(keep));
    return keep && listed ? [{ slug: conflict.slug, keep }] : [];
  });
}

/** Conflicts nothing was chosen for: they are skipped, never guessed. */
export const unresolvedConflicts = (plan: SkillMigrationPlan, choices: MigrationChoices): number => (
  plan.conflicts.length - resolutionsOf(plan, choices).length
);

/** Skill folders the confirm button would act on: every move, and every
 *  conflict that has a choice. */
export const migrationSize = (plan: SkillMigrationPlan, choices: MigrationChoices): number => (
  plan.moves.length + resolutionsOf(plan, choices).length
);
