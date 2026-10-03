import { useT } from '../../lib/I18nContext';
import type { PluginKind } from '../../lib/pluginKind';

/**
 * Compact "what kind of plugin is this" badge — shown on each installed
 * config card so the user can tell at a glance whether the plugin
 * surfaces tools via MCP (synced to `.mcp.json`), via REST API (injected
 * in the agent's system prompt), or both. Avoids the trap where a user
 * sees a `🌐 CLI local` chip on an API-only plugin and assumes it's
 * being written to their host config files (it isn't — only MCP
 * transports are synced; API-only plugins live in prompts).
 */
export function PluginKindBadge({ kind }: { kind: PluginKind }) {
  const { t } = useT();
  const meta = kind === 'cli'
    ? { label: t('mcp.pluginKindBadge.cli.label'), tooltip: t('mcp.pluginKindBadge.cli.tooltip') }
    : kind === 'api'
    ? { label: t('mcp.pluginKindBadge.api.label'), tooltip: t('mcp.pluginKindBadge.api.tooltip') }
    : kind === 'hybrid'
    ? { label: t('mcp.pluginKindBadge.hybrid.label'), tooltip: t('mcp.pluginKindBadge.hybrid.tooltip') }
    : { label: t('mcp.pluginKindBadge.mcp.label'), tooltip: t('mcp.pluginKindBadge.mcp.tooltip') };
  return (
    <span
      className="mcp-scope-badge mcp-kind-badge"
      data-kind={kind}
      title={meta.tooltip}
    >
      {meta.label}
    </span>
  );
}
