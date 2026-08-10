import type { McpDefinition } from '../types/generated';

export type PluginCredentialKind = 'api' | 'cli';

export function pluginCredentialKind(
  definition: Pick<McpDefinition, 'tags'> | null | undefined,
): PluginCredentialKind {
  return definition?.tags.includes('cli') ? 'cli' : 'api';
}

export function cliFallbackEnvKey(
  definition: Pick<McpDefinition, 'api_spec'> | null | undefined,
): string | null {
  const auth = definition?.api_spec?.auth;
  if (!auth || typeof auth !== 'object' || !('CliToken' in auth)) return null;
  return auth.CliToken.fallback_env_key ?? null;
}

export function pluginCredentialKeys(
  definition: Pick<McpDefinition, 'env_keys' | 'api_spec'> | null | undefined,
  storedKeys: string[] = [],
): string[] {
  const keys = new Set(storedKeys);
  definition?.env_keys.forEach(key => keys.add(key));
  const fallback = cliFallbackEnvKey(definition);
  if (fallback) keys.add(fallback);
  return [...keys];
}

export function compactPluginCredentials(
  definition: Pick<McpDefinition, 'tags'> | null | undefined,
  env: Record<string, string>,
): Record<string, string> {
  if (pluginCredentialKind(definition) !== 'cli') return env;
  return Object.fromEntries(
    Object.entries(env).filter(([, value]) => value.trim() !== ''),
  );
}
