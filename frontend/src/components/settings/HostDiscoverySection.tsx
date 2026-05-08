/**
 * Settings > Host MCP discovery card (Phase 1 of inbound/outbound MCP feature).
 *
 * Read-only view of MCPs declared in `~/.claude.json`,
 * `~/.gemini/settings.json`, `~/.codex/config.toml`, and
 * `~/.copilot/mcp-config.json`. Surfaces which entries are managed by
 * Kronn vs. configured externally — no mutations.
 *
 * Phase 2 will add an "Adopt" button that imports an external entry into
 * the Kronn registry. Phase 3 will add the outbound sync (toggle to write
 * Kronn entries back into the home files).
 */
import { useCallback, useEffect, useState } from 'react';
import { mcps as mcpsApi } from '../../lib/api';
import type { DiscoveredHostMcp, HostScope, KronnOwnership } from '../../types/generated';
import { CheckCircle2, Cpu, Eye, RefreshCw, AlertTriangle, FileText, Plus, X } from 'lucide-react';
import '../../pages/SettingsPage.css';

interface HostDiscoverySectionProps {
  t: (key: string, ...args: (string | number)[]) => string;
}

/** Display label for each scope, used as a section heading. */
function scopeLabel(scope: HostScope): string {
  switch (scope.kind) {
    case 'ClaudeUser':
      return 'Claude Code (~/.claude.json — global)';
    case 'ClaudeLocal':
      return `Claude Code (project-scoped: ${scope.value.project_path})`;
    case 'Gemini':
      return 'Gemini CLI (~/.gemini/settings.json)';
    case 'Codex':
      return 'Codex (~/.codex/config.toml)';
    case 'Copilot':
      return 'Copilot CLI (~/.copilot/mcp-config.json)';
  }
}

/** Stable key for grouping by scope (treats every ClaudeLocal path as its own group). */
function scopeKey(scope: HostScope): string {
  if (scope.kind === 'ClaudeLocal') return `ClaudeLocal:${scope.value.project_path}`;
  return scope.kind;
}

/** Order: top-level Claude → Gemini → Codex → Copilot → per-project Claude (most niche). */
function scopeOrder(scope: HostScope): number {
  switch (scope.kind) {
    case 'ClaudeUser': return 0;
    case 'Gemini': return 1;
    case 'Codex': return 2;
    case 'Copilot': return 3;
    case 'ClaudeLocal': return 4;
  }
}

/** Compact ownership badge with stable colors. */
function OwnershipBadge({ ownership }: { ownership: KronnOwnership }) {
  if (ownership.type === 'NotManaged') {
    return (
      <span
        className="badge"
        style={{ background: 'var(--kr-warning-bg, #f59e0b22)', color: 'var(--kr-warning, #b45309)' }}
        title="Cette entrée n'est pas dans le registre Kronn (configurée à la main avec `claude mcp add`, etc.). Kronn ne la touche jamais."
      >
        Externe
      </span>
    );
  }
  const tooltip = ownership.type === 'ManagedByMarker'
    ? `Marqueur _kronn présent (config_id: ${ownership.config_id})`
    : `Détecté par hash (config_id: ${ownership.config_id})`;
  return (
    <span
      className="badge"
      style={{ background: 'var(--kr-success-bg, #10b98122)', color: 'var(--kr-success, #047857)' }}
      title={tooltip}
    >
      <CheckCircle2 size={12} style={{ verticalAlign: 'text-bottom', marginRight: 4 }} />
      Géré par Kronn
    </span>
  );
}

/** Transport summary — single short line. */
function transportSummary(d: DiscoveredHostMcp): string {
  const t = d.transport;
  if (t === 'ApiOnly') return 'API only';
  if ('Stdio' in t) {
    const args = t.Stdio.args.length > 0 ? ` ${t.Stdio.args.slice(0, 2).join(' ')}${t.Stdio.args.length > 2 ? '…' : ''}` : '';
    return `${t.Stdio.command}${args}`;
  }
  if ('Sse' in t) return `SSE → ${t.Sse.url}`;
  return `HTTP → ${t.Streamable.url}`;
}

export function HostDiscoverySection({ t: _t }: HostDiscoverySectionProps) {
  const [entries, setEntries] = useState<DiscoveredHostMcp[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loadedAt, setLoadedAt] = useState<Date | null>(null);
  const [adoptTarget, setAdoptTarget] = useState<DiscoveredHostMcp | null>(null);
  const [adopting, setAdopting] = useState(false);
  const [adoptError, setAdoptError] = useState<string | null>(null);

  const adopt = useCallback(async () => {
    if (!adoptTarget) return;
    setAdopting(true);
    setAdoptError(null);
    try {
      await mcpsApi.adoptHost({
        source_file: adoptTarget.source_file,
        scope: adoptTarget.scope,
        name: adoptTarget.name,
      });
      setAdoptTarget(null);
      // Re-scan so the freshly-adopted entry now shows the "Géré par Kronn"
      // badge instead of the "Hors Kronn" + adopt button.
      const data = await mcpsApi.hostDiscovery();
      setEntries(data);
      setLoadedAt(new Date());
    } catch (e: unknown) {
      setAdoptError(e instanceof Error ? e.message : String(e));
    } finally {
      setAdopting(false);
    }
  }, [adoptTarget]);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await mcpsApi.hostDiscovery();
      setEntries(data);
      setLoadedAt(new Date());
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { refresh(); }, [refresh]);

  // Group entries by scope for display
  const groups = new Map<string, { scope: HostScope; items: DiscoveredHostMcp[] }>();
  for (const entry of entries) {
    const key = scopeKey(entry.scope);
    let group = groups.get(key);
    if (!group) {
      group = { scope: entry.scope, items: [] };
      groups.set(key, group);
    }
    group.items.push(entry);
  }
  const sortedGroups = Array.from(groups.values()).sort((a, b) => scopeOrder(a.scope) - scopeOrder(b.scope));

  const totalCount = entries.length;
  const managedCount = entries.filter(e => e.managed_by_kronn.type !== 'NotManaged').length;
  const externalCount = totalCount - managedCount;

  return (
    <div className="set-section">
      <div className="flex-row gap-4 set-section-header-lg">
        <Cpu size={20} />
        <div>
          <h3 style={{ margin: 0 }}>MCPs détectés en local</h3>
          <p className="text-muted" style={{ margin: '4px 0 0 0', fontSize: '0.85em' }}>
            Vue lecture seule des MCPs configurés dans les fichiers home des CLIs (Claude Code, Gemini, Codex, Copilot).
            Aucun fichier n'est modifié par cette page.
          </p>
        </div>
        <button
          className="btn btn-ghost"
          onClick={refresh}
          disabled={loading}
          style={{ marginLeft: 'auto' }}
          title="Re-scanner les fichiers home"
        >
          <RefreshCw size={14} className={loading ? 'spin' : ''} />
        </button>
      </div>

      {error && (
        <div className="alert alert-warning" style={{ marginTop: 12 }}>
          <AlertTriangle size={14} />
          <span>{error}</span>
        </div>
      )}

      {!error && loadedAt && (
        <div className="text-muted" style={{ fontSize: '0.85em', marginTop: 8 }}>
          {totalCount} MCP{totalCount !== 1 ? 's' : ''} détecté{totalCount !== 1 ? 's' : ''}
          {totalCount > 0 && (
            <> — <strong>{managedCount}</strong> géré{managedCount !== 1 ? 's' : ''} par Kronn, <strong>{externalCount}</strong> externe{externalCount !== 1 ? 's' : ''}</>
          )}
        </div>
      )}

      {!loading && entries.length === 0 && !error && (
        <div className="text-muted" style={{ marginTop: 12, padding: 12, border: '1px dashed var(--kr-border, #ccc)', borderRadius: 4 }}>
          Aucun MCP détecté dans les 4 fichiers home scannés. Si tu as configuré un MCP via <code>claude mcp add</code> ou similaire, vérifie que la commande s'est bien exécutée.
        </div>
      )}

      {sortedGroups.map(group => (
        <div key={scopeKey(group.scope)} style={{ marginTop: 16 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
            <FileText size={14} className="text-muted" />
            <strong style={{ fontSize: '0.95em' }}>{scopeLabel(group.scope)}</strong>
            <span className="badge" style={{ marginLeft: 'auto' }}>{group.items.length}</span>
          </div>
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: '0.9em' }}>
            <thead>
              <tr style={{ borderBottom: '1px solid var(--kr-border, #ccc)' }}>
                <th style={{ textAlign: 'left', padding: '6px 8px', fontWeight: 500, color: 'var(--kr-text-muted)' }}>Nom</th>
                <th style={{ textAlign: 'left', padding: '6px 8px', fontWeight: 500, color: 'var(--kr-text-muted)' }}>Transport</th>
                <th style={{ textAlign: 'left', padding: '6px 8px', fontWeight: 500, color: 'var(--kr-text-muted)' }}>Env keys</th>
                <th style={{ textAlign: 'left', padding: '6px 8px', fontWeight: 500, color: 'var(--kr-text-muted)' }}>Statut</th>
                <th style={{ textAlign: 'left', padding: '6px 8px', fontWeight: 500, color: 'var(--kr-text-muted)' }}>Action</th>
              </tr>
            </thead>
            <tbody>
              {group.items.map((entry, i) => (
                <tr key={`${scopeKey(entry.scope)}-${entry.name}-${i}`} style={{ borderBottom: '1px solid var(--kr-border-subtle, #eee)' }}>
                  <td style={{ padding: '6px 8px', fontWeight: 500 }}>{entry.name}</td>
                  <td style={{ padding: '6px 8px', fontFamily: 'var(--kr-font-mono, monospace)', fontSize: '0.85em', color: 'var(--kr-text-muted)' }}>
                    {transportSummary(entry)}
                  </td>
                  <td style={{ padding: '6px 8px', fontSize: '0.85em' }}>
                    {entry.env_keys.length === 0 ? (
                      <span className="text-muted">—</span>
                    ) : (
                      <span title={entry.env_keys.join(', ')}>
                        <Eye size={11} style={{ verticalAlign: 'text-bottom', marginRight: 4 }} />
                        {entry.env_keys.length} clé{entry.env_keys.length !== 1 ? 's' : ''}
                      </span>
                    )}
                  </td>
                  <td style={{ padding: '6px 8px' }}>
                    <OwnershipBadge ownership={entry.managed_by_kronn} />
                  </td>
                  <td style={{ padding: '6px 8px' }}>
                    {entry.managed_by_kronn.type === 'NotManaged' && (
                      <button
                        className="btn btn-xs btn-ghost"
                        onClick={() => { setAdoptError(null); setAdoptTarget(entry); }}
                        title="Importer dans Kronn (ne touche pas au fichier home — l'entrée externe reste telle quelle)"
                      >
                        <Plus size={11} style={{ verticalAlign: 'text-bottom', marginRight: 4 }} />
                        Importer
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ))}

      {adoptTarget && (
        <AdoptModal
          entry={adoptTarget}
          onConfirm={adopt}
          onCancel={() => { setAdoptTarget(null); setAdoptError(null); }}
          loading={adopting}
          error={adoptError}
        />
      )}
    </div>
  );
}

interface AdoptModalProps {
  entry: DiscoveredHostMcp;
  onConfirm: () => void;
  onCancel: () => void;
  loading: boolean;
  error: string | null;
}

/**
 * Confirmation modal — preview of the future McpConfig in plain language.
 * UX#5: avoid jargon (host_sync=GlobalOnly), show the transport + source
 * scope, and offer a fold-out "Détails techniques" for those who want
 * the raw shape.
 */
function AdoptModal({ entry, onConfirm, onCancel, loading, error }: AdoptModalProps) {
  const [showTech, setShowTech] = useState(false);

  return (
    <div
      role="dialog"
      aria-modal="true"
      style={{
        position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.5)',
        display: 'flex', alignItems: 'center', justifyContent: 'center', zIndex: 1000,
      }}
      onClick={onCancel}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          background: 'var(--kr-bg, #fff)', borderRadius: 8, padding: 24,
          maxWidth: 560, width: '90%', maxHeight: '85vh', overflow: 'auto',
          boxShadow: '0 8px 32px rgba(0,0,0,0.3)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 16 }}>
          <h3 style={{ margin: 0 }}>Importer "{entry.name}" dans Kronn ?</h3>
          <button className="btn btn-ghost" onClick={onCancel} disabled={loading} title="Annuler">
            <X size={16} />
          </button>
        </div>

        {/* Plain-language preview */}
        <div style={{ background: 'var(--kr-info-bg, rgba(96, 165, 250, 0.06))', padding: 14, borderRadius: 6, marginBottom: 16, fontSize: '0.9em', lineHeight: 1.55 }}>
          <strong>Ce qui va se passer :</strong>
          <ul style={{ margin: '8px 0 0 20px', padding: 0 }}>
            <li>Kronn ajoute <strong>{entry.name}</strong> à sa liste de plugins</li>
            <li>Tu pourras l'activer sur tes projets depuis la page Plugins</li>
            <li>Par défaut, il sera <strong>exposé en CLI local</strong> (présent dans tes ~/.claude.json, etc.) — modifiable ensuite</li>
            <li>Le fichier source <code>{entry.source_file}</code> n'est <strong>pas modifié</strong></li>
          </ul>
        </div>

        {/* Visual preview of the future config */}
        <div style={{ marginBottom: 12 }}>
          <strong style={{ fontSize: '0.9em' }}>Aperçu :</strong>
          <table style={{ width: '100%', fontSize: '0.85em', marginTop: 6, borderCollapse: 'collapse' }}>
            <tbody>
              <tr>
                <td style={{ padding: 4, color: 'var(--kr-text-muted)', width: 110 }}>Source</td>
                <td style={{ padding: 4 }}>{scopeLabel(entry.scope)}</td>
              </tr>
              <tr>
                <td style={{ padding: 4, color: 'var(--kr-text-muted)' }}>Transport</td>
                <td style={{ padding: 4, fontFamily: 'monospace', fontSize: '0.85em' }}>{transportSummary(entry)}</td>
              </tr>
              {entry.env_keys.length > 0 && (
                <tr>
                  <td style={{ padding: 4, color: 'var(--kr-text-muted)' }}>Variables</td>
                  <td style={{ padding: 4 }}>
                    {entry.env_keys.join(', ')}
                    <span className="text-muted" style={{ marginLeft: 6, fontSize: '0.85em' }}>
                      (les valeurs sont copiées depuis le fichier source)
                    </span>
                  </td>
                </tr>
              )}
              <tr>
                <td style={{ padding: 4, color: 'var(--kr-text-muted)' }}>Portée CLI</td>
                <td style={{ padding: 4 }}>Dans Kronn + CLIs locaux</td>
              </tr>
              <tr>
                <td style={{ padding: 4, color: 'var(--kr-text-muted)' }}>Projets liés</td>
                <td style={{ padding: 4 }}><span className="text-muted">aucun (à choisir après l'import)</span></td>
              </tr>
            </tbody>
          </table>
        </div>

        {/* Technical details — opt-in */}
        <button
          type="button"
          onClick={() => setShowTech(s => !s)}
          style={{
            all: 'unset', cursor: 'pointer', fontSize: '0.8em',
            color: 'var(--kr-text-muted)', marginBottom: 12,
          }}
        >
          {showTech ? '▾' : '▸'} Détails techniques
        </button>
        {showTech && (
          <pre style={{
            background: 'var(--kr-code-bg, rgba(0,0,0,0.04))',
            padding: 10, borderRadius: 4, fontSize: '0.78em',
            margin: '0 0 12px 0', overflow: 'auto',
          }}>{`McpConfig {
  source: ${entry.managed_by_kronn.type === 'NotManaged' ? 'HostImported' : 'Registry'},
  host_sync: GlobalOnly,
  is_global: false,
  project_ids: [],
  env_keys: [${entry.env_keys.map(k => `"${k}"`).join(', ')}],
}`}</pre>
        )}

        {error && (
          <div className="alert alert-warning" style={{ marginBottom: 12 }}>
            <AlertTriangle size={14} />
            <span>{error}</span>
          </div>
        )}

        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
          <button className="btn btn-ghost" onClick={onCancel} disabled={loading}>Annuler</button>
          <button className="btn btn-primary" onClick={onConfirm} disabled={loading}>
            {loading ? 'Import en cours…' : 'Importer'}
          </button>
        </div>
      </div>
    </div>
  );
}
