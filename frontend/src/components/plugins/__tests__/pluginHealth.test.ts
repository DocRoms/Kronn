import { describe, expect, it } from 'vitest';
import type { McpConfigDisplay } from '../../../types/generated';
import { accessHealth, configHealth, isAvailableLocally, latestPluginTest, visibleToPluginProject } from '../pluginHealth';

const config = (overrides: Partial<McpConfigDisplay> = {}): McpConfigDisplay => ({
  id: 'config-1',
  server_id: 'plugin-1',
  server_name: 'Plugin',
  label: 'Plugin',
  env_keys: [],
  env_masked: [],
  args_override: null,
  is_global: false,
  include_general: false,
  config_hash: 'hash',
  project_ids: ['project-1'],
  project_names: ['Project'],
  secrets_broken: false,
  host_sync: 'None',
  preferred_interface: 'mcp',
  interfaces: ['mcp', 'api'],
  effective_kind: 'hybrid',
  effective_preferred_interface: 'mcp',
  credential_source: 'stored',
  last_probes: [],
  ...overrides,
});

describe('plugin health projection', () => {
  it('keeps health separate for each access and preserves its timestamp', () => {
    const target = config({
      last_probes: [
        { access: 'mcp', ok: true, code: 'ok', summary: 'ready', tested_at: '2026-09-28T08:00:00Z' },
        { access: 'api', ok: false, code: 'unauthorized', summary: 'denied', tested_at: '2026-09-28T09:00:00Z' },
      ],
    });

    expect(accessHealth(target, 'mcp')).toMatchObject({ state: 'ok', code: 'ok' });
    expect(accessHealth(target, 'api')).toEqual({
      access: 'api', state: 'error', code: 'unauthorized', testedAt: '2026-09-28T09:00:00Z',
    });
    expect(configHealth(target)).toBe('error');
    expect(latestPluginTest(target)).toBe('2026-09-28T09:00:00Z');
  });

  it('marks untested access for verification and broken secrets as errors', () => {
    expect(accessHealth(config(), 'mcp')).toMatchObject({ state: 'warning', code: 'never_tested' });
    expect(configHealth(config({ secrets_broken: true }))).toBe('error');
  });

  it('projects global, project and no-project visibility without duplicating rules in the view', () => {
    const scoped = config();
    expect(visibleToPluginProject(scoped, 'project-1')).toBe(true);
    expect(visibleToPluginProject(scoped, '__none__')).toBe(false);
    expect(visibleToPluginProject(scoped, '__all__')).toBe(true);
    expect(visibleToPluginProject(config({ is_global: true }), '__none__')).toBe(true);
    expect(visibleToPluginProject(config({ include_general: false, project_ids: [] }), '__none__')).toBe(true);
  });

  it('counts a plugin as local when it is host-synced or reachable through a CLI', () => {
    expect(isAvailableLocally(config({ interfaces: ['mcp'], host_sync: 'None' }))).toBe(false);
    expect(isAvailableLocally(config({ interfaces: ['mcp'], host_sync: 'GlobalOnly' }))).toBe(true);
    expect(isAvailableLocally(config({ interfaces: ['mcp'], host_sync: 'MirrorAll' }))).toBe(true);
    expect(isAvailableLocally(config({ interfaces: ['cli'], host_sync: 'None' }))).toBe(true);
  });
});
