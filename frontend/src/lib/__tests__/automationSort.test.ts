import { describe, expect, it } from 'vitest';
import type { QuickApi, QuickPrompt } from '../../types/generated';
import { sortAutomationResources, sortQuickApis, sortQuickPrompts, type SortableAutomation } from '../automationSort';

const quickPrompt = (id: string, name: string, updatedAt: string): QuickPrompt => ({
  id,
  pinned: false,
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
  pinned: false,
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

describe('sorting the global Automation sidebar (KT-912)', () => {
  const resource = (
    id: string,
    kind: SortableAutomation['kind'],
    pinned = false,
    updatedAt = '2026-01-01T00:00:00Z',
  ): SortableAutomation & { id: string } => ({ id, kind, name: id, pinned, updatedAt });
  const source = [
    resource('Zeta', 'workflows', false, '2026-01-01T00:00:00Z'),
    resource('beta', 'quickExecs', false, '2026-03-01T00:00:00Z'),
    resource('Gamma', 'quickPrompts', true, '2026-02-01T00:00:00Z'),
    resource('alpha', 'quickApis', false, '2026-02-15T00:00:00Z'),
    resource('Delta', 'workflows', true, '2026-01-15T00:00:00Z'),
  ];
  const ids = (items: Array<{ id: string }>) => items.map(item => item.id);

  it('sorts by name without mutating the source, favorites staying first', () => {
    expect(ids(sortAutomationResources(source, 'name'))).toEqual(['Delta', 'Gamma', 'alpha', 'beta', 'Zeta']);
    expect(ids(source)).toEqual(['Zeta', 'beta', 'Gamma', 'alpha', 'Delta']);
  });

  it('reverses each side of the favorites line, never sinking a favorite', () => {
    expect(ids(sortAutomationResources(source, 'name', true))).toEqual(['Gamma', 'Delta', 'Zeta', 'beta', 'alpha']);
  });

  it('sorts by last modification, newest first', () => {
    expect(ids(sortAutomationResources(source, 'updated'))).toEqual(['Gamma', 'Delta', 'beta', 'alpha', 'Zeta']);
    expect(ids(sortAutomationResources(source, 'updated', true))).toEqual(['Delta', 'Gamma', 'Zeta', 'alpha', 'beta']);
  });

  it('sorts by type in the order of the type list, then by name', () => {
    expect(ids(sortAutomationResources(source, 'kind'))).toEqual(['Delta', 'Gamma', 'Zeta', 'alpha', 'beta']);
  });

  describe('by last opening (KT-916)', () => {
    const opened = (id: string, lastOpenedAt: number | null, pinned = false): SortableAutomation & { id: string } => ({
      id, kind: 'workflows', name: id, pinned, updatedAt: '2026-01-01T00:00:00Z', lastOpenedAt,
    });
    const history = [
      opened('Never', null),
      opened('Old', 100),
      opened('Pinned', 50, true),
      opened('Fresh', 300),
      opened('Also never', null),
    ];

    it('puts the latest opening first and what was never opened after it, by name', () => {
      expect(ids(sortAutomationResources(history, 'opened'))).toEqual(['Pinned', 'Fresh', 'Old', 'Also never', 'Never']);
    });

    it('reverses the order of each side of the favorites line', () => {
      expect(ids(sortAutomationResources(history, 'opened', true))).toEqual(['Pinned', 'Never', 'Also never', 'Old', 'Fresh']);
    });

    it('is a plain history, favorites not pulled up, for the Recent chip', () => {
      expect(ids(sortAutomationResources(history, 'opened', false, { pinnedFirst: false })))
        .toEqual(['Fresh', 'Old', 'Pinned', 'Also never', 'Never']);
    });

    it('reads an absent date as never opened', () => {
      expect(ids(sortAutomationResources([resource('B', 'workflows'), opened('A', 10)], 'opened'))).toEqual(['A', 'B']);
    });
  });
});
