import { describe, expect, it } from 'vitest';
import { listing, resource, skill } from '../../components/__tests__/repositoryResourceFixtures';
import { describeTransfer, transferSide, type TransferKind } from '../repositoryResourceEffects';
import { allRows, buildRows } from '../repositoryResourceRows';

const context = { kronnExists: true, formatDate: (iso?: string) => iso ?? '—' };
const keysOf = (kind: TransferKind, data: ReturnType<typeof listing>, options = context) => {
  const [row] = allRows(buildRows(data));
  return describeTransfer({ kind, rows: [row] }, options).map(line => line.key.split('.').pop());
};

describe('transfer effects', () => {
  it('writing to the repository names the exact files, the commit left to do and what is replaced', () => {
    const data = listing({
      resources: [resource({
        id: 'a', name: 'A', kind: 'quick_prompt', status: 'kronn_newer',
        repository_paths: ['kronn/prompts/a.md'],
        write_preview: ['kronn/kronn.toml', 'kronn/prompts/a.md'],
        repository_updated_at: '2026-09-01T10:00:00Z',
      })],
    });
    const lines = describeTransfer({ kind: 'update_repository', rows: buildRows(data).automation }, context);
    expect(lines.map(line => line.tone)).toEqual(['write', 'commit', 'activation', 'loss']);
    expect(lines[0].args).toEqual(['kronn/kronn.toml, kronn/prompts/a.md']);
    expect(lines[3]).toMatchObject({ key: 'projects.repositoryResources.effect.lossRepository', args: ['2026-09-01T10:00:00Z'] });
  });

  it('announces the kronn/ folder it creates when it does not exist yet', () => {
    const data = listing({
      kronn_exists: false,
      resources: [resource({ id: 'a', name: 'A', kind: 'quick_prompt', status: 'kronn_only' })],
    });
    expect(keysOf('publish', data, { ...context, kronnExists: false })).toEqual([
      'write', 'createsFolder', 'commitTodo', 'publishNoActivation', 'lossNone',
    ]);
  });

  it('loading into Kronn leaves the repository alone and waits for approval on executable kinds', () => {
    const executable = listing({ resources: [resource({ id: 'a', name: 'A', kind: 'quick_exec', status: 'repository_only' })] });
    const passive = listing({ resources: [resource({ id: 'a', name: 'A', kind: 'artifact', status: 'repository_only' })] });
    expect(keysOf('import', executable)).toEqual(['repositoryUntouched', 'kronnCreates', 'commitNone', 'needsApproval', 'lossNone']);
    expect(keysOf('import', passive)).toContain('noExecution');
  });

  it('updating Kronn says which Kronn version is replaced', () => {
    const data = listing({
      resources: [resource({ id: 'a', name: 'A', kind: 'workflow', status: 'repository_newer', kronn_updated_at: '2026-09-02T08:00:00Z' })],
    });
    const lines = describeTransfer({ kind: 'update_kronn', rows: buildRows(data).automation }, context);
    expect(lines.at(-1)).toMatchObject({ key: 'projects.repositoryResources.effect.lossKronn', args: ['2026-09-02T08:00:00Z'] });
  });

  it('using a native skill references the chosen file without writing anywhere', () => {
    const data = listing({
      skills_present: [skill({ id: 'repository:x', name: 'X', provenance: 'repository', status: 'native_skill', repository_paths: ['.claude/skills/x/SKILL.md', '.agents/skills/x/SKILL.md'] })],
    });
    const rows = buildRows(data).skills;
    const lines = describeTransfer({ kind: 'use_native', rows }, { ...context, nativePath: '.agents/skills/x/SKILL.md' });
    expect(lines[0]).toMatchObject({ key: 'projects.repositoryResources.effect.usePath', args: ['.agents/skills/x/SKILL.md'] });
    expect(lines.map(line => line.tone)).toEqual(['write', 'commit', 'activation', 'loss']);
  });

  it('writing with linked items lists their files too and says how many come along', () => {
    const data = listing({
      resources: [
        resource({ id: 'a', name: 'A', kind: 'workflow', status: 'kronn_only', repository_paths: ['kronn/workflows/a.yaml'], write_preview: ['kronn/workflows/a.yaml'] }),
        resource({ id: 'b', name: 'B', kind: 'quick_prompt', status: 'kronn_only', repository_paths: ['kronn/prompts/b.md'], write_preview: ['kronn/prompts/b.md'] }),
      ],
    });
    const [workflow, prompt] = allRows(buildRows(data));
    const lines = describeTransfer({ kind: 'publish', rows: [workflow], linked: [prompt] }, context);
    expect(lines[0].args).toEqual(['kronn/prompts/b.md, kronn/workflows/a.yaml']);
    expect(lines[1]).toMatchObject({ tone: 'write', key: 'projects.repositoryResources.effect.linkedIncluded', args: [1] });
    expect(describeTransfer({ kind: 'publish', rows: [workflow], linked: [] }, context).map(line => line.key.split('.').pop()))
      .not.toContain('linkedIncluded');
  });

  it('loading with linked items says how many come along, and the repository stays untouched', () => {
    const data = listing({
      resources: [
        resource({ id: 'a', name: 'A', kind: 'workflow', status: 'repository_only' }),
        resource({ id: 'b', name: 'B', kind: 'quick_prompt', status: 'repository_only' }),
      ],
    });
    const [workflow, prompt] = allRows(buildRows(data));
    const keys = describeTransfer({ kind: 'import', rows: [workflow], linked: [prompt] }, context)
      .map(line => line.key.split('.').pop());
    expect(keys).toEqual(['repositoryUntouched', 'kronnCreates', 'linkedIncluded', 'commitNone', 'needsApproval', 'lossNone']);
  });

  it('only a one-resource transfer between the two sides has dependencies to announce', () => {
    expect(transferSide('publish')).toBe('repository');
    expect(transferSide('update_repository')).toBe('repository');
    expect(transferSide('import')).toBe('kronn');
    expect(transferSide('update_kronn')).toBe('kronn');
    expect(['publish_selected', 'use_native', 'copy_native', 'attach'].map(kind => transferSide(kind as TransferKind)))
      .toEqual([null, null, null, null]);
  });
});
