import { describe, expect, it } from 'vitest';
import type { QuickApi, QuickPrompt } from '../../types/generated';
import { sortQuickApis, sortQuickPrompts } from '../automationSort';

const quickPrompt = (id: string, name: string, updatedAt: string): QuickPrompt => ({
  id,
  name,
  icon: '✨',
  prompt_template: name,
  variables: [],
  agent: 'Codex',
  project_id: null,
  skill_ids: [],
  profile_ids: [],
  directive_ids: [],
  tier: 'default',
  description: '',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: updatedAt,
});

const quickApi = (
  id: string,
  name: string,
  plugin: string,
  endpoint: string,
  updatedAt: string,
): QuickApi => ({
  id,
  name,
  icon: '⚡',
  description: '',
  project_id: null,
  api_plugin_slug: plugin,
  api_config_id: `${plugin}-config`,
  api_endpoint_path: endpoint,
  variables: [],
  profile_ids: [],
  directive_ids: [],
  created_at: '2026-01-01T00:00:00Z',
  updated_at: updatedAt,
});

describe('automation list sorting', () => {
  it('sorts Quick Prompts by name, modification date or total usage without mutating the API list', () => {
    const source = [
      quickPrompt('z', 'Zulu 10', '2026-03-01T00:00:00Z'),
      quickPrompt('a', 'alpha', '2026-01-01T00:00:00Z'),
      quickPrompt('b', 'Zulu 2', '2026-02-01T00:00:00Z'),
    ];

    expect(sortQuickPrompts(source, 'name', {}).map(item => item.id)).toEqual(['a', 'b', 'z']);
    expect(sortQuickPrompts(source, 'name', {}, true).map(item => item.id)).toEqual(['z', 'b', 'a']);
    expect(sortQuickPrompts(source, 'updated', {}).map(item => item.id)).toEqual(['z', 'b', 'a']);
    expect(sortQuickPrompts(source, 'updated', {}, true).map(item => item.id)).toEqual(['a', 'b', 'z']);
    expect(sortQuickPrompts(source, 'usage', { a: 9, b: 2, z: 4 }).map(item => item.id))
      .toEqual(['a', 'z', 'b']);
    expect(source.map(item => item.id)).toEqual(['z', 'a', 'b']);
  });

  it('sorts Quick APIs by name, modification date or plugin/endpoint', () => {
    const source = [
      quickApi('z', 'Zulu', 'jira', '/tickets', '2026-03-01T00:00:00Z'),
      quickApi('a', 'Alpha', 'chartbeat', '/top', '2026-01-01T00:00:00Z'),
      quickApi('b', 'Beta', 'chartbeat', '/live', '2026-02-01T00:00:00Z'),
    ];

    expect(sortQuickApis(source, 'name').map(item => item.id)).toEqual(['a', 'b', 'z']);
    expect(sortQuickApis(source, 'name', true).map(item => item.id)).toEqual(['z', 'b', 'a']);
    expect(sortQuickApis(source, 'updated').map(item => item.id)).toEqual(['z', 'b', 'a']);
    expect(sortQuickApis(source, 'endpoint').map(item => item.id)).toEqual(['b', 'a', 'z']);
    expect(sortQuickApis(source, 'endpoint', true).map(item => item.id)).toEqual(['z', 'a', 'b']);
  });
});
