import { describe, expect, it } from 'vitest';
import { listing, resource, skill } from '../../components/__tests__/repositoryResourceFixtures';
import {
  alignLines,
  attachedSkillIds,
  attentionCount,
  attentionItems,
  buildRows,
  matchesPresence,
  matchesQuery,
  originLabel,
  splitPath,
} from '../repositoryResourceRows';

describe('repository resource rows', () => {
  it('maps every status to one primary action', () => {
    const rows = buildRows(listing({
      resources: [
        resource({ id: 'a', name: 'A', kind: 'quick_prompt', status: 'kronn_only' }),
        resource({ id: 'repository:quick_exec:b', name: 'B', kind: 'quick_exec', status: 'repository_only' }),
        resource({ id: 'c', name: 'C', kind: 'workflow', status: 'repository_newer' }),
        resource({ id: 'd', name: 'D', kind: 'workflow', status: 'kronn_newer' }),
        resource({ id: 'e', name: 'E', kind: 'quick_api', status: 'conflict' }),
        resource({ id: 'f', name: 'F', kind: 'quick_exec', status: 'approval_required' }),
        resource({ id: 'g', name: 'G', kind: 'quick_prompt', status: 'up_to_date' }),
      ],
    }));
    expect(Object.fromEntries(rows.automation.map(row => [row.name, row.primary]))).toEqual({
      A: 'publish',
      B: 'import',
      C: 'update_kronn',
      D: 'update_repository',
      E: 'compare',
      F: 'approve',
      G: 'view',
    });
  });

  it('shows a kronn-only resource at its target path and marks it as not written yet', () => {
    const [row] = buildRows(listing({
      resources: [resource({
        id: 'a', name: 'A', kind: 'quick_prompt', status: 'kronn_only',
        repository_paths: ['kronn/prompts/a.md'],
      })],
    })).automation;
    expect(row.displayPath).toBe('kronn/prompts/a.md');
    expect(row.pathExists).toBe(false);
    expect(row.presence).toBe('kronn');
  });

  it('picks the rendered page of an artifact as its main path', () => {
    const [row] = buildRows(listing({
      resources: [resource({
        id: 'p', name: 'Page', kind: 'artifact', status: 'up_to_date',
        repository_paths: ['kronn/artifacts/page/artifact.yaml', 'kronn/artifacts/page/index.html'],
      })],
    })).artifacts;
    expect(row.displayPath).toBe('kronn/artifacts/page/index.html');
  });

  it('treats skills by where they live: native, catalog, referenced, attach-only', () => {
    const rows = buildRows(listing({
      skills_present: [
        skill({
          id: 'repository:lint', name: 'Lint', provenance: 'repository', status: 'native_skill',
          repository_paths: ['.claude/skills/lint/SKILL.md'],
        }),
        skill({
          id: 'repository:ref', name: 'Ref', provenance: 'repository', status: 'native_skill',
          repository_paths: ['.agents/skills/ref/SKILL.md'], referenced: true,
        }),
        skill({
          id: 'docs', name: 'Docs', provenance: 'repository', status: null,
          repository_paths: ['.claude/skills/docs/SKILL.md'],
        }),
      ],
      skills_available: [skill({ id: 'rust', name: 'Rust', provenance: 'kronn', status: null })],
    }));
    const byName = Object.fromEntries([...rows.skills, ...rows.catalog].map(row => [row.name, row]));
    expect(byName.Lint).toMatchObject({ state: 'native_skill', primary: 'use_native', presence: 'repository', origins: ['.claude'] });
    expect(byName.Ref).toMatchObject({ state: 'up_to_date', primary: 'view', scope: 'referenced', presence: 'both' });
    expect(byName.Docs).toMatchObject({ state: 'repository_only', primary: 'attach', attachOnly: true, scope: 'catalog' });
    expect(byName.Rust).toMatchObject({ state: 'catalog', primary: 'attach', presence: 'kronn', pathExists: false });
  });

  it('resends only the skills already attached when attaching another one', () => {
    const data = listing({
      skills_present: [
        skill({ id: 'kept', name: 'Kept', provenance: 'both', status: 'up_to_date' }),
        skill({ id: 'native', name: 'Native', provenance: 'repository', status: 'native_skill' }),
      ],
    });
    expect(attachedSkillIds(data)).toEqual(['kept']);
  });

  it('orders what needs attention by urgency: two versions, approval, late, new', () => {
    const data = listing({
      resources: [
        resource({ id: 'n', name: 'Zeta new', kind: 'quick_exec', status: 'repository_only' }),
        resource({ id: 'l', name: 'Late', kind: 'workflow', status: 'kronn_newer' }),
        resource({ id: 'a', name: 'Approve', kind: 'quick_exec', status: 'approval_required' }),
        resource({ id: 'c', name: 'Conflict', kind: 'workflow', status: 'conflict' }),
        resource({ id: 'k', name: 'Private', kind: 'quick_prompt', status: 'kronn_only' }),
        resource({ id: 'u', name: 'Same', kind: 'quick_prompt', status: 'up_to_date' }),
      ],
    });
    expect(attentionItems(buildRows(data)).map(item => [item.row.name, item.reason])).toEqual([
      ['Conflict', 'conflict'],
      ['Approve', 'approval'],
      ['Late', 'late'],
      ['Zeta new', 'new'],
    ]);
    expect(attentionCount(data)).toBe(4);
    expect(attentionCount(listing())).toBe(0);
  });

  it('leaves conflicts, approvals and native skills out of "align all"', () => {
    const rows = buildRows(listing({
      skills_present: [
        skill({ id: 'repository:lint', name: 'Lint', provenance: 'repository', status: 'native_skill', repository_paths: ['.claude/skills/lint/SKILL.md'] }),
      ],
      resources: [
        resource({ id: 'a', name: 'A', kind: 'quick_prompt', status: 'kronn_only' }),
        resource({ id: 'b', name: 'B', kind: 'quick_prompt', status: 'repository_only' }),
        resource({ id: 'c', name: 'C', kind: 'workflow', status: 'repository_newer' }),
        resource({ id: 'd', name: 'D', kind: 'workflow', status: 'kronn_newer' }),
        resource({ id: 'e', name: 'E', kind: 'quick_api', status: 'conflict' }),
        resource({ id: 'f', name: 'F', kind: 'quick_exec', status: 'approval_required' }),
      ],
    }));
    expect(alignLines(rows).map(line => [line.row.name, line.direction])).toEqual([
      ['A', 'to_repository'],
      ['B', 'to_kronn'],
      ['C', 'to_kronn'],
      ['D', 'to_repository'],
    ]);
  });

  it('filters by presence and searches the name and every path', () => {
    const { automation } = buildRows(listing({
      resources: [
        resource({ id: 'a', name: 'Nightly report', kind: 'workflow', status: 'up_to_date', repository_paths: ['kronn/workflows/nightly.yaml'] }),
        resource({ id: 'b', name: 'Private', kind: 'quick_prompt', status: 'kronn_only' }),
      ],
    }));
    // Rows come back sorted by name.
    const [both, kronnOnly] = automation;
    expect([both.name, kronnOnly.name]).toEqual(['Nightly report', 'Private']);
    expect(matchesPresence(both, 'both')).toBe(true);
    expect(matchesPresence(both, 'kronn')).toBe(false);
    expect(matchesPresence(kronnOnly, 'kronn')).toBe(true);
    expect(matchesPresence(kronnOnly, 'all')).toBe(true);
    expect(matchesQuery(both, 'NIGHTLY')).toBe(true);
    expect(matchesQuery(both, 'workflows/night')).toBe(true);
    expect(matchesQuery(both, 'nothing')).toBe(false);
    expect(matchesQuery(both, '  ')).toBe(true);
  });

  it('labels a path by its top folder and keeps the last two segments visible', () => {
    expect(originLabel('kronn/skills/rust/SKILL.md')).toBe('kronn/');
    expect(originLabel('.claude/skills/rust/SKILL.md')).toBe('.claude');
    expect(splitPath('.claude/skills/rust/SKILL.md')).toEqual({ head: '.claude/skills/', tail: 'rust/SKILL.md' });
    expect(splitPath('INDEX.md')).toEqual({ head: '', tail: 'INDEX.md' });
  });
});
