// Ollama-specific card in the Agents section (v0.4.0).
//
// 4 states:
// 1. not_installed → Install instructions + link
// 2. offline/unreachable → Launch instructions (contextual WSL/macOS/Linux)
// 3. online, 0 models → Pull suggestions (block open: first use)
// 4. online + models → Context-window controls; the download block is folded
// Durable catalogue model choices remain visible in every settled state.
// A download or an update in flight stays visible whether the block is folded
// or not.
// What the official library says (an update is available, a suggestion's size)
// arrives from a separate, slower call and fills in when it lands; the card
// never waits on it, and anything it could not confirm reads as "not checked",
// never as "up to date".

import { useState, useEffect, useCallback, useRef, type ReactNode } from 'react';
import { ollama as ollamaApi, config as configApi, modelCatalogApi, type OllamaPullProgress } from '../../lib/api';
import { useApi } from '../../hooks/useApi';
import { usePersistentFold } from '../../hooks/usePersistentFold';
import { catalogModelOptions, catalogTierEntry, modelRuntimeTargetId } from '../../lib/modelCatalogSelection';
import type { OllamaHealthResponse, OllamaModel, OllamaRegistryResponse, OllamaUpdateStatus, ModelTiersConfig } from '../../types/generated';
import { RefreshCw, ExternalLink, Download, AlertTriangle, Loader2, Save, RotateCcw, ChevronRight } from 'lucide-react';
import { suggestedModelsFor } from './ollamaModels';
import { SearchableSelect } from '../SearchableSelect';
import '../../pages/SettingsPage.css';

interface OllamaCardProps {
  t: (key: string, ...args: (string | number)[]) => string;
  modelCostSuffix?: (model: string) => string;
  headerAccessory?: ReactNode;
  /** KT-586 — lets the settings page pass the name-as-mention-colour control,
   *  so the card says "Ollama" once instead of once as a title and again as a
   *  colour chip beside it. */
  title?: ReactNode;
}

interface ContextFeedback {
  warnings: string[];
  error?: string;
}

interface PullState {
  progress: OllamaPullProgress;
  error?: string;
}

const CONTEXT_FLOOR = 2_048;
const CONTEXT_OVERRIDE_MAX = 1_048_576;
/** The user's own fold of the download block; absent until they touch it. */
const DOWNLOAD_FOLD_KEY = 'kronn:ollamaDownloadOpen';

function formatContextTokens(value: number | null): string {
  return value == null ? '—' : value.toLocaleString();
}

function formatDownloadBytes(value: number | null): string {
  if (value == null) return '—';
  return `${(value / 1_000_000).toLocaleString(undefined, { maximumFractionDigits: 1 })} MB`;
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

/** What the official library says about an installed model, next to its Update
 *  action. Nothing until the library has answered; "not checked" (with the
 *  reason on hover) whenever it could not confirm either way. */
function FreshnessBadge({ status, t }: { status: OllamaUpdateStatus | undefined; t: OllamaCardProps['t'] }) {
  if (!status) return null;
  return (
    <span
      className={`set-ollama-fresh set-ollama-fresh-${status}`}
      title={status === 'unknown' ? t('ollama.fresh.unknownHint') : undefined}
    >
      {t(`ollama.fresh.${status}`)}
    </span>
  );
}

/** One download or update, with its progress and its cancel. Rendered outside
 *  the foldable block, so a pull started from inside it is not hidden by
 *  folding it back. */
function PullProgressRow({ model, state, active, onCancel, t }: {
  model: string;
  state: PullState;
  active: boolean;
  onCancel: (model: string) => void;
  t: OllamaCardProps['t'];
}) {
  const { progress, error } = state;
  return (
    <div className="set-ollama-pull-progress" role="status">
      <div className="set-ollama-pull-progress-head">
        <code className="set-ollama-model-name">{model}</code>
        <span>{progress.status}</span>
        {active && (
          <button type="button" className="set-ollama-pull-cancel" onClick={() => onCancel(model)}>
            {t('ollama.pullCancel')}
          </button>
        )}
      </div>
      {progress.total != null && (
        <progress value={progress.completed ?? 0} max={progress.total ?? undefined} />
      )}
      <span className="set-ollama-suggestion-desc">
        {formatDownloadBytes(progress.completed)} / {formatDownloadBytes(progress.total)}
        {progress.total != null && ` · ${Math.round(((progress.completed ?? 0) / (progress.total ?? 1)) * 100)}%`}
      </span>
      {error && <div className="set-ollama-context-error" role="alert">{error}</div>}
    </div>
  );
}

export function OllamaCard({ t, modelCostSuffix, headerAccessory, title }: OllamaCardProps) {
  const [health, setHealth] = useState<OllamaHealthResponse | null>(null);
  const [models, setModels] = useState<OllamaModel[]>([]);
  const [loading, setLoading] = useState(true);
  // Source of truth for the per-tier model choice is `tiers.ollama.{economy,
  // default,reasoning}`; the selects read it directly.
  const [tiers, setTiers] = useState<ModelTiersConfig | null>(null);
  // A tier save locks all tier controls until the confirmed write settles.
  const [savingTier, setSavingTier] = useState<'economy' | 'default' | 'reasoning' | null>(null);
  const savingTierRef = useRef(false);
  const refreshingRef = useRef(false);
  const [tierError, setTierError] = useState<string | null>(null);
  const [tiersLoadError, setTiersLoadError] = useState(false);
  const catalog = useApi(() => modelCatalogApi.list(), []);
  const reloadCatalog = catalog.refetch;
  const [contextDrafts, setContextDrafts] = useState<Record<string, string>>({});
  const [savingContext, setSavingContext] = useState<string | null>(null);
  const savingContextRef = useRef(false);
  const [contextFeedback, setContextFeedback] = useState<Record<string, ContextFeedback>>({});
  const [pulls, setPulls] = useState<Record<string, PullState>>({});
  const [activePulls, setActivePulls] = useState<Set<string>>(() => new Set());
  const pullControllers = useRef(new Map<string, AbortController>());
  // Open for a first use (nothing installed yet), folded once there is a model:
  // the block is a way to ADD one, and should not take the card over forever.
  const [downloadOpen, toggleDownload] = usePersistentFold(DOWNLOAD_FOLD_KEY, models.length === 0);
  // The official library's answer, `null` until it lands. Bumping the tick asks
  // again (after a refresh, after an update changed what is installed).
  const [registry, setRegistry] = useState<OllamaRegistryResponse | null>(null);
  const [registryTick, setRegistryTick] = useState(0);
  // Tags just pulled: what the library said about them described the copy that
  // was replaced, so they show no verdict until it has been asked again.
  const [withheld, setWithheld] = useState<Set<string>>(() => new Set());

  const syncModels = useCallback((nextModels: OllamaModel[]) => {
    setModels(nextModels);
    setContextDrafts(Object.fromEntries(
      nextModels.map(model => [model.name, model.context_override?.toString() ?? '']),
    ));
  }, []);

  const refresh = useCallback(async () => {
    if (refreshingRef.current || savingTierRef.current || savingContextRef.current) return;
    refreshingRef.current = true;
    setLoading(true);
    setRegistryTick(tick => tick + 1);
    try {
      const [h, t] = await Promise.all([
        ollamaApi.health(),
        configApi.getModelTiers().catch(() => null),
      ]);
      setHealth(h);
      setTiersLoadError(t === null);
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
      setHealth({ status: 'offline', version: null, endpoint: '', models_count: 0, hint: null, mlx_capable: false });
    } finally {
      reloadCatalog();
      refreshingRef.current = false;
      setLoading(false);
    }
  }, [syncModels, reloadCatalog]);

  useEffect(() => {
    let active = true;
    Promise.all([
      ollamaApi.health(),
      configApi.getModelTiers().catch(() => null),
    ])
      .then(async ([nextHealth, nextTiers]) => {
        if (!active) return;
        setHealth(nextHealth);
        setTiersLoadError(nextTiers === null);
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
          setHealth({ status: 'offline', version: null, endpoint: '', models_count: 0, hint: null, mlx_capable: false });
        }
      })
      .finally(() => {
        if (active) {
          setLoading(false);
          reloadCatalog();
        }
      });
    return () => { active = false; };
  }, [syncModels, reloadCatalog]);

  // Explicit overrides only. Automatic resolution uses durable assignments,
  // never an embedded model. Merge one field into fresh settings so editing a
  // local model preserves other fields changed since this card was loaded.
  // This read/merge/write is not a cross-client transaction.
  const pickTierModel = useCallback(async (
    tier: 'economy' | 'default' | 'reasoning',
    name: string | null,
  ) => {
    if (!tiers || loading || refreshingRef.current || savingTierRef.current) return;
    savingTierRef.current = true;
    setSavingTier(tier);
    setTierError(null);
    try {
      const fresh = await configApi.getModelTiers();
      const next: ModelTiersConfig = { ...fresh, ollama: { ...fresh.ollama, [tier]: name } };
      await configApi.setModelTiers(next);
      setTiers(next);
    } catch {
      setTierError(t('config.saveError'));
    } finally {
      savingTierRef.current = false;
      setSavingTier(null);
    }
  }, [tiers, loading, t]);

  const saveContextOverride = useCallback(async (model: OllamaModel, reset = false) => {
    if (savingContextRef.current || refreshingRef.current || loading) return;
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

    savingContextRef.current = true;
    setSavingContext(model.name);
    setContextFeedback(prev => ({ ...prev, [model.name]: { warnings: [] } }));
    try {
      const result = await ollamaApi.setContextOverride(model.name, value);
      try {
        const refreshed = await ollamaApi.models();
        syncModels(refreshed.models);
        reloadCatalog();
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
      savingContextRef.current = false;
      setSavingContext(null);
    }
  }, [contextDrafts, syncModels, t, reloadCatalog, loading]);

  const startPull = useCallback(async (model: string) => {
    if (pullControllers.current.has(model)) return;
    const controller = new AbortController();
    pullControllers.current.set(model, controller);
    setActivePulls(prev => new Set(prev).add(model));
    setPulls(prev => ({
      ...prev,
      [model]: { progress: { status: t('ollama.pullStarting'), digest: null, completed: null, total: null } },
    }));
    let succeeded = false;
    try {
      await ollamaApi.pull(model, {
        onProgress: progress => setPulls(prev => ({ ...prev, [model]: { progress } })),
        onSuccess: progress => {
          succeeded = true;
          setPulls(prev => ({ ...prev, [model]: { progress } }));
        },
        onError: error => setPulls(prev => ({
          ...prev,
          [model]: { progress: prev[model]?.progress ?? { status: '', digest: null, completed: null, total: null }, error },
        })),
      }, controller.signal);
      if (succeeded) {
        try {
          const refreshed = await ollamaApi.models();
          syncModels(refreshed.models);
          reloadCatalog();
          setHealth(prev => prev ? { ...prev, models_count: refreshed.models.length } : prev);
          setWithheld(prev => new Set(prev).add(model));
          setRegistryTick(tick => tick + 1);
        } catch (error) {
          setPulls(prev => ({
            ...prev,
            [model]: {
              progress: prev[model]?.progress ?? { status: 'success', digest: null, completed: null, total: null },
              error: `The model downloaded successfully, but Kronn could not refresh the installed-model list: ${error instanceof Error ? error.message : t('ollama.pullFailed')}`,
            },
          }));
        }
      }
    } catch (error) {
      if (!controller.signal.aborted) {
        setPulls(prev => ({
          ...prev,
          [model]: { progress: prev[model]?.progress ?? { status: '', digest: null, completed: null, total: null }, error: error instanceof Error ? error.message : t('ollama.pullFailed') },
        }));
      }
    } finally {
      pullControllers.current.delete(model);
      setActivePulls(prev => {
        const next = new Set(prev);
        next.delete(model);
        return next;
      });
      if (controller.signal.aborted) {
        setPulls(prev => {
          const remaining = { ...prev };
          delete remaining[model];
          return remaining;
        });
      }
    }
  }, [syncModels, t, reloadCatalog]);

  const cancelPull = useCallback((model: string) => {
    pullControllers.current.get(model)?.abort();
  }, []);

  useEffect(() => () => {
    pullControllers.current.forEach(controller => controller.abort());
  }, []);

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

  // The backend decides whether `-mlx` builds will run here (a Mac on Apple
  // Silicon, an Ollama that runs MLX) — never this browser's user agent.
  const suggestions = suggestedModelsFor(health?.mlx_capable === true);

  const online = health?.status === 'online';
  const suggestedTags = suggestions.map(m => m.name).join(',');
  const installedTags = models.map(m => m.name).join(',');
  useEffect(() => {
    if (!online) {
      setRegistry(null);
      return;
    }
    let active = true;
    // Deliberately apart from `loading`: the card is drawn from the local
    // answers, and this fills in whatever the library says whenever it
    // arrives, or never. A failure is an empty answer, so every installed
    // model reads "not checked" instead of silently showing nothing.
    Promise.resolve()
      .then(() => ollamaApi.registry(suggestedTags ? suggestedTags.split(',') : []))
      .then(answer => {
        if (!active) return;
        setRegistry(answer);
        setWithheld(new Set());
      })
      .catch(() => {
        if (!active) return;
        setRegistry({ models: [], suggestions: [] });
        setWithheld(new Set());
      });
    return () => { active = false; };
  }, [online, suggestedTags, installedTags, registryTick]);

  /** `undefined` until the library has answered (and while a verdict is
   *  withheld); then the verdict, or "unknown" for a model it said nothing
   *  about. */
  const freshnessOf = (name: string): OllamaUpdateStatus | undefined =>
    registry && !withheld.has(name)
      ? (registry.models.find(item => item.name === name)?.status ?? 'unknown')
      : undefined;
  const sizeOf = (name: string): string | undefined =>
    registry?.suggestions.find(item => item.name === name)?.size;
  const updatesAvailable = models.filter(model => freshnessOf(model.name) === 'update_available').length;

  const savedTarget = catalog.data?.targets.find(view => view.runtime_target_id === modelRuntimeTargetId('Ollama'));
  const target = savedTarget && (!savedTarget.live_refresh_ok || catalog.error || catalog.loading || health?.status !== 'online')
    ? { ...savedTarget, stale: true, live_refresh_ok: false } : savedTarget;

  return (
    <div className="set-ollama-card">
      {/* Header with status pill */}
      <div className="set-ollama-header">
        <div className="flex-row gap-4" style={{ alignItems: 'center' }}>
          <div className="set-dot" data-on={health?.status === 'online'} aria-hidden="true" />
          {title ?? <span className="font-semibold text-base">Ollama</span>}
          <span className="set-ollama-status" style={{ color: statusColor }}>
            {loading ? <Loader2 size={10} className="spin" /> : statusLabel}
          </span>
          <div className="set-ollama-header-actions">
            {headerAccessory}
            <button className="set-icon-btn" onClick={refresh} disabled={loading || savingTier !== null || savingContext !== null} title={t('ollama.refresh')} aria-label={t('ollama.refresh')}>
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

          {/* ── Online: downloads in flight, then the download block ── */}
          {health.status === 'online' && Object.keys(pulls).length > 0 && (
            <div className="set-ollama-pulls">
              {Object.entries(pulls).map(([model, state]) => (
                <PullProgressRow
                  key={model}
                  model={model}
                  state={state}
                  active={activePulls.has(model)}
                  onCancel={cancelPull}
                  t={t}
                />
              ))}
            </div>
          )}
          {health.status === 'online' && (
            <details className="set-ollama-wizard set-ollama-download" open={downloadOpen}>
              {/* Driven by state, not by the browser's own toggle: the fold is
                  remembered, and has to behave the same wherever it runs. */}
              <summary
                className="set-ollama-download-summary"
                onClick={event => { event.preventDefault(); toggleDownload(); }}
              >
                <ChevronRight size={12} className="set-accordion-chevron" data-expanded={downloadOpen} aria-hidden="true" />
                <Download size={14} />
                <span className="set-ollama-download-title">{t('ollama.pullTitle')}</span>
                {!downloadOpen && (
                  <span className="set-ollama-download-meta">
                    {t('ollama.pullSummarySuggestions', suggestions.length)}
                    {updatesAvailable > 0 && ` · ${t('ollama.pullSummaryUpdates', updatesAvailable)}`}
                    {activePulls.size > 0 && ` · ${t('ollama.pullSummaryActive', activePulls.size)}`}
                  </span>
                )}
              </summary>
              <p className="set-ollama-wizard-desc">{t(models.length === 0 ? 'ollama.pullDesc' : 'ollama.pullMoreDesc')}</p>
              <div className="set-ollama-suggestions">
                {suggestions.map(m => {
                  const installed = models.some(model => model.name === m.name);
                  return (
                    <div key={m.name} className="set-ollama-suggestion">
                      <div className="set-ollama-suggestion-head">
                        <code className="set-ollama-cmd">{m.name}</code>
                        <span className={`set-ollama-tier set-ollama-tier-${m.tier}`}>
                          {t(`ollama.tier.${m.tier}`)}
                        </span>
                        {m.mlx && (
                          <span className="set-ollama-tier set-ollama-tier-mlx">{t('ollama.mlxBadge')}</span>
                        )}
                        {sizeOf(m.name) && (
                          <span className="set-ollama-suggestion-desc">{sizeOf(m.name)}</span>
                        )}
                        <button
                          type="button"
                          className="set-ollama-pull-button"
                          disabled={activePulls.has(m.name) || installed}
                          onClick={() => startPull(m.name)}
                        >
                          {activePulls.has(m.name) ? <Loader2 size={12} className="spin" /> : <Download size={12} />}
                          {installed ? t('ollama.pullInstalled') : t('ollama.pullButton')}
                        </button>
                      </div>
                      <span className="set-ollama-suggestion-desc">{t(m.descKey)}</span>
                    </div>
                  );
                })}
              </div>
              {models.length > 0 && (
                <div className="set-ollama-installed">
                  <div className="set-ollama-installed-title">{t('ollama.installedModels')}</div>
                  <div className="set-ollama-suggestions">
                    {models.map(model => (
                      <div key={model.name} className="set-ollama-suggestion">
                        <div className="set-ollama-suggestion-head">
                          <code className="set-ollama-cmd">{model.name}</code>
                          <span className="set-ollama-suggestion-desc">{model.size}</span>
                          <FreshnessBadge status={freshnessOf(model.name)} t={t} />
                          <button
                            type="button"
                            className="set-ollama-pull-button"
                            disabled={activePulls.has(model.name)}
                            aria-label={t('ollama.updateFor', model.name)}
                            onClick={() => startPull(model.name)}
                          >
                            {activePulls.has(model.name) ? <Loader2 size={12} className="spin" /> : <RefreshCw size={12} />}
                            {t('ollama.updateButton')}
                          </button>
                        </div>
                      </div>
                    ))}
                  </div>
                  <p className="set-ollama-suggestion-desc">{t('ollama.updateHint')}</p>
                </div>
              )}
            </details>
          )}

          {/* Catalogue choices remain visible offline; installed inventory is
              used only for local context/download controls and size hints. */}
          <div className="set-ollama-models">
            <div className="text-xs text-muted mb-2">{t('ollama.tierPickerTitle')}</div>
            {tiersLoadError && <p className="set-ollama-context-error" role="alert">{t('common.error')} — {t('ollama.tierPickerTitle')}</p>}
            {tierError && <p className="set-ollama-context-error" role="alert">{tierError}</p>}
            {catalog.error && <p className="set-hint" role="alert">
              {t('modelCatalog.loadError')}{' '}
              <button type="button" className="set-icon-btn" disabled={catalog.loading} onClick={reloadCatalog}>{t('modelCatalog.reload')}</button>
            </p>}
            {catalog.loading && <p className="set-hint" role="status">{t('common.loading')}</p>}
            {!catalog.loading && !catalog.error && !target?.models.length && <p className="set-hint">{t('modelCatalog.empty')}</p>}
            {target?.stale && <p className="set-hint">{t('modelCatalog.stale')}{target.last_error_reason ? ` — ${target.last_error_reason}` : ''}</p>}
            <div className="set-ollama-tier-grid">
              {(['economy', 'default', 'reasoning'] as const).map(tier => {
                const configuredDefault = tier === 'default' ? '' : tiers?.ollama.default || '';
                const fallback = catalogTierEntry(target, tier, configuredDefault, true);
                const fallbackId = configuredDefault || fallback?.model_id;
                const fallbackDetail = fallback?.availability === 'unavailable' ? ` — ${t('modelCatalog.unavailable')}` : '';
                const clearLabel = `${t('ollama.tierAuto')}${fallbackId ? ` (${fallbackId}${fallbackDetail})` : ''}`;
                const options = catalogModelOptions(target, tiers?.ollama[tier] ?? '', t, model => modelCostSuffix?.(model) ?? '')
                  .map(option => ({ ...option, description: [option.description, models.find(model => model.name === option.value)?.size].filter(Boolean).join(' · ') }));
                return (
                  <div key={tier} className="set-ollama-tier-row">
                    {/* Same tier emotes as every other agent (AgentsSection). */}
                    <span className="set-ollama-tier-label">
                      <span aria-hidden="true" style={{ marginRight: 4 }}>
                        {tier === 'economy' ? '⚡' : tier === 'reasoning' ? '🧠' : '🎯'}
                      </span>
                      {t(`disc.tier.${tier}`)}
                    </span>
                    <SearchableSelect
                      className="searchable-select--compact"
                      value={tiers?.ollama?.[tier] ?? ''}
                      options={options}
                      disabled={!tiers || savingTier !== null || catalog.loading}
                      onChange={value => void pickTierModel(tier, value || null)}
                      label={t(`disc.tier.${tier}`)}
                      placeholder={tiers?.ollama[tier] ? t('config.searchModel') : clearLabel}
                      emptyLabel={t('modelCatalog.empty')}
                      clearLabel={clearLabel}
                      dataModelTierAgent="Ollama"
                      dataModelTier={tier}
                    />
                  </div>
                );
              })}
            </div>
            {/* Bench-based guidance (2026-07) — which local model fits which
                job, so users don't put a weak model on a demanding step. */}
            <div className="set-ollama-tier-guidance">💡 {t('ollama.tierGuidance')}</div>
            {health.status === 'online' && models.length > 0 && <details className="set-ollama-context-section">
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
                          disabled={savingContext !== null}
                          aria-label={t('ollama.contextOverrideFor', model.name)}
                          onChange={event => setContextDrafts(prev => ({
                            ...prev,
                            [model.name]: event.target.value,
                          }))}
                        />
                        <button
                          type="button"
                          className="set-ollama-context-action"
                          disabled={savingContext !== null}
                          onClick={() => saveContextOverride(model)}
                        >
                          {isSaving ? <Loader2 size={11} className="spin" /> : <Save size={11} />}
                          {t('ollama.contextSave')}
                        </button>
                        <button
                          type="button"
                          className="set-ollama-context-action"
                          disabled={savingContext !== null || model.context_override == null}
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
            </details>}
            <div className="set-ollama-pull-hint">
              <span className="text-2xs text-muted">
                {t('ollama.tierPickerHint')}
                {' · '}
                {t('ollama.pullMoreHint')}
              </span>
            </div>
          </div>

        </div>
      )}
    </div>
  );
}
