import { describe, expect, it } from 'vitest';
import type { McpDefinition } from '../../types/generated';
import {
  cliFallbackEnvKey,
  compactPluginCredentials,
  pluginCredentialKeys,
  pluginCredentialKind,
} from '../pluginCredentials';

function definition(patch: Partial<McpDefinition>): McpDefinition {
  return {
    id: 'plugin',
    name: 'Plugin',
    description: '',
    transport: 'ApiOnly',
    env_keys: [],
    tags: [],
    token_url: null,
    token_help: null,
    publisher: 'Vendor',
    official: true,
    api_spec: null,
    ...patch,
  };
}

describe('plugin credentials', () => {
  it('distinguishes CLI authentication from API credentials', () => {
    expect(pluginCredentialKind(definition({ tags: ['cli', 'api'] }))).toBe('cli');
    expect(pluginCredentialKind(definition({ tags: ['api'] }))).toBe('api');
  });

  it('adds a registry-declared CLI fallback without duplicating stored keys', () => {
    const fastly = definition({
      tags: ['cli', 'api'],
      api_spec: {
        base_url: 'https://api.fastly.com',
        auth: {
          CliToken: {
            command: 'fastly',
            args: ['auth', 'token'],
            inject: { CustomHeader: { name: 'Fastly-Key' } },
            fallback_env_key: 'FASTLY_API_TOKEN',
          },
        },
        endpoints: [],
        config_keys: [],
      },
    });
    expect(cliFallbackEnvKey(fastly)).toBe('FASTLY_API_TOKEN');
    expect(pluginCredentialKeys(fastly, ['FASTLY_API_TOKEN'])).toEqual(['FASTLY_API_TOKEN']);
  });

  it('drops blank optional CLI values but preserves blank API fields', () => {
    const cli = definition({ tags: ['cli'], env_keys: ['GITLAB_TOKEN', 'GITLAB_HOST'] });
    expect(compactPluginCredentials(cli, {
      GITLAB_TOKEN: 'glpat-secret',
      GITLAB_HOST: '   ',
    })).toEqual({ GITLAB_TOKEN: 'glpat-secret' });

    const api = definition({ tags: ['api'] });
    expect(compactPluginCredentials(api, { API_TOKEN: '' })).toEqual({ API_TOKEN: '' });
  });
});
