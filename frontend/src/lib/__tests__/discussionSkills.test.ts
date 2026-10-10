import { describe, expect, it } from 'vitest';
import {
  missingSkillLabel,
  skillPickerModel,
  withoutForeignRepositorySkills,
} from '../discussionSkills';
import type { SkillPickerInput } from '../discussionSkills';
import type { ProjectUsedSkill, Skill, SkillCategory } from '../../types/generated';

const skill = (id: string, name: string, category: SkillCategory = 'Domain', description = ''): Skill => ({
  id,
  name,
  description,
  icon: '🔧',
  category,
  content: '',
  is_builtin: true,
  token_estimate: 0,
});

const repositorySkill = (over: Partial<ProjectUsedSkill> = {}): ProjectUsedSkill => ({
  project_id: 'p1',
  slug: 'block-migration',
  name: 'Block migration',
  root: '.agents/skills',
  relative_path: '.agents/skills/block-migration/SKILL.md',
  referenced: true,
  published: false,
  ...over,
});

const CATALOG = [
  skill('rust', 'Rust', 'Language', 'Idiomatic Rust'),
  skill('typescript', 'TypeScript', 'Language'),
  skill('security', 'Security', 'Domain', 'Threat modelling'),
  skill('seo', 'SEO', 'Business'),
  skill('devops', 'DevOps', 'Domain'),
];

const PROJECT = { id: 'p1', name: 'front_euronews', default_skill_ids: ['devops'] };

const model = (over: Partial<SkillPickerInput> = {}) => skillPickerModel({
  catalog: CATALOG,
  project: PROJECT,
  usedSkills: [repositorySkill()],
  selectedIds: [],
  query: '',
  loaded: true,
  ...over,
});

const names = (entries: { skill: Skill }[]) => entries.map(entry => entry.skill.name);

describe('skillPickerModel — a discussion with a project', () => {
  it('lists first what the project uses: its attached skills and the repository ones', () => {
    const picked = model();
    expect(names(picked.used)).toEqual(['Block migration', 'DevOps']);
    const repository = picked.used.find(entry => entry.repository);
    expect(repository?.id).toBe('repository:p1:block-migration');
    expect(repository?.repository?.root).toBe('.agents/skills');
    expect(picked.foldAvailable).toBe(true);
  });

  it('counts a catalog skill Kronn published into the repository as used', () => {
    const picked = model({
      project: { ...PROJECT, default_skill_ids: [] },
      usedSkills: [repositorySkill({ skill_id: 'security', slug: 'security', name: 'Security', referenced: false, published: true })],
    });
    expect(names(picked.used)).toEqual(['Security']);
    expect(picked.used[0].repository).toBeUndefined();
  });

  it('puts the ticked skills next, then the rest of the catalog by category', () => {
    const picked = model({ selectedIds: ['rust', 'devops'] });
    expect(names(picked.ticked)).toEqual(['Rust']);
    expect(picked.available.map(group => [group.category, names(group.entries)])).toEqual([
      ['Language', ['TypeScript']],
      ['Domain', ['Security']],
      ['Business', ['SEO']],
    ]);
    expect(picked.availableCount).toBe(3);
  });

  it('never offers another project\'s repository skills', () => {
    const picked = model({
      usedSkills: [
        repositorySkill(),
        repositorySkill({ project_id: 'p2', slug: 'elsewhere', name: 'Elsewhere' }),
      ],
    });
    expect(names(picked.used)).not.toContain('Elsewhere');
  });

  it('never offers another project\'s own skill, and lists this project\'s one as used', () => {
    const scoped = (id: string, name: string, project_id: string): Skill => ({ ...skill(id, name), is_builtin: false, project_id });
    const picked = model({
      catalog: [...CATALOG, scoped('custom-mine', 'Mine', 'p1'), scoped('custom-theirs', 'Theirs', 'p2')],
    });
    expect(names(picked.used)).toContain('Mine');
    const offered = [...picked.used, ...picked.ticked, ...picked.available.flatMap(group => group.entries)];
    expect(names(offered)).not.toContain('Theirs');
    expect(picked.total).toBe(CATALOG.length + 2);
  });

  it('a skill that left the repository is no longer listed', () => {
    const picked = model({ usedSkills: [] });
    expect(names(picked.used)).toEqual(['DevOps']);
  });

  it('keeps a ticked id nothing answers to, so it can be unticked', () => {
    const picked = model({ usedSkills: [], selectedIds: ['repository:p1:block-migration', 'gone'] });
    expect(picked.missing).toEqual(['repository:p1:block-migration', 'gone']);
    // Not before the lists were read: it is not missing, it is not loaded yet.
    expect(model({ usedSkills: [], selectedIds: ['repository:p1:block-migration'], loaded: false }).missing).toEqual([]);
  });

  it('searches name, description, id and folder across every section', () => {
    expect(names(model({ query: 'migration' }).used)).toEqual(['Block migration']);
    expect(names(model({ query: '.agents' }).used)).toEqual(['Block migration']);
    const byDescription = model({ query: 'threat' });
    expect(byDescription.used).toEqual([]);
    expect(byDescription.available.map(group => names(group.entries))).toEqual([['Security']]);
    const none = model({ query: 'zzz' });
    expect(none.used.length + none.ticked.length + none.availableCount).toBe(0);
  });
});

describe('skillPickerModel — a discussion with no project', () => {
  it('shows the whole catalog by category, with nothing to fold', () => {
    const picked = model({ project: null, usedSkills: [repositorySkill()], selectedIds: ['rust'] });
    expect(picked.used).toEqual([]);
    expect(picked.ticked).toEqual([]);
    expect(picked.foldAvailable).toBe(false);
    expect(picked.available.map(group => group.category)).toEqual(['Language', 'Domain', 'Business']);
    // Ticked skills stay where the catalog puts them.
    expect(names(picked.available[0].entries)).toEqual(['Rust', 'TypeScript']);
    // No project, so no repository skill.
    expect(picked.total).toBe(CATALOG.length);
  });
});

describe('skillPickerModel — robustness', () => {
  it('lists a skill of a category this build does not know instead of dropping it', () => {
    const odd = { ...skill('odd', 'Odd'), category: 'Whatever' as unknown as SkillCategory };
    const picked = model({ project: null, catalog: [odd] });
    expect(picked.available.map(group => [group.category, names(group.entries)])).toEqual([['Domain', ['Odd']]]);
  });
});

describe('withoutForeignRepositorySkills', () => {
  it('keeps catalog skills and the project\'s own repository skills only', () => {
    const ids = ['rust', 'repository:p1:a', 'repository:p2:b'];
    expect(withoutForeignRepositorySkills(ids, 'p1')).toEqual(['rust', 'repository:p1:a']);
    expect(withoutForeignRepositorySkills(ids, '')).toEqual(['rust']);
  });
});

describe('missingSkillLabel', () => {
  it('names a repository skill by its folder', () => {
    expect(missingSkillLabel('repository:p1:block-migration')).toBe('block-migration');
    expect(missingSkillLabel('custom-old')).toBe('custom-old');
  });
});
