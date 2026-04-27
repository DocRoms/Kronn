/**
 * Dynamic preview of where Kronn will write a config in the local CLI
 * config files. Renders the asymmetry between Claude (which has a native
 * `local` scope under `projects[<path>].mcpServers`) and the other CLIs
 * (Gemini / Codex / Copilot) that only support a top-level scope.
 *
 * UX rationale (Antoine + Marie): never surprise the user about which
 * file gets written. Showing the destination per CLI right under the
 * checkbox makes the routing transparent and avoids the "I scoped to
 * APP_ANDROID, why does Gemini see this MCP everywhere?" trap.
 */
import type { Project } from '../types/generated';

interface HostSyncPreviewProps {
  /** Kronn-side global flag — `true` means "applied to all Kronn projects". */
  isGlobal: boolean;
  /** Selected projects (empty when isGlobal=true or "no project bound"). */
  projectIds: string[];
  /** All projects, used to resolve project paths for the Claude `local` scope. */
  projects: Project[];
}

const ROW_STYLE: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: '110px 1fr',
  gap: 6,
  padding: '3px 0',
  fontSize: '0.82em',
};

const CLI_STYLE: React.CSSProperties = {
  color: 'var(--kr-text-muted)',
  fontFamily: 'var(--kr-font-mono, monospace)',
};

const TARGET_STYLE: React.CSSProperties = {
  fontFamily: 'var(--kr-font-mono, monospace)',
  fontSize: '0.95em',
  color: 'var(--kr-text-primary, inherit)',
  wordBreak: 'break-all',
};

export function HostSyncPreview({ isGlobal, projectIds, projects }: HostSyncPreviewProps) {
  // Resolve project paths in deterministic order (sorted by name).
  const linkedPaths = !isGlobal
    ? projects
        .filter(p => projectIds.includes(p.id))
        .sort((a, b) => a.name.localeCompare(b.name))
        .map(p => p.path)
    : [];

  // Determine Claude target lines.
  const claudeLines: string[] = (() => {
    if (isGlobal) return ['~/.claude.json (top-level, tous projets)'];
    if (linkedPaths.length === 0) return ['~/.claude.json (top-level, aucun projet sélectionné)'];
    return linkedPaths.map(p => `~/.claude.json › projects[${p}]`);
  })();

  // Other CLIs are always top-level (no native per-project scope in their
  // config files). Surface this asymmetry with a hint so the user knows
  // their per-project Kronn selection doesn't transfer there.
  const otherCliHint = !isGlobal && linkedPaths.length > 0
    ? ' (top-level — scope projet non supporté)'
    : '';

  return (
    <div
      style={{
        marginTop: 8,
        padding: '8px 10px',
        background: 'var(--kr-info-bg, rgba(96, 165, 250, 0.06))',
        border: '1px solid var(--kr-border-subtle, rgba(0,0,0,0.06))',
        borderRadius: 4,
      }}
    >
      <div style={{ fontSize: '0.78em', color: 'var(--kr-text-muted)', marginBottom: 6, textTransform: 'uppercase', letterSpacing: '0.04em' }}>
        Sera écrit dans
      </div>
      {claudeLines.map((line, i) => (
        <div key={`claude-${i}`} style={ROW_STYLE}>
          <span style={CLI_STYLE}>{i === 0 ? 'Claude Code' : ''}</span>
          <span style={TARGET_STYLE}>{line}</span>
        </div>
      ))}
      <div style={ROW_STYLE}>
        <span style={CLI_STYLE}>Gemini</span>
        <span style={TARGET_STYLE}>~/.gemini/settings.json{otherCliHint}</span>
      </div>
      <div style={ROW_STYLE}>
        <span style={CLI_STYLE}>Codex</span>
        <span style={TARGET_STYLE}>~/.codex/config.toml{otherCliHint}</span>
      </div>
      <div style={ROW_STYLE}>
        <span style={CLI_STYLE}>Copilot</span>
        <span style={TARGET_STYLE}>~/.copilot/mcp-config.json{otherCliHint}</span>
      </div>
    </div>
  );
}
