// Ollama-specific card in the Agents section (v0.4.0).
//
// 4 states:
// 1. not_installed → Install instructions + link
// 2. offline/unreachable → Launch instructions (contextual WSL/macOS/Linux)
// 3. online, 0 models → Pull suggestions
// 4. online + models → Model picker dropdown

import { useState, useEffect, useCallback } from 'react';
import { ollama as ollamaApi, config as configApi } from '../../lib/api';
import type { OllamaHealthResponse, OllamaModel, ModelTiersConfig } from '../../types/generated';
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

/** Discreet "can my hardware run this model?" link.
 *
 *  Surfaced only on local-agent surfaces (Ollama card, the future
 *  local-model SetupWizard step) — never on cloud-only screens. The
 *  external `canirun.ai` lookup answers RAM/VRAM sizing in seconds,
 *  saving the user a 30 GB pull they'd then OOM. */
function CaniRunHint({ t }: { t: (key: string) => string }) {
  return (
    <div className="set-ollama-canirun">
      <a
        href="https://www.canirun.ai/"
        target="_blank"
        rel="noreferrer"
        className="set-ollama-canirun-link"
      >
        <ExternalLink size={9} />
        <span>{t('ollama.canirunHint')}</span>
      </a>
    </div>
  );
}

export function OllamaCard({ t }: OllamaCardProps) {
  const [health, setHealth] = useState<OllamaHealthResponse | null>(null);
  const [models, setModels] = useState<OllamaModel[]>([]);
  const [loading, setLoading] = useState(true);
  // User's preferred default Ollama model — surfaces as the selected
  // radio in the picker. Persisted via the existing model-tier config
  // endpoint (`ModelTierConfig.default` for the Ollama agent). When
  // unset (`null`), the backend falls back to the hardcoded
  // `llama3.2` in `runner.rs:resolve_model_flag`.
  const [defaultModel, setDefaultModel] = useState<string | null>(null);
  const [tiers, setTiers] = useState<ModelTiersConfig | null>(null);
  const [savingDefault, setSavingDefault] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [h, t] = await Promise.all([
        ollamaApi.health(),
        configApi.getModelTiers().catch(() => null),
      ]);
      setHealth(h);
      if (t) {
        setTiers(t);
        setDefaultModel(t.ollama?.default ?? null);
      }
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

  const pickDefaultModel = useCallback(async (name: string) => {
    if (!tiers) return;
    setSavingDefault(true);
    // Optimistic — flip the radio immediately so the click feels responsive.
    // If the POST fails the next refresh corrects it.
    setDefaultModel(name);
    try {
      const next: ModelTiersConfig = {
        ...tiers,
        ollama: { ...tiers.ollama, default: name },
      };
      await configApi.setModelTiers(next);
      setTiers(next);
    } catch (err) {
      console.warn('Failed to save default Ollama model:', err);
      // Roll back optimistic flip on failure.
      setDefaultModel(tiers.ollama?.default ?? null);
    } finally {
      setSavingDefault(false);
    }
  }, [tiers]);

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

      {/* canirun.ai info box — visible right under the title in EVERY
       *  state including `not_installed`. User report 2026-05-11: the
       *  link used to live at the bottom (under "how to start Ollama")
       *  and got skipped by users who pre-emptively assumed their
       *  machine wasn't powerful enough — those are exactly the
       *  people canirun.ai exists for, since the answer is usually
       *  "yes, with X model". Promoted to a discrete info box so it
       *  reads as "FYI before you commit" rather than "after-thought
       *  hint". */}
      <CaniRunHint t={t} />

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

          {/* ── Online + models → picker ──
           *  Each installed model is a radio. The selected one becomes
           *  the Default-tier override (`ModelTierConfig.default`) that
           *  `runner.rs:resolve_model_flag` reads before falling back
           *  to the hardcoded `llama3.2`. Effective immediately — no
           *  Save button. */}
          {health.status === 'online' && models.length > 0 && (
            <div className="set-ollama-models">
              <div className="text-xs text-muted mb-2">{t('ollama.installedModels')}</div>
              <div className="set-ollama-model-list" role="radiogroup" aria-label={t('ollama.installedModels')}>
                {models.map(m => {
                  const isDefault = defaultModel === m.name;
                  return (
                    <button
                      key={m.name}
                      type="button"
                      role="radio"
                      aria-checked={isDefault}
                      className="set-ollama-model-row set-ollama-model-row-pickable"
                      data-selected={isDefault}
                      onClick={() => pickDefaultModel(m.name)}
                      disabled={savingDefault}
                    >
                      <span
                        className="set-ollama-radio"
                        aria-hidden="true"
                        data-checked={isDefault}
                      >
                        {isDefault && <Check size={8} style={{ color: 'var(--kr-success)' }} />}
                      </span>
                      <code className="set-ollama-model-name">{m.name}</code>
                      <span className="text-ghost text-2xs">{m.size}</span>
                      {isDefault && (
                        <span className="set-ollama-default-tag">{t('ollama.defaultTag')}</span>
                      )}
                    </button>
                  );
                })}
              </div>
              <div className="set-ollama-pull-hint">
                <span className="text-2xs text-muted">
                  {defaultModel ? t('ollama.pickedDefaultHint', defaultModel) : t('ollama.pickDefaultHint')}
                  {' · '}
                  {t('ollama.pullMoreHint')}
                </span>
              </div>
            </div>
          )}

        </div>
      )}
    </div>
  );
}
