import { afterEach, describe, expect, it } from 'vitest';
import {
  SKILL_FAVORITES_STORAGE_KEY,
  SKILL_UPDATED_AT,
  automationSkillEntries,
  isRepositorySkillId,
  projectsUsingSkill,
  readSkillFavorites,
  repositorySkillId,
  skillOrigin,
  writeSkillFavorites,
} from '../automationSkills';
import { sortAutomationResources } from '../automationSort';
import type { ProjectUsedSkill, Skill } from '../../types/generated';

afterEach(() => localStorage.removeItem(SKILL_FAVORITES_STORAGE_KEY));

describe('automation skills', () => {
  it('finds the projects that list a skill among their default skills', () => {
    const projects = [
      { id: 'alpha', name: 'Alpha', default_skill_ids: ['review', 'rust'] },
      { id: 'beta', name: 'Beta', default_skill_ids: ['review'] },
      { id: 'gamma', name: 'Gamma', default_skill_ids: [] },
      { id: 'delta', name: 'Delta' },
    ];
    expect(projectsUsingSkill('review', projects).map(project => project.id)).toEqual(['alpha', 'beta']);
    expect(projectsUsingSkill('rust', projects).map(project => project.id)).toEqual(['alpha']);
    expect(projectsUsingSkill('unused', projects)).toEqual([]);
  });

  it('keeps a skill\'s favorites in this browser and survives a corrupted value', () => {
    expect(readSkillFavorites().size).toBe(0);
    writeSkillFavorites(new Set(['review', 'rust']));
    expect([...readSkillFavorites()]).toEqual(['review', 'rust']);
    localStorage.setItem(SKILL_FAVORITES_STORAGE_KEY, '{not json');
    expect(readSkillFavorites().size).toBe(0);
    localStorage.setItem(SKILL_FAVORITES_STORAGE_KEY, JSON.stringify(['ok', 3, null]));
    expect([...readSkillFavorites()]).toEqual(['ok']);
  });

  it('tells where a skill comes from', () => {
    expect(skillOrigin({ is_builtin: true })).toBe('kronn');
    expect(skillOrigin({ is_builtin: true, external: true })).toBe('external');
    expect(skillOrigin({ is_builtin: false })).toBe('personal');
  });

  it('ranks a skill after every dated automation, in a stable order', () => {
    const entry = (name: string, kind: 'workflows' | 'skills', updatedAt: string) => ({ name, kind, pinned: false, updatedAt });
    const sorted = sortAutomationResources([
      entry('Zeta skill', 'skills', SKILL_UPDATED_AT),
      entry('Nightly', 'workflows', '2026-01-01T00:00:00Z'),
      entry('Alpha skill', 'skills', SKILL_UPDATED_AT),
    ], 'updated');
    expect(sorted.map(item => item.name)).toEqual(['Nightly', 'Alpha skill', 'Zeta skill']);
    // Grouped by type, Skills come last.
    expect(sortAutomationResources(sorted, 'kind').map(item => item.name)).toEqual(['Nightly', 'Alpha skill', 'Zeta skill']);
  });
});

describe('the skills the sidebar lists (KT-921)', () => {
  const skill = (id: string, name: string): Skill => ({
    id, name, description: '', icon: '🧩', category: 'Domain', content: '', is_builtin: false, token_estimate: 0,
  });
  const CATALOG = [skill('review', 'Review'), skill('rust', 'Rust'), skill('orphan', 'Orphan')];
  const PROJECTS = [
    { id: 'alpha', name: 'Alpha', default_skill_ids: ['review'] },
    { id: 'beta', name: 'Beta', default_skill_ids: [] },
    { id: 'gamma', name: 'Gamma' },
  ];
  const used = (over: Partial<ProjectUsedSkill>): ProjectUsedSkill => ({
    project_id: 'alpha', slug: 'block-migration', name: 'block-migration',
    root: '.agents/skills', relative_path: '.agents/skills/block-migration/SKILL.md', referenced: true, published: false,
    ...over,
  });
  const byId = (entries: ReturnType<typeof automationSkillEntries>) => Object.fromEntries(entries.map(entry => [entry.id, entry]));

  it('counts a catalog skill as used only when some project attaches it, references it or has it published', () => {
    const entries = byId(automationSkillEntries(CATALOG, PROJECTS, [
      used({ project_id: 'beta', skill_id: 'rust', slug: 'rust', referenced: false, published: true }),
    ]));
    expect(entries.review).toMatchObject({ projectIds: ['alpha'], used: true });
    expect(entries.rust).toMatchObject({ projectIds: ['beta'], used: true });
    expect(entries.orphan).toMatchObject({ projectIds: [], used: false });
  });

  it('lists a skill both attached and published once, under each project once, in the order of the projects', () => {
    const entries = byId(automationSkillEntries(CATALOG, PROJECTS, [
      used({ project_id: 'beta', skill_id: 'review', slug: 'review' }),
      used({ project_id: 'alpha', skill_id: 'review', slug: 'review' }),
    ]));
    expect(entries.review.projectIds).toEqual(['alpha', 'beta']);
  });

  it('lists a native skill the catalog does not hold under its project alone, with where it lives', () => {
    const entries = automationSkillEntries(CATALOG, PROJECTS, [used({})]);
    const native = entries.find(entry => entry.repository);
    expect(native).toMatchObject({
      id: 'repository:alpha:block-migration',
      projectIds: ['alpha'],
      used: true,
      repository: {
        projectId: 'alpha', root: '.agents/skills', relativePath: '.agents/skills/block-migration/SKILL.md',
        referenced: true, published: false,
      },
    });
    expect(native?.skill).toMatchObject({ id: 'repository:alpha:block-migration', name: 'block-migration', is_builtin: false });
    expect(entries.filter(entry => !entry.repository).map(entry => entry.id)).toEqual(['review', 'rust', 'orphan']);
  });

  it('keeps the same native skill of two repositories as two skills', () => {
    const entries = automationSkillEntries([], PROJECTS, [used({}), used({ project_id: 'beta' })]);
    expect(entries.map(entry => entry.id)).toEqual(['repository:alpha:block-migration', 'repository:beta:block-migration']);
    expect(entries.map(entry => entry.projectIds)).toEqual([['alpha'], ['beta']]);
  });

  it('ignores what a project the page does not know uses', () => {
    const entries = automationSkillEntries(CATALOG, PROJECTS, [
      used({ project_id: 'ghost' }),
      used({ project_id: 'ghost', skill_id: 'orphan', slug: 'orphan' }),
    ]);
    expect(entries.some(entry => entry.repository)).toBe(false);
    expect(byId(entries).orphan.used).toBe(false);
  });

  it('names a repository skill after its folder when it has no name', () => {
    const [entry] = automationSkillEntries([], PROJECTS, [used({ name: '' })]);
    expect(entry.skill.name).toBe('block-migration');
  });

  it('tells a repository skill id from a catalog one', () => {
    expect(repositorySkillId('alpha', 'block-migration')).toBe('repository:alpha:block-migration');
    expect(isRepositorySkillId('repository:alpha:block-migration')).toBe(true);
    expect(isRepositorySkillId('review')).toBe(false);
    expect(isRepositorySkillId('custom-repository')).toBe(false);
  });
});
