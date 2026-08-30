/**
 * Compact, single-line scope chip for the MCP card grid (McpPage cards).
 *
 * Always shown so "global in Kronn" can never be mistaken for "available in
 * every local CLI". Click on the parent card opens the drawer where
 * the actual checkbox lives — single source of edit (the previous
 * 3-mode radio was removed in the Phase-3 refactor that unified scope
 * Kronn ↔ scope CLI under one model).
 */
import type { HostSyncMode } from '../types/generated';
import { useT } from '../lib/I18nContext';

export function HostSyncChip({ mode }: { mode: HostSyncMode }) {
  const { t } = useT();
  if (mode === 'None') {
    return (
      <span
        className="badge"
        title={t('mcp.hostScope.kronnOnlyHint')}
        style={{
          background: 'var(--kr-surface-muted, rgba(100, 116, 139, 0.1))',
          color: 'var(--kr-text-secondary, #64748b)',
          fontSize: '0.75em',
        }}
      >
        {t('mcp.hostScope.kronnOnly')}
      </span>
    );
  }
  // Both `GlobalOnly` and `MirrorAll` (legacy) collapse to the same
  // user-facing label: this MCP is in your local CLI files.
  const label = t('mcp.hostScope.localCli');
  const tooltip = t('mcp.hostScope.localCliHint');
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
