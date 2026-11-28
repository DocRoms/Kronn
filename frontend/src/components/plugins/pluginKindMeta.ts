import { Globe, Plug, Puzzle, Terminal, type LucideIcon } from 'lucide-react';
import type { PluginKind } from '../../lib/pluginKind';

/**
 * KT-2020 — how each plugin kind presents itself, in one place.
 *
 * - `label` / `tooltip` — the short badge of a registry card in the
 *   « add a plugin » form (`mcp.kind.*`), with its `Icon`.
 * - `rowLabel` / `rowTooltip` — the badge of a Plugins overview row
 *   (`mcp.pluginKindBadge.*`, see `PluginKindBadge`).
 *
 * Typed as a `Record` over `PluginKind`: a new kind that has no entry here
 * fails the type check instead of falling back to the MCP badge (the bug
 * where a CLI card read "MCP" and its tooltip was the raw key
 * `mcp.kind.cliTooltip`).
 */
export interface PluginKindMeta {
  label: string;
  tooltip: string;
  Icon: LucideIcon;
  rowLabel: string;
  rowTooltip: string;
}

export const PLUGIN_KIND_META: Record<PluginKind, PluginKindMeta> = {
  mcp: {
    label: 'mcp.kind.mcp',
    tooltip: 'mcp.kind.mcpTooltip',
    Icon: Plug,
    rowLabel: 'mcp.pluginKindBadge.mcp.label',
    rowTooltip: 'mcp.pluginKindBadge.mcp.tooltip',
  },
  api: {
    label: 'mcp.kind.api',
    tooltip: 'mcp.kind.apiTooltip',
    Icon: Globe,
    rowLabel: 'mcp.pluginKindBadge.api.label',
    rowTooltip: 'mcp.pluginKindBadge.api.tooltip',
  },
  hybrid: {
    label: 'mcp.kind.hybrid',
    tooltip: 'mcp.kind.hybridTooltip',
    Icon: Puzzle,
    rowLabel: 'mcp.pluginKindBadge.hybrid.label',
    rowTooltip: 'mcp.pluginKindBadge.hybrid.tooltip',
  },
  cli: {
    label: 'mcp.kind.cli',
    tooltip: 'mcp.kind.cliTooltip',
    // The terminal the credential badge already uses for a CLI login.
    Icon: Terminal,
    rowLabel: 'mcp.pluginKindBadge.cli.label',
    rowTooltip: 'mcp.pluginKindBadge.cli.tooltip',
  },
};
