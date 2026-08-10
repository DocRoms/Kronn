// LiteLLM card in the Agents section.
//
// Two steps, deliberately: unlike Ollama, a LiteLLM proxy can live anywhere
// and may sit behind a key, so there is nothing to auto-detect. The user
// declares endpoint + key and proves them with a test. Once saved, that
// configuration remains visible even while the proxy is temporarily
// unreachable; connectivity must never look like lost credentials.
//
//   1. Connect  → endpoint + key + "Test connection"
//   2. Models   → per-tier pickers, fed by what the proxy declares
//
// Step 2 stays reachable via "Change endpoint" so a working setup can be
// re-pointed without losing the tier choices.

import { useState, useEffect, useCallback, type ReactNode } from 'react';
import { liteLlm as liteLlmApi, config as configApi } from '../../lib/api';
import type {
  LiteLlmHealthResponse,
  LiteLlmModel,
  LiteLlmModelFailure,
  ModelTiersConfig,
} from '../../types/generated';
import { RefreshCw, AlertTriangle, Loader2, Plug, Check, Pencil, RotateCcw, X } from 'lucide-react';
import '../../pages/SettingsPage.css';

interface LiteLlmCardProps {
  t: (key: string, ...args: (string | number)[]) => string;
  modelCostSuffix?: (model: string) => string;
  headerAccessory?: ReactNode;
}

type Phase = 'connect' | 'models';
const MODEL_NOT_IN_CATALOGUE = 'kronn:model-not-in-catalogue';

interface LiteLlmSnapshot {
  health: LiteLlmHealthResponse;
  models: LiteLlmModel[];
  catalogueUnavailable: boolean;
  tiers: ModelTiersConfig | null;
  failures: LiteLlmModelFailure[];
  phase: Phase;
}

function compactFailure(error: string): string {
  let message = error;
  try {
    const parsed = JSON.parse(error) as { error?: { message?: unknown } };
    if (typeof parsed.error?.message === 'string') message = parsed.error.message;
  } catch { /* the runner may have stored a plain-text diagnostic */ }
  // LiteLLM often nests the Vertex JSON as text inside its own JSON error.
  // Prefer the innermost publisher message: it is the part an operator can
  // act on, while the full untouched payload remains available via `title`.
  const upstream = /["']message["']\s*:\s*["']([^"']+)/i.exec(message)?.[1];
  if (upstream) message = upstream;
  const compact = message.replace(/\\n|\s+/g, ' ').trim();
  return compact.length > 180 ? `${compact.slice(0, 177)}…` : compact;
}

function isRemovedFromCatalogue(failure: LiteLlmModelFailure): boolean {
  return failure.status_code === 410 && failure.error_message === MODEL_NOT_IN_CATALOGUE;
}

function failureDate(value: string): string {
  const normalized = value.includes('T') ? value : `${value.replace(' ', 'T')}Z`;
  const date = new Date(normalized);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

async function fetchLiteLlmSnapshot(): Promise<LiteLlmSnapshot> {
  const [health, tiers, failureResponse] = await Promise.all([
    liteLlmApi.health(),
    configApi.getModelTiers().catch(() => null),
    liteLlmApi.modelFailures().catch(() => ({ failures: [] })),
  ]);
  let models: LiteLlmModel[] = [];
  let catalogueUnavailable = false;
  if (health.status === 'online') {
    try {
      models = (await liteLlmApi.models()).models;
    } catch {
      catalogueUnavailable = true;
    }
  }
  return {
    health,
    models,
    catalogueUnavailable,
    tiers,
    failures: failureResponse.failures,
    phase: health.status === 'online' || health.configured ? 'models' : 'connect',
  };
}

export function LiteLlmCard({ t, modelCostSuffix, headerAccessory }: LiteLlmCardProps) {
  const [health, setHealth] = useState<LiteLlmHealthResponse | null>(null);
  const [models, setModels] = useState<LiteLlmModel[]>([]);
  const [catalogueUnavailable, setCatalogueUnavailable] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tiers, setTiers] = useState<ModelTiersConfig | null>(null);
  const [failures, setFailures] = useState<LiteLlmModelFailure[]>([]);
  const [savingTier, setSavingTier] = useState<'economy' | 'default' | 'reasoning' | null>(null);
  const [retryingModel, setRetryingModel] = useState<string | null>(null);
  const [forgettingModel, setForgettingModel] = useState<string | null>(null);
  const [retryNotice, setRetryNotice] = useState<string | null>(null);

  // Connect-form state. The key is write-only: the backend never sends it
  // back, so an empty field means "keep whatever is stored".
  const [endpoint, setEndpoint] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [keyTouched, setKeyTouched] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testError, setTestError] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>('connect');

  const applySnapshot = useCallback((snapshot: LiteLlmSnapshot) => {
    setHealth(snapshot.health);
    if (!snapshot.catalogueUnavailable) setModels(snapshot.models);
    setCatalogueUnavailable(snapshot.catalogueUnavailable);
    if (snapshot.tiers) setTiers(snapshot.tiers);
    setFailures(snapshot.failures);
    if (snapshot.health.endpoint) setEndpoint(prev => prev || snapshot.health.endpoint);
    setPhase(snapshot.phase);
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      applySnapshot(await fetchLiteLlmSnapshot());
      setLoadError(null);
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : t('liteLlm.loadErrorDesc'));
    } finally {
      setLoading(false);
    }
  }, [applySnapshot, t]);

  useEffect(() => {
    let cancelled = false;
    void fetchLiteLlmSnapshot()
      .then(snapshot => {
        if (cancelled) return;
        applySnapshot(snapshot);
        setLoadError(null);
      })
      .catch(error => {
        if (cancelled) return;
        setLoadError(error instanceof Error ? error.message : t('liteLlm.loadErrorDesc'));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => { cancelled = true; };
  }, [applySnapshot, t]);

  const runTest = useCallback(async () => {
    setTesting(true);
    setTestError(null);
    try {
      const res = await liteLlmApi.test({
        base_url: endpoint,
        // Only send the key when the user typed one, so re-testing an existing
        // setup doesn't wipe a working credential.
        api_key: keyTouched ? apiKey : null,
      });
      if (res.ok) {
        setModels(res.models);
        setKeyTouched(false);
        setApiKey('');
        setPhase('models');
        setTestError(res.hint ?? null);
        await load();
      } else {
        setTestError(res.hint ?? t('liteLlm.testFailed'));
      }
    } catch (err) {
      setTestError(err instanceof Error ? err.message : t('liteLlm.testFailed'));
    } finally {
      setTesting(false);
    }
  }, [endpoint, apiKey, keyTouched, load, t]);

  // Assign a proxy model to a tier. Empty economy/reasoning fall back to
  // `default` (runner::resolve_model_flag), so setting only `default` works.
  const pickTierModel = useCallback(async (
    tier: 'economy' | 'default' | 'reasoning',
    name: string | null,
  ) => {
    if (!tiers) return;
    setSavingTier(tier);
    const prev = tiers;
    const next: ModelTiersConfig = {
      ...tiers,
      lite_llm: { ...tiers.lite_llm, [tier]: name },
    };
    setTiers(next); // optimistic
    try {
      await configApi.setModelTiers(next);
    } catch (err) {
      console.warn('Failed to save LiteLLM tier model:', err);
      setTiers(prev);
    } finally {
      setSavingTier(null);
    }
  }, [tiers]);

  const online = health?.status === 'online';
  const verifiedOnline = online && !loadError;
  const configured = health?.configured === true;
  const failureByModel = new Map(failures.map(failure => [failure.model, failure]));
  const modelOptions = [...models];
  for (const selected of Object.values(tiers?.lite_llm ?? {})) {
    if (selected && !modelOptions.some(model => model.id === selected)) {
      modelOptions.push({ id: selected, backing_model: null, provider: null });
    }
  }

  const retryModel = useCallback(async (model: string) => {
    setRetryingModel(model);
    setRetryNotice(null);
    try {
      const result = await liteLlmApi.retryModel(model);
      if (result.healthy) {
        setFailures(current => current.filter(failure => failure.model !== model));
        setRetryNotice(t('liteLlm.failureRecovered', model));
      } else if (result.failure) {
        const failure = result.failure;
        setFailures(current => [
          failure,
          ...current.filter(failure => failure.model !== model),
        ]);
        setRetryNotice(isRemovedFromCatalogue(failure)
          ? t('liteLlm.failureRemovedNotice', model)
          : t('liteLlm.failureStillDown', model, failure.status_code));
      }
    } catch (error) {
      setRetryNotice(error instanceof Error ? error.message : t('liteLlm.failureRetryFailed'));
    } finally {
      setRetryingModel(null);
    }
  }, [t]);

  const forgetModelFailure = useCallback(async (model: string) => {
    setForgettingModel(model);
    setRetryNotice(null);
    try {
      await liteLlmApi.forgetModelFailure(model);
      setFailures(current => current.filter(failure => failure.model !== model));
      setRetryNotice(t('liteLlm.failureForgotten', model));
    } catch (error) {
      setRetryNotice(error instanceof Error ? error.message : t('liteLlm.failureForgetFailed'));
    } finally {
      setForgettingModel(null);
    }
  }, [t]);
  const statusColor = verifiedOnline
    ? 'var(--kr-success)'
    : configured
      ? 'var(--kr-warning)'
      : health?.status === 'not_installed'
      ? 'var(--kr-text-ghost)'
      : 'var(--kr-warning)';

  const statusLabel = loadError
    ? t('liteLlm.stateUnavailable')
    : online
    ? `${t('liteLlm.online')} — ${health?.models_count ?? 0} ${t('liteLlm.models')}`
    : health?.status === 'unauthorized'
        ? t('liteLlm.unauthorized')
        : configured
          ? t('liteLlm.offline')
          : health?.status === 'not_installed'
            ? t('liteLlm.notInstalled')
            : t('liteLlm.notConfigured');

  return (
    <div className="set-ollama-card">
      <div className="set-ollama-header">
        <div className="flex-row gap-4" style={{ alignItems: 'center' }}>
          <div className="set-dot" data-on={verifiedOnline} aria-hidden="true" />
          <span className="font-semibold text-base">LiteLLM</span>
          <span className="set-ollama-status" style={{ color: statusColor }}>
            {loading ? <Loader2 size={10} className="spin" /> : statusLabel}
          </span>
          <div className="set-ollama-header-actions">
            {headerAccessory}
            <button
              className="set-icon-btn"
              onClick={() => void load()}
              title={t('liteLlm.refresh')}
              aria-label={t('liteLlm.refresh')}
            >
              <RefreshCw size={11} className={loading ? 'spin' : ''} />
            </button>
          </div>
        </div>
      </div>

      {!loading && (
        <div className="set-ollama-body">

          {loadError && (
            <div className="set-litellm-unavailable" role="alert">
              <AlertTriangle size={16} aria-hidden="true" />
              <div>
                <strong>{t('liteLlm.loadErrorTitle')}</strong>
                <span>{t('liteLlm.loadErrorDesc')}</span>
                <small>{t('liteLlm.savedUnavailablePreserved')}</small>
              </div>
              <button
                type="button"
                className="set-btn-secondary set-litellm-reconnect"
                onClick={() => void load()}
              >
                <RefreshCw size={11} />
                {t('liteLlm.retryConnection')}
              </button>
            </div>
          )}

          {/* ── Step 1 — connect ── */}
          {!loadError && phase === 'connect' && (
            <div className="set-ollama-wizard">
              <div className="set-ollama-wizard-title">
                <Plug size={14} /> {t('liteLlm.connectTitle')}
              </div>
              <p className="set-ollama-wizard-desc">{t('liteLlm.connectDesc')}</p>

              <label className="set-litellm-field">
                <span className="set-litellm-label">{t('liteLlm.endpointLabel')}</span>
                <input
                  className="set-litellm-input"
                  type="url"
                  inputMode="url"
                  placeholder="http://localhost:4000"
                  value={endpoint}
                  onChange={e => setEndpoint(e.target.value)}
                  aria-label={t('liteLlm.endpointLabel')}
                />
              </label>

              <label className="set-litellm-field">
                <span className="set-litellm-label">{t('liteLlm.keyLabel')}</span>
                <input
                  className="set-litellm-input"
                  type="password"
                  autoComplete="off"
                  placeholder={health?.configured ? t('liteLlm.keyKept') : t('liteLlm.keyOptional')}
                  value={apiKey}
                  onChange={e => { setApiKey(e.target.value); setKeyTouched(true); }}
                  aria-label={t('liteLlm.keyLabel')}
                />
              </label>

              <button
                className="set-btn-primary"
                onClick={() => void runTest()}
                disabled={testing || !endpoint.trim()}
              >
                {testing ? <Loader2 size={12} className="spin" /> : <Plug size={12} />}
                {' '}{t('liteLlm.testBtn')}
              </button>

              {testError && (
                <pre className="set-ollama-hint">
                  <AlertTriangle size={12} /> {testError}
                </pre>
              )}
              {!testError && health?.hint && (
                <pre className="set-ollama-hint">{health.hint}</pre>
              )}
            </div>
          )}

          {/* ── Step 2 — model tiers ── */}
          {phase === 'models' && (
            <div className="set-ollama-models">
              <div className="set-litellm-connected">
                {verifiedOnline
                  ? <Check size={12} style={{ color: 'var(--kr-success)' }} />
                  : <AlertTriangle size={12} style={{ color: 'var(--kr-warning-amber)' }} />}
                <code className="set-litellm-endpoint">{health?.endpoint}</code>
                <button
                  className="set-icon-btn"
                  onClick={() => setPhase('connect')}
                  title={t('liteLlm.changeEndpoint')}
                  aria-label={t('liteLlm.changeEndpoint')}
                >
                  <Pencil size={11} />
                </button>
              </div>

              {!loadError && !online && configured && (
                <div className="set-litellm-unavailable" role="alert">
                  <AlertTriangle size={16} aria-hidden="true" />
                  <div>
                    <strong>{t('liteLlm.savedUnavailableTitle')}</strong>
                    <span>
                      {health?.status === 'unauthorized' && health.hint
                        ? health.hint
                        : t('liteLlm.savedUnavailableDesc')}
                    </span>
                    <small>{t('liteLlm.savedUnavailablePreserved')}</small>
                  </div>
                  <button
                    type="button"
                    className="set-btn-secondary set-litellm-reconnect"
                    onClick={() => void load()}
                    disabled={loading}
                  >
                    <RefreshCw size={11} className={loading ? 'spin' : ''} />
                    {t('liteLlm.retryConnection')}
                  </button>
                </div>
              )}

              {catalogueUnavailable && (
                <div className="set-litellm-unavailable" role="alert">
                  <AlertTriangle size={16} aria-hidden="true" />
                  <div>
                    <strong>{t('liteLlm.catalogueUnavailableTitle')}</strong>
                    <span>{t('liteLlm.catalogueUnavailableDesc')}</span>
                    <small>{t('liteLlm.savedUnavailablePreserved')}</small>
                  </div>
                </div>
              )}

              {online && !catalogueUnavailable && models.length === 0 ? (
                <div className="set-ollama-wizard">
                  <div className="set-ollama-wizard-title">
                    <AlertTriangle size={14} /> {t('liteLlm.noModelsTitle')}
                  </div>
                  <p className="set-ollama-wizard-desc">{t('liteLlm.noModelsDesc')}</p>
                  <code className="set-ollama-cmd">litellm --config config.yaml</code>
                </div>
              ) : (
                <>
                  <div className="text-xs text-muted mb-2">{t('liteLlm.tierPickerTitle')}</div>
                  {/* Where the models really come from. A proxy alias hides
                      this by design, and "which machine answers" decides both
                      cost and confidentiality. */}
                  {models.length > 0 && (
                    <div className="set-litellm-providers">
                      {Array.from(
                        models.reduce((acc, m) => {
                          const key = m.provider ?? t('liteLlm.providerUnknown');
                          acc.set(key, (acc.get(key) ?? 0) + 1);
                          return acc;
                        }, new Map<string, number>()),
                      ).map(([provider, count]) => (
                        <span key={provider} className="set-litellm-provider-chip">
                          {provider} · {count}
                        </span>
                      ))}
                    </div>
                  )}
                  <div className="set-ollama-tier-grid">
                    {(['economy', 'default', 'reasoning'] as const).map(tier => (
                      <label key={tier} className="set-ollama-tier-row">
                        <span className="set-ollama-tier-label">
                          <span aria-hidden="true" style={{ marginRight: 4 }}>
                            {tier === 'economy' ? '⚡' : tier === 'reasoning' ? '🧠' : '🎯'}
                          </span>
                          {t(`disc.tier.${tier}`)}
                        </span>
                        <select
                          className="set-ollama-tier-select"
                          data-model-tier-agent="LiteLlm"
                          data-model-tier={tier}
                          value={tiers?.lite_llm?.[tier] ?? ''}
                          disabled={!verifiedOnline || catalogueUnavailable || savingTier === tier}
                          onChange={e => void pickTierModel(tier, e.target.value || null)}
                          aria-label={t(`disc.tier.${tier}`)}
                        >
                          <option value="">{t('liteLlm.tierAuto')}</option>
                          {/* Show what ACTUALLY answers, from /model/info —
                              never `owned_by`, which LiteLLM hardcodes to
                              "openai" even for a local Ollama model. */}
                          {modelOptions.map(m => {
                            const modelFailure = failureByModel.get(m.id);
                            return (
                              <option key={m.id} value={m.id} disabled={Boolean(modelFailure)}>
                                {m.id}{m.backing_model ? ` → ${m.backing_model}` : ''}
                                {modelCostSuffix?.(m.backing_model ?? m.id) ?? ''}
                                {modelFailure
                                  ? isRemovedFromCatalogue(modelFailure)
                                    ? ` · ⚠ ${t('liteLlm.failureRemoved')}`
                                    : ` · ⚠ HTTP ${modelFailure.status_code}`
                                  : ''}
                              </option>
                            );
                          })}
                        </select>
                      </label>
                    ))}
                  </div>
                  {failures.length > 0 && (
                    <div className="set-litellm-failures" data-testid="litellm-model-failures">
                      <div className="set-litellm-failures-head">
                        <div>
                          <strong><AlertTriangle size={12} /> {t('liteLlm.failuresTitle')}</strong>
                          <span>{t('liteLlm.failuresHint')}</span>
                        </div>
                        <span className="set-accordion-count">{failures.length}</span>
                      </div>
                      <div
                        className="set-litellm-failure-table"
                        role="table"
                        aria-label={t('liteLlm.failuresTitle')}
                      >
                        <div className="set-litellm-failure-header" role="row">
                          <span role="columnheader">{t('liteLlm.failureModel')}</span>
                          <span role="columnheader">{t('liteLlm.failureError')}</span>
                          <span role="columnheader">{t('liteLlm.failureDate')}</span>
                          <span role="columnheader" className="set-sr-only">
                            {t('liteLlm.failureRetry')}
                          </span>
                        </div>
                        {failures.map(failure => (
                          <div className="set-litellm-failure-row" role="row" key={failure.model}>
                            <code role="cell" title={failure.model}>{failure.model}</code>
                            <span role="cell" title={failure.error_message}>
                              <b>
                                {isRemovedFromCatalogue(failure)
                                  ? t('liteLlm.failureRemoved')
                                  : `HTTP ${failure.status_code}`}
                              </b>
                              {!isRemovedFromCatalogue(failure) && (
                                <>{' · '}{compactFailure(failure.error_message)}</>
                              )}
                              {failure.failure_count > 1 && (
                                <small> ×{failure.failure_count}</small>
                              )}
                            </span>
                            <time role="cell" dateTime={failure.last_failed_at}>
                              {failureDate(failure.last_failed_at)}
                            </time>
                            <div role="cell" className="set-litellm-retry-cell">
                              <button
                                type="button"
                                className="set-btn-secondary set-litellm-retry"
                                onClick={() => void retryModel(failure.model)}
                                disabled={!online || retryingModel !== null}
                                title={t('liteLlm.failureRetryHint')}
                              >
                                {retryingModel === failure.model
                                  ? <Loader2 size={11} className="spin" />
                                  : <RotateCcw size={11} />}
                                {t('liteLlm.failureRetry')}
                              </button>
                              <button
                                type="button"
                                className="set-litellm-forget"
                                onClick={() => void forgetModelFailure(failure.model)}
                                disabled={forgettingModel !== null || retryingModel !== null}
                                aria-label={t('liteLlm.failureForget', failure.model)}
                                title={t('liteLlm.failureForgetHint')}
                              >
                                {forgettingModel === failure.model
                                  ? <Loader2 size={12} className="spin" />
                                  : <X size={13} />}
                              </button>
                            </div>
                          </div>
                        ))}
                      </div>
                    </div>
                  )}
                  {retryNotice && <div className="set-litellm-retry-notice" role="status">{retryNotice}</div>}
                  <div className="set-ollama-pull-hint">
                    <span className="text-2xs text-muted">
                      {verifiedOnline && !catalogueUnavailable
                        ? t('liteLlm.tierPickerHint')
                        : t('liteLlm.tierPickerOfflineHint')}
                    </span>
                  </div>
                </>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
