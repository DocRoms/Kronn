import type { ApiSpec, McpDefinition } from '../types/generated';

/**
 * What kind of plugin a registry/config entry represents, for the
 * compact badge on each MCP config card.
 *
 * - `cli`    — a CLI wrapper (carries the `cli` tag). Checked FIRST so a
 *              future CLI wrapper that ALSO exposes a REST API stays
 *              bucketed as CLI (the prereq is what matters to the user).
 * - `api`    — API-only plugin (transport sentinel `"ApiOnly"`); tools
 *              are injected in the agent prompt, NOT synced to `.mcp.json`.
 * - `hybrid` — an MCP transport that ALSO carries an `api_spec`.
 * - `mcp`    — plain MCP transport (synced to host `.mcp.json`).
 *
 * Extracted from McpPage so the bucketing logic can be unit-tested
 * independently of the (heavy, stateful) page shell.
 */
export type PluginKind = 'mcp' | 'api' | 'hybrid' | 'cli';

export function pluginKind(m: {
  transport: McpDefinition['transport'];
  api_spec?: ApiSpec | null;
  tags?: string[];
}): PluginKind {
  const hasApi = !!m.api_spec;
  const hasCliTag = Array.isArray(m.tags) && m.tags.includes('cli');
  // McpTransport is a discriminated union; the API-only sentinel is the
  // string literal "ApiOnly" (not a { tag: ... } object).
  const isApiOnly = (m.transport as unknown) === 'ApiOnly';
  if (hasCliTag) return 'cli';
  if (isApiOnly) return 'api';
  if (hasApi) return 'hybrid';
  return 'mcp';
}
