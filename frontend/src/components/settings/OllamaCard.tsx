// Ollama-specific card in the Agents section (v0.4.0).
//
// 4 states:
// 1. not_installed → Install instructions + link
// 2. offline/unreachable → Launch instructions (contextual WSL/macOS/Linux)
// 3. online, 0 models → Pull suggestions
// 4. online + models → Model picker dropdown

import { useState, useEffect, useCallback, type ReactNode } from 'react';
import { ollama as ollamaApi, config as configApi } from '../../lib/api';
import type { OllamaHealthResponse, OllamaModel, ModelTiersConfig } from '../../types/generated';
import { RefreshCw, ExternalLink, Download, AlertTriangle, Loader2, Save, RotateCcw } from 'lucide-react';
import { SUGGESTED_MODELS } from './ollamaModels';
import '../../pages/SettingsPage.css';

interface OllamaCardProps {
  t: (key: string, ...args: (string | number)[]) => string;
  modelCostSuffix?: (model: string) => string;
  headerAccessory?: ReactNode;
}

interface ContextFeedback {
  warnings: string[];
  error?: string;
}

const CONTEXT_FLOOR = 2_048;
const CONTEXT_OVERRIDE_MAX = 1_048_576;

function formatContextTokens(value: number | null): string {
  return value == null ? '—' : value.toLocaleString();
}

// Hardware tier of a suggested model — drives a badge so users don't pull a
// 19 GB model onto an 8 GB no-GPU laptop. Kronn runs on Windows/WSL boxes with
// no GPU too, not just beefy Macs.
/** Discreet "can my hardware run this model?" link.
 *
 *  Surfaced only on local-agent surfaces (Ollama card, the future
 *  local-model SetupWizard step) — never on cloud-only screens. The
 *  external `canirun.ai` lookup answers RAM/VRAM sizing in seconds,
 *  saving the user a 30 GB pull they'd then OOM. */
function CaniRunHint({ t }: { t: (key: string) => string }) {
  return (
    <a
      href="https://www.canirun.ai/"
      target="_blank"
      rel="noreferrer"
      className="set-ollama-canirun"
    >
      <ExternalLink size={14} />
      <span>{t('ollama.canirunHint')}</span>
    </a>
  );
}

export function OllamaCard({ t, modelCostSuffix, headerAccessory }: OllamaCardProps) {
  const [health, setHealth] = useState<OllamaHealthResponse | null>(null);
  const [models, setModels] = useState<OllamaModel[]>([]);
  const [loading, setLoading] = useState(true);
  // Source of truth for the per-tier model choice is `tiers.ollama.{economy,
  // default,reasoning}`; the selects read it directly.
  const [tiers, setTiers] = useState<ModelTiersConfig | null>(null);
  // Which tier row is mid-save (disables just that <select>).
  const [savingTier, setSavingTier] = useState<'economy' | 'default' | 'reasoning' | null>(null);
  const [contextDrafts, setContextDrafts] = useState<Record<string, string>>({});
  const [savingContext, setSavingContext] = useState<string | null>(null);
  const [contextFeedback, setContextFeedback] = useState<Record<string, ContextFeedback>>({});

  const syncModels = useCallback((nextModels: OllamaModel[]) => {
    setModels(nextModels);
    setContextDrafts(Object.fromEntries(
      nextModels.map(model => [model.name, model.context_override?.toString() ?? '']),
    ));
  }, []);

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
      }
      if (h.status === 'online') {
        const m = await ollamaApi.models();
        syncModels(m.models);
      } else {
        syncModels([]);
      }
    } catch {
      setHealth({ status: 'offline', version: null, endpoint: '', models_count: 0, hint: null });
    } finally {
      setLoading(false);
    }
  }, [syncModels]);

  useEffect(() => {
    let active = true;
    Promise.all([
      ollamaApi.health(),
      configApi.getModelTiers().catch(() => null),
    ])
      .then(async ([nextHealth, nextTiers]) => {
        if (!active) return;
        setHealth(nextHealth);
        if (nextTiers) setTiers(nextTiers);
        if (nextHealth.status === 'online') {
          const nextModels = await ollamaApi.models();
          if (active) syncModels(nextModels.models);
        } else {
          syncModels([]);
        }
      })
      .catch(() => {
        if (active) {
          setHealth({ status: 'offline', version: null, endpoint: '', models_count: 0, hint: null });
        }
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => { active = false; };
  }, [syncModels]);

  // Assign an installed model to a tier (economy/default/reasoning) — ISO with
  // the per-tier picking every other agent gets in AgentsSection. `null` clears
  // the slot back to the backend built-in. Empty economy/reasoning slots fall
  // back to the `default` slot for Ollama (runner::resolve_model_flag), so
  // setting only `default` still applies everywhere — these two just let you
  // pick a lighter model for cheap steps and a stronger one for reasoning.
  const pickTierModel = useCallback(async (
    tier: 'economy' | 'default' | 'reasoning',
    name: string | null,
  ) => {
    if (!tiers) return;
    setSavingTier(tier);
    const prev = tiers;
    const next: ModelTiersConfig = {
      ...tiers,
      ollama: { ...tiers.ollama, [tier]: name },
    };
    // Optimistic — reflect immediately; rollback on failure.
    setTiers(next);
    try {
      await configApi.setModelTiers(next);
    } catch (err) {
      console.warn('Failed to save Ollama tier model:', err);
      setTiers(prev);
    } finally {
      setSavingTier(null);
    }
  }, [tiers]);

  const saveContextOverride = useCallback(async (model: OllamaModel, reset = false) => {
    const raw = contextDrafts[model.name]?.trim() ?? '';
    const parsed = Number(raw);
    if (!reset && (
      raw === '' || !Number.isInteger(parsed)
      || parsed < CONTEXT_FLOOR || parsed > CONTEXT_OVERRIDE_MAX
    )) {
      setContextFeedback(prev => ({
        ...prev,
        [model.name]: { warnings: [], error: t('ollama.contextInvalid') },
      }));
      return;
    }
    const value = reset ? null : parsed;

    setSavingContext(model.name);
    setContextFeedback(prev => ({ ...prev, [model.name]: { warnings: [] } }));
    try {
      const result = await ollamaApi.setContextOverride(model.name, value);
      try {
        const refreshed = await ollamaApi.models();
        syncModels(refreshed.models);
      } catch {
        // The mutation is already durable. Keep the UI honest even if the
        // follow-up probe temporarily fails; the Refresh button can recover
        // trained-window/origin details later.
        setModels(prev => prev.map(item => item.name === model.name ? {
          ...item,
          context_override: result.num_ctx,
          context_ceiling: result.num_ctx ?? item.context_ceiling,
          context_origin: result.num_ctx == null ? 'refresh_required' : 'model_override',
        } : item));
        setContextDrafts(prev => ({
          ...prev,
          [model.name]: result.num_ctx?.toString() ?? '',
        }));
      }
      setContextFeedback(prev => ({
        ...prev,
        [model.name]: { warnings: result.warnings },
      }));
    } catch (error) {
      setContextFeedback(prev => ({
        ...prev,
        [model.name]: {
          warnings: [],
          error: error instanceof Error ? error.message : t('ollama.contextSaveFailed'),
        },
      }));
    } finally {
      setSavingContext(null);
    }
  }, [contextDrafts, syncModels, t]);

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
          <div className="set-ollama-header-actions">
            {headerAccessory}
            <button className="set-icon-btn" onClick={refresh} title={t('ollama.refresh')} aria-label={t('ollama.refresh')}>
              <RefreshCw size={11} className={loading ? 'spin' : ''} />
            </button>
          </div>
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
                    <div className="set-ollama-suggestion-head">
                      <code className="set-ollama-cmd">ollama pull {m.name}</code>
                      <span className={`set-ollama-tier set-ollama-tier-${m.tier}`}>
                        {t(`ollama.tier.${m.tier}`)}
                      </span>
                    </div>
                    <span className="set-ollama-suggestion-desc">{t(m.descKey)} · {m.size}</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* ── Online + models → per-tier pickers ──
           *  One selector per model tier (economy/default/reasoning), ISO with
           *  the per-tier model choice every other agent gets in AgentsSection.
           *  Writes `ModelTierConfig.{economy,default,reasoning}` for Ollama,
           *  read by `runner.rs:resolve_model_flag`. Empty economy/reasoning
           *  fall back to `default` (Ollama has no built-in tiers). Effective
           *  immediately — no Save button. */}
          {health.status === 'online' && models.length > 0 && (
            <div className="set-ollama-models">
              <div className="text-xs text-muted mb-2">{t('ollama.tierPickerTitle')}</div>
              <div className="set-ollama-tier-grid">
                {(['economy', 'default', 'reasoning'] as const).map(tier => (
                  <label key={tier} className="set-ollama-tier-row">
                    {/* Same tier emotes as every other agent (AgentsSection). */}
                    <span className="set-ollama-tier-label">
                      <span aria-hidden="true" style={{ marginRight: 4 }}>
                        {tier === 'economy' ? '⚡' : tier === 'reasoning' ? '🧠' : '🎯'}
                      </span>
                      {t(`disc.tier.${tier}`)}
                    </span>
                    <select
                      className="set-ollama-tier-select"
                      data-model-tier-agent="Ollama"
                      data-model-tier={tier}
                      value={tiers?.ollama?.[tier] ?? ''}
                      disabled={savingTier === tier}
                      onChange={e => pickTierModel(tier, e.target.value || null)}
                      aria-label={t(`disc.tier.${tier}`)}
                    >
                      <option value="">{t('ollama.tierAuto')}</option>
                      {models.map(m => (
                        <option key={m.name} value={m.name}>
                          {m.name} · {m.size}{modelCostSuffix?.(m.name) ?? ''}
                        </option>
                      ))}
                    </select>
                  </label>
                ))}
              </div>
              {/* Bench-based guidance (2026-07) — which local model fits which
                  job, so users don't put a weak model on a demanding step. */}
              <div className="set-ollama-tier-guidance">💡 {t('ollama.tierGuidance')}</div>
              <details className="set-ollama-context-section">
                <summary className="set-ollama-context-summary text-xs text-muted">
                  {t('ollama.contextTitle')}
                </summary>
                <div className="set-ollama-context-list">
                  {models.map(model => {
                    const feedback = contextFeedback[model.name];
                    const isSaving = savingContext === model.name;
                    return (
                      <div className="set-ollama-context-card" key={model.name}>
                        <div className="set-ollama-context-head">
                          <code className="set-ollama-model-name">{model.name}</code>
                          <span className="text-2xs text-muted">{model.size}</span>
                        </div>
                        <div className="set-ollama-context-metrics">
                          <span>{t('ollama.contextAdvertised')} <strong>{formatContextTokens(model.advertised_context)}</strong></span>
                          <span>{t('ollama.contextCeiling')} <strong>{formatContextTokens(model.context_ceiling)}</strong></span>
                          <span>{t('ollama.contextOriginLabel')} <strong>{t(`ollama.contextOrigin.${model.context_origin}`)}</strong></span>
                        </div>
                        {model.context_origin === 'portable_fallback' && (
                          <div className="set-ollama-context-alert" role="alert">
                            <AlertTriangle size={12} />
                            <span>{t('ollama.contextFallbackWarning')}</span>
                          </div>
                        )}
                        <div className="set-ollama-context-editor">
                          <label htmlFor={`ollama-context-${model.name}`}>{t('ollama.contextOverride')}</label>
                          <input
                            id={`ollama-context-${model.name}`}
                            type="number"
                            min={CONTEXT_FLOOR}
                            max={CONTEXT_OVERRIDE_MAX}
                            step={1024}
                            value={contextDrafts[model.name] ?? ''}
                            placeholder={t('ollama.contextAuto')}
                            disabled={isSaving}
                            aria-label={t('ollama.contextOverrideFor', model.name)}
                            onChange={event => setContextDrafts(prev => ({
                              ...prev,
                              [model.name]: event.target.value,
                            }))}
                          />
                          <button
                            type="button"
                            className="set-ollama-context-action"
                            disabled={isSaving}
                            onClick={() => saveContextOverride(model)}
                          >
                            {isSaving ? <Loader2 size={11} className="spin" /> : <Save size={11} />}
                            {t('ollama.contextSave')}
                          </button>
                          <button
                            type="button"
                            className="set-ollama-context-action"
                            disabled={isSaving || model.context_override == null}
                            onClick={() => saveContextOverride(model, true)}
                          >
                            <RotateCcw size={11} />
                            {t('ollama.contextReset')}
                          </button>
                        </div>
                        {feedback?.error && (
                          <div className="set-ollama-context-error" role="alert">{feedback.error}</div>
                        )}
                        {feedback?.warnings.map(warning => (
                          <div className="set-ollama-context-alert" role="alert" key={warning}>
                            <AlertTriangle size={12} />
                            <span>{warning}</span>
                          </div>
                        ))}
                      </div>
                    );
                  })}
                </div>
                <p className="set-ollama-context-hint">{t('ollama.contextHint')}</p>
              </details>
              <div className="set-ollama-pull-hint">
                <span className="text-2xs text-muted">
                  {t('ollama.tierPickerHint')}
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
