import { describe, expect, it } from 'vitest';
import { buildRows } from '../repositoryResourceRows';
import {
  catalogSkillTraits,
  groupSkills,
  isUnsyncedState,
  resourceRowSkillTraits,
  skillBadgesOf,
  skillGroupLabelKey,
  skillGroupOf,
} from '../skillGroups';
import { automationSkillEntries } from '../automationSkills';
import { listing, skill as repositorySkill } from '../../components/__tests__/repositoryResourceFixtures';
import type { ProjectUsedSkill, Skill } from '../../types/generated';

const catalog = (over: Partial<Skill> & Pick<Skill, 'id'>): Skill => ({
  name: over.id, description: '', icon: '🧩', category: 'Domain', content: '', is_builtin: false, token_estimate: 0, ...over,
});

describe('skillGroups (KT-1140)', () => {
  it('puts a built-in skill in « kronn » and a custom, project or repository one in « mine »', () => {
    expect(skillGroupOf(catalogSkillTraits(catalog({ id: 'rust', is_builtin: true })))).toBe('kronn');
    expect(skillGroupOf(catalogSkillTraits(catalog({ id: 'custom-a' })))).toBe('mine');
    expect(skillGroupOf(catalogSkillTraits(catalog({ id: 'custom-b', project_id: 'p-1' })))).toBe('mine');
    expect(skillGroupOf(catalogSkillTraits(catalog({ id: 'repository:p-1:x' }), { root: '.agents/skills' }))).toBe('mine');
  });

  it('classifies a skill the same way on the project card and on the Automation page', () => {
    const rows = buildRows(listing({
      skills_present: [
        repositorySkill({ id: 'rust', name: 'Rust', is_builtin: true }),
        repositorySkill({ id: 'custom-team', name: 'Team', provenance: 'kronn', status: 'kronn_only', project_owned: true }),
        repositorySkill({ id: 'repository:p-1:block', name: 'block', provenance: 'repository', status: 'native_skill', referenced: true }),
      ],
    }));
    const card = Object.fromEntries(rows.skills.map(row => [row.id, resourceRowSkillTraits(row)]));
    const automation = {
      rust: catalogSkillTraits(catalog({ id: 'rust', is_builtin: true })),
      'custom-team': catalogSkillTraits(catalog({ id: 'custom-team', project_id: 'p-1' })),
      'repository:p-1:block': catalogSkillTraits(catalog({ id: 'repository:p-1:block' }), { root: '.agents/skills' }),
    };
    for (const id of Object.keys(automation) as Array<keyof typeof automation>) {
      expect(skillGroupOf(card[id])).toBe(skillGroupOf(automation[id]));
      expect(skillBadgesOf(card[id])).toEqual(skillBadgesOf(automation[id]));
    }
  });

  const used = (projectId: string, skillId: string, sync_status: ProjectUsedSkill['sync_status']): ProjectUsedSkill => ({
    project_id: projectId, skill_id: skillId, slug: skillId, name: skillId, root: '.agents/skills',
    relative_path: `.agents/skills/${skillId}/SKILL.md`, referenced: false, published: true, sync_status,
  });
  const automationTraits = (catalog: Skill[], projects: Array<{ id: string; name: string; default_skill_ids: string[] }>, usedSkills: ProjectUsedSkill[]) =>
    Object.fromEntries(automationSkillEntries(catalog, projects, usedSkills)
      .map(entry => [entry.id, catalogSkillTraits(entry.skill, entry.repository, entry.syncStates)]));

  it('badges a skill in conflict and one the repository moved « Pas synchro » on both screens', () => {
    const rows = buildRows(listing({
      skills_present: [
        repositorySkill({ id: 'custom-team', name: 'Team', provenance: 'both', status: 'conflict', project_owned: true }),
        repositorySkill({ id: 'rust', name: 'Rust', is_builtin: true, status: 'repository_newer' }),
      ],
    }));
    const card = Object.fromEntries(rows.skills.map(row => [row.id, resourceRowSkillTraits(row)]));
    const automation = automationTraits(
      [catalog({ id: 'custom-team', project_id: 'p-1' }), catalog({ id: 'rust', is_builtin: true })],
      [{ id: 'p-1', name: 'One', default_skill_ids: ['rust'] }],
      [used('p-1', 'custom-team', 'conflict'), used('p-1', 'rust', 'repository_newer')],
    );
    for (const id of ['custom-team', 'rust']) {
      expect(skillBadgesOf(card[id])).toContain('unsynced');
      expect(skillBadgesOf(automation[id])).toEqual(skillBadgesOf(card[id]));
    }
  });

  it('calls a skill out of sync when any one of the projects using it is', () => {
    const projects = [
      { id: 'p-1', name: 'One', default_skill_ids: ['review'] },
      { id: 'p-2', name: 'Two', default_skill_ids: ['review'] },
    ];
    const review = [catalog({ id: 'review' })];
    expect(automationTraits(review, projects, [used('p-1', 'review', 'up_to_date'), used('p-2', 'review', 'conflict')]).review.unsynced)
      .toBe(true);
    expect(automationTraits(review, projects, [used('p-1', 'review', 'up_to_date'), used('p-2', 'review', 'up_to_date')]).review.unsynced)
      .toBe(false);
    // A project the page does not know reports nothing.
    expect(automationTraits(review, projects.slice(0, 1), [used('p-1', 'review', 'up_to_date'), used('p-2', 'review', 'conflict')]).review.unsynced)
      .toBe(false);
    expect(['repository_newer', 'kronn_newer', 'conflict'].every(state => isUnsyncedState(state as never))).toBe(true);
    expect(['up_to_date', 'kronn_only', 'native_skill', undefined].some(state => isUnsyncedState(state as never))).toBe(false);
  });

  it('badges project, repository and out-of-sync in one order', () => {
    expect(skillBadgesOf({ builtin: false, projectOwned: true, repository: true, unsynced: true }))
      .toEqual(['project', 'repository', 'unsynced']);
    expect(skillBadgesOf({ builtin: true, projectOwned: false, repository: false, unsynced: false })).toEqual([]);
  });

  it('lists the non-empty groups, Kronn first, each in its input order', () => {
    const items = ['mine-b', 'kronn-a', 'mine-a'];
    const traits = (id: string) => ({ builtin: id.startsWith('kronn'), projectOwned: false, repository: false, unsynced: false });
    expect(groupSkills(items, traits)).toEqual([
      { group: 'kronn', items: ['kronn-a'] },
      { group: 'mine', items: ['mine-b', 'mine-a'] },
    ]);
    expect(groupSkills(['mine-a'], traits).map(entry => entry.group)).toEqual(['mine']);
    expect(skillGroupLabelKey('mine')).toBe('skills.group.mine');
  });
});
