import { describe, expect, it } from 'vitest';
import type { SkillMigrationPlan } from '../../types/generated';
import {
  MIGRATION_SOURCE_ROOTS,
  migratableSkillCount,
  migrationSize,
  resolutionsOf,
  unresolvedConflicts,
} from '../skillMigration';

const plan: SkillMigrationPlan = {
  target_root: '.agents/skills',
  moves: [{
    slug: 'lint',
    source: '.gemini/skills/lint',
    target: '.agents/skills/lint',
    action: 'move',
    converted: false,
    kronn_managed: false,
  }],
  conflicts: [{
    slug: 'review',
    target: '.agents/skills/review',
    versions: [
      { fingerprint: 'aaaaaaaa', paths: ['.agents/skills/review'], at_target: true },
      { fingerprint: 'bbbbbbbb', paths: ['.claude/skills/review', '.cursor/skills/review'], at_target: false },
    ],
  }],
  blocked: [],
};

describe('migratableSkillCount', () => {
  it('counts the skills outside .agents/skills, kronn/skills included', () => {
    expect(migratableSkillCount([
      { path: '.agents/skills', skill_count: 4 },
      { path: '.claude/skills', skill_count: 2 },
      { path: 'kronn/skills', skill_count: 1 },
      { path: '.vibe/skills', skill_count: 7 },
    ])).toBe(3);
  });

  it('is zero when everything already sits in .agents/skills', () => {
    expect(migratableSkillCount([{ path: '.agents/skills', skill_count: 4 }])).toBe(0);
    expect(migratableSkillCount([])).toBe(0);
  });

  it('gathers from every native folder the task names and never from the target', () => {
    for (const root of ['.claude/skills', '.gemini/skills', '.codex/skills', '.github/skills', '.opencode/skill', '.opencode/skills', '.cursor/skills', 'kronn/skills']) {
      expect(MIGRATION_SOURCE_ROOTS).toContain(root);
    }
    expect(MIGRATION_SOURCE_ROOTS).not.toContain('.agents/skills');
  });
});

describe('conflict choices', () => {
  it('never picks a version: no choice means no resolution', () => {
    expect(resolutionsOf(plan, {})).toEqual([]);
    expect(unresolvedConflicts(plan, {})).toBe(1);
    expect(migrationSize(plan, {})).toBe(1);
  });

  it('turns a choice into a resolution and counts the conflict as settled', () => {
    const choices = { review: '.claude/skills/review' };
    expect(resolutionsOf(plan, choices)).toEqual([{ slug: 'review', keep: '.claude/skills/review' }]);
    expect(unresolvedConflicts(plan, choices)).toBe(0);
    expect(migrationSize(plan, choices)).toBe(2);
  });

  it('drops a choice that names no folder of the conflict or no conflict at all', () => {
    expect(resolutionsOf(plan, { review: '../elsewhere', ghost: '.claude/skills/ghost' })).toEqual([]);
  });
});
