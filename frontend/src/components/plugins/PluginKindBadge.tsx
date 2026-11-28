import { useT } from '../../lib/I18nContext';
import type { PluginKind } from '../../lib/pluginKind';
import { PLUGIN_KIND_META } from './pluginKindMeta';

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
  const meta = PLUGIN_KIND_META[kind];
  return (
    <span
      className="mcp-scope-badge mcp-kind-badge"
      data-kind={kind}
      title={t(meta.rowTooltip)}
    >
      {t(meta.rowLabel)}
    </span>
  );
}

/**
 * KT-2020 — the kind badge of a registry card in the « add a plugin » form:
 * icon + short label, the tooltip explaining the kind. Label, icon and
 * tooltip all come from `PLUGIN_KIND_META`.
 */
export function RegistryKindBadge({ kind }: { kind: PluginKind }) {
  const { t } = useT();
  const { label, tooltip, Icon } = PLUGIN_KIND_META[kind];
  return (
    <span className={`mcp-kind-badge mcp-kind-badge-${kind}`} title={t(tooltip)}>
      <Icon size={9} aria-hidden="true" /> {t(label)}
    </span>
  );
}
