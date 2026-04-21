// Ollama-specific card in the Agents section (v0.4.0).
//
// 4 states:
// 1. not_installed → Install instructions + link
// 2. offline/unreachable → Launch instructions (contextual WSL/macOS/Linux)
// 3. online, 0 models → Pull suggestions
// 4. online + models → Model picker dropdown

import { useState, useEffect, useCallback } from 'react';
import { ollama as ollamaApi } from '../../lib/api';
import type { OllamaHealthResponse, OllamaModel } from '../../types/generated';
import { RefreshCw, ExternalLink, Download, Check, AlertTriangle, Loader2 } from 'lucide-react';
import '../../pages/SettingsPage.css';

interface OllamaCardProps {
  t: (key: string, ...args: (string | number)[]) => string;
}

const SUGGESTED_MODELS = [
  { name: 'llama3.2', desc: 'Meta — bon généraliste, léger (2 GB)', size: '~2 GB' },
  { name: 'gemma4:26b', desc: 'Google — meilleur rapport qualité/vitesse (16 GB)', size: '~16 GB' },
  { name: 'qwen2.5-coder:14b', desc: 'Alibaba — spécialisé code (9 GB)', size: '~9 GB' },
  { name: 'codestral', desc: 'Mistral — spécialisé code (13 GB)', size: '~13 GB' },
];

export function OllamaCard({ t }: OllamaCardProps) {
  const [health, setHealth] = useState<OllamaHealthResponse | null>(null);
  const [models, setModels] = useState<OllamaModel[]>([]);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const h = await ollamaApi.health();
      setHealth(h);
      if (h.status === 'online') {
        const m = await ollamaApi.models();
        setModels(m.models);
      } else {
        setModels([]);
      }
    } catch {
      setHealth({ status: 'offline', version: null, endpoint: '', models_count: 0, hint: null });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { refresh(); }, [refresh]);

  const statusColor = health?.status === 'online'
    ? 'var(--kr-success)'
    : health?.status === 'offline' || health?.status === 'unreachable'
      ? 'var(--kr-warning)'
      : 'var(--kr-text-ghost)';

  const statusLabel = health?.status === 'online'
    ? `${t('ollama.online')} — ${health.models_count} ${t('ollama.models')}`
    : health?.status === 'offline'
      ? t('ollama.offline')
      : health?.status === 'unreachable'
        ? t('ollama.unreachable')
        : t('ollama.notInstalled');

  return (
    <div className="set-ollama-card">
      {/* Header with status pill */}
      <div className="set-ollama-header">
        <div className="flex-row gap-4" style={{ alignItems: 'center' }}>
          <div className="set-dot" data-on={health?.status === 'online'} aria-hidden="true" />
          <span className="font-semibold text-base">Ollama</span>
          <span className="set-ollama-status" style={{ color: statusColor }}>
            {loading ? <Loader2 size={10} className="spin" /> : statusLabel}
          </span>
          <button className="set-icon-btn" onClick={refresh} title={t('ollama.refresh')} aria-label={t('ollama.refresh')} style={{ marginLeft: 'auto' }}>
            <RefreshCw size={11} className={loading ? 'spin' : ''} />
          </button>
        </div>
      </div>

      {/* State-specific content */}
      {!loading && health && (
        <div className="set-ollama-body">

          {/* ── Not installed ── */}
          {health.status === 'not_installed' && (
            <div className="set-ollama-wizard">
              <div className="set-ollama-wizard-title">
                <Download size={14} /> {t('ollama.installTitle')}
              </div>
              <p className="set-ollama-wizard-desc">{t('ollama.installDesc')}</p>
              <div className="set-ollama-commands">
                <div className="set-ollama-cmd-group">
                  <span className="set-ollama-cmd-label">macOS</span>
                  <code className="set-ollama-cmd">brew install ollama</code>
                </div>
                <div className="set-ollama-cmd-group">
                  <span className="set-ollama-cmd-label">Linux / WSL</span>
                  <code className="set-ollama-cmd">curl -fsSL https://ollama.com/install.sh | sh</code>
                </div>
              </div>
              <a href="https://ollama.com" target="_blank" rel="noopener noreferrer" className="set-ollama-link">
                <ExternalLink size={10} /> ollama.com
              </a>
            </div>
          )}

          {/* ── Offline / Unreachable ── */}
          {(health.status === 'offline' || health.status === 'unreachable') && (
            <div className="set-ollama-wizard">
              <div className="set-ollama-wizard-title">
                <AlertTriangle size={14} /> {t('ollama.launchTitle')}
              </div>
              {health.hint && (
                <pre className="set-ollama-hint">{health.hint}</pre>
              )}
              {!health.hint && (
                <p className="set-ollama-wizard-desc">{t('ollama.launchDesc')}</p>
              )}
            </div>
          )}

          {/* ── Online, no models ── */}
          {health.status === 'online' && models.length === 0 && (
            <div className="set-ollama-wizard">
              <div className="set-ollama-wizard-title">
                <Download size={14} /> {t('ollama.pullTitle')}
              </div>
              <p className="set-ollama-wizard-desc">{t('ollama.pullDesc')}</p>
              <div className="set-ollama-suggestions">
                {SUGGESTED_MODELS.map(m => (
                  <div key={m.name} className="set-ollama-suggestion">
                    <code className="set-ollama-cmd">ollama pull {m.name}</code>
                    <span className="set-ollama-suggestion-desc">{m.desc}</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* ── Online + models → picker ── */}
          {health.status === 'online' && models.length > 0 && (
            <div className="set-ollama-models">
              <div className="text-xs text-muted mb-2">{t('ollama.installedModels')}</div>
              <div className="set-ollama-model-list">
                {models.map(m => (
                  <div key={m.name} className="set-ollama-model-row">
                    <Check size={9} style={{ color: 'var(--kr-success)', flexShrink: 0 }} />
                    <code className="set-ollama-model-name">{m.name}</code>
                    <span className="text-ghost text-2xs">{m.size}</span>
                  </div>
                ))}
              </div>
              <div className="set-ollama-pull-hint">
                <span className="text-2xs text-muted">{t('ollama.pullMoreHint')}</span>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
