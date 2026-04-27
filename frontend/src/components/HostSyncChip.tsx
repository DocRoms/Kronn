/**
 * Compact, single-line scope chip for the MCP card grid (McpPage cards).
 *
 * Shown when the config has `host_sync !== 'None'` to indicate at a
 * glance that this MCP is exposed to local CLIs (Claude Code / Gemini /
 * Codex / Copilot). Click on the parent card opens the drawer where
 * the actual checkbox lives — single source of edit (the previous
 * 3-mode radio was removed in the Phase-3 refactor that unified scope
 * Kronn ↔ scope CLI under one model).
 */
import type { HostSyncMode } from '../types/generated';

export function HostSyncChip({ mode }: { mode: HostSyncMode }) {
  if (mode === 'None') return null;
  // Both `GlobalOnly` and `MirrorAll` (legacy) collapse to the same
  // user-facing label: this MCP is in your local CLI files.
  const label = '🌐 CLI local';
  const tooltip = 'Synchronisé dans ~/.claude.json, ~/.gemini/settings.json, ~/.codex/config.toml, ~/.copilot/mcp-config.json — disponible quand tu utilises ces CLIs hors Kronn.';
  return (
    <span
      className="badge"
      title={tooltip}
      style={{
        background: 'var(--kr-accent-bg, rgba(59, 130, 246, 0.1))',
        color: 'var(--kr-accent, #3b82f6)',
        fontSize: '0.75em',
      }}
    >
      {label}
    </span>
  );
}
