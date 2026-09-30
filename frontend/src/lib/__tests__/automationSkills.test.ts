import { afterEach, describe, expect, it } from 'vitest';
import {
  SKILL_FAVORITES_STORAGE_KEY,
  SKILL_UPDATED_AT,
  projectsUsingSkill,
  readSkillFavorites,
  skillOrigin,
  writeSkillFavorites,
} from '../automationSkills';
import { sortAutomationResources } from '../automationSort';

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
