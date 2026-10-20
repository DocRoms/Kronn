import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, Download, ExternalLink, Loader2, RefreshCw, X } from 'lucide-react';
import { comfyui, type ComfyUiPullProgress } from '../../lib/api';
import type { ComfyUiStatus, ComfyUiSuggestion, MediaModality } from '../../types/generated';
import { SearchableSelect } from '../SearchableSelect';
import { comfyUiSlotOptions } from '../../lib/comfyUi';

interface ComfyUiCardProps {
  t: (key: string, ...args: (string | number)[]) => string;
}

const SLOTS: { modality: MediaModality; icon: string; field: 'image_model' | 'video_model' }[] = [
  { modality: 'image', icon: '🖼', field: 'image_model' },
  { modality: 'video', icon: '🎬', field: 'video_model' },
];

function formatGigabytes(bytes: number): string {
  return `${(bytes / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 1 })} GB`;
}

/** KT-1381 — ComfyUI as a local media source, laid out like the Ollama card. */
export function ComfyUiCard({ t }: ComfyUiCardProps) {
  const [status, setStatus] = useState<ComfyUiStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState<MediaModality | null>(null);
  const savingRef = useRef(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setStatus(await comfyui.status());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    comfyui.status()
      .then(next => { if (active) setStatus(next); })
      .catch(e => { if (active) setError(e instanceof Error ? e.message : String(e)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, []);

  const pick = useCallback(async (modality: MediaModality, value: string | null) => {
    if (!status || savingRef.current) return;
    savingRef.current = true;
    setSaving(modality);
    setError(null);
    try {
      const saved = await comfyui.setSlots({
        image_model: modality === 'image' ? value : status.image_model,
        video_model: modality === 'video' ? value : status.video_model,
      });
      setStatus(current => current && { ...current, image_model: saved.image_model ?? null, video_model: saved.video_model ?? null });
      // The video row says what the newly chosen model still lacks.
      if (modality === 'video') void refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      savingRef.current = false;
      setSaving(null);
    }
  }, [status, refresh]);

  // KT-1385 — one download at a time: they are 3 to 17 GB each.
  const [pull, setPull] = useState<{ id: string; progress: ComfyUiPullProgress | null; error: string | null; active: boolean } | null>(null);
  const pullAbort = useRef<AbortController | null>(null);
  useEffect(() => () => pullAbort.current?.abort(), []);
  const startPull = useCallback((suggestion: ComfyUiSuggestion) => {
    if (pullAbort.current) return;
    const controller = new AbortController();
    pullAbort.current = controller;
    setPull({ id: suggestion.id, progress: null, error: null, active: true });
    const settle = (error: string | null) => {
      pullAbort.current = null;
      setPull(current => current && { ...current, error, active: false });
      if (!error) void refresh();
    };
    void comfyui.pull(suggestion.id, {
      onProgress: progress => setPull(current => current && { ...current, progress }),
      onSuccess: () => settle(null),
      onError: message => settle(message),
    }, controller.signal);
  }, [refresh]);
  const cancelPull = useCallback(() => {
    pullAbort.current?.abort();
    pullAbort.current = null;
    setPull(null);
  }, []);

  const online = status?.state === 'online';
  const statusLabel = loading ? null
    : online ? t('comfyui.online', status?.version ?? '')
    : status?.state === 'offline' ? t('comfyui.offline') : t('comfyui.notFound');

  return (
    <div className="set-ollama-card" data-testid="comfyui-card">
      <div className="set-ollama-header">
        <div className="flex-row gap-4" style={{ alignItems: 'center' }}>
          <div className="set-dot" data-on={online} aria-hidden="true" />
          <span className="font-semibold text-base">ComfyUI</span>
          <span className="set-ollama-status" data-state={status?.state}>
            {loading ? <Loader2 size={10} className="spin" /> : statusLabel}
          </span>
          <div className="set-ollama-header-actions">
            <button className="set-icon-btn" onClick={() => void refresh()} disabled={loading || saving !== null} title={t('ollama.refresh')} aria-label={t('ollama.refresh')}>
              <RefreshCw size={11} className={loading ? 'spin' : ''} />
            </button>
          </div>
        </div>
      </div>

      {error && <p className="set-ollama-context-error" role="alert">{error}</p>}

      {!loading && status && (
        <div className="set-ollama-body">
          {status.state === 'not_found' && (
            <div className="set-ollama-wizard">
              <div className="set-ollama-wizard-title">
                <Download size={14} /> {t('comfyui.installTitle')}
              </div>
              <p className="set-ollama-wizard-desc">{t('comfyui.installDesc')}</p>
              {status.install_command && (
                <div className="set-ollama-commands">
                  <div className="set-ollama-cmd-group">
                    <span className="set-ollama-cmd-label">macOS</span>
                    <code className="set-ollama-cmd">{status.install_command}</code>
                  </div>
                </div>
              )}
              <p className="set-ollama-wizard-desc">{t('comfyui.firstLaunch')}</p>
              <a href={status.download_url} target="_blank" rel="noopener noreferrer" className="set-ollama-link">
                <ExternalLink size={10} /> comfy.org/download
              </a>
            </div>
          )}

          {status.state === 'offline' && (
            <div className="set-ollama-wizard">
              <div className="set-ollama-wizard-title">
                <AlertTriangle size={14} /> {t('comfyui.launchTitle')}
              </div>
              <p className="set-ollama-wizard-desc">{t('comfyui.launchDesc', status.endpoint)}</p>
            </div>
          )}

          {/* The slots stay visible offline, like the Ollama tiers: a saved
              choice is never hidden because the server is down. */}
          <div className="set-ollama-models">
            <div className="text-xs text-muted mb-2">{t('comfyui.slotsTitle')}</div>
            <div className="set-ollama-tier-grid">
              {SLOTS.map(({ modality, icon, field }) => {
                const value = status[field] ?? '';
                const options = comfyUiSlotOptions(status.models, modality, t);
                if (value && !options.some(option => option.value === value)) {
                  options.push({ value, label: `${value} — ${t('modelCatalog.notInCatalog')}`, keywords: value, disabled: true });
                }
                return (
                  <div key={modality} className="set-ollama-tier-row">
                    <span className="set-ollama-tier-label">
                      <span aria-hidden="true" style={{ marginRight: 4 }}>{icon}</span>
                      {t(`comfyui.slot.${modality}`)}
                    </span>
                    <SearchableSelect
                      className="searchable-select--compact"
                      value={value}
                      options={options}
                      disabled={saving !== null}
                      onChange={next => void pick(modality, next || null)}
                      label={t(`comfyui.slot.${modality}`)}
                      placeholder={t('comfyui.none')}
                      emptyLabel={online ? t('comfyui.noModel') : t('comfyui.offlineModels')}
                      clearLabel={t('comfyui.none')}
                      testId={`comfyui-slot-${modality}`}
                    />
                  </div>
                );
              })}
            </div>
            {status.video_model && status.video_blocker && (
              <p className="set-ollama-context-error" role="alert">{status.video_blocker}</p>
            )}
            <div className="set-ollama-tier-guidance">💡 {t('comfyui.guidance')}</div>
          </div>

          {online && status.suggestions.length > 0 && (
            <div className="set-ollama-installed" data-testid="comfyui-suggestions">
              <div className="set-ollama-installed-title">
                <Download size={12} /> {t('comfyui.downloadTitle')}
              </div>
              <div className="set-ollama-suggestions">
                {status.suggestions.map(suggestion => {
                  const running = pull?.id === suggestion.id && pull.active;
                  return (
                    <div key={suggestion.id} className="set-ollama-suggestion" data-testid={`comfyui-suggestion-${suggestion.id}`}>
                      <div className="set-ollama-suggestion-head">
                        <span aria-hidden="true">{suggestion.modality === 'image' ? '🖼' : '🎬'}</span>
                        <code className="set-ollama-cmd">{t(`comfyui.suggestion.${suggestion.id}`)}</code>
                        <span className="set-ollama-suggestion-desc">{formatGigabytes(suggestion.bytes)}</span>
                        {suggestion.installed ? (
                          <span className="set-ollama-fresh">{t('comfyui.installed')}</span>
                        ) : running ? (
                          <button type="button" className="set-ollama-pull-cancel" onClick={cancelPull}>
                            <X size={11} /> {t('ollama.pullCancel')}
                          </button>
                        ) : suggestion.downloadable ? (
                          <button
                            type="button"
                            className="set-ollama-pull-button"
                            disabled={pull?.active === true}
                            onClick={() => startPull(suggestion)}
                            data-testid={`comfyui-pull-${suggestion.id}`}
                          >
                            <Download size={12} /> {t('comfyui.download')}
                          </button>
                        ) : (
                          <span className="set-ollama-suggestion-desc">{t('comfyui.notDownloadable')}</span>
                        )}
                      </div>
                      <span className="set-ollama-suggestion-desc">{t(`comfyui.family.${suggestion.family}`)}</span>
                      {pull?.id === suggestion.id && pull.progress && (
                        <div role="status">
                          <progress value={pull.progress.completed} max={pull.progress.total} />
                          <span className="set-ollama-suggestion-desc">
                            {formatGigabytes(pull.progress.completed)} / {formatGigabytes(pull.progress.total)}
                          </span>
                        </div>
                      )}
                      {pull?.id === suggestion.id && pull.error && (
                        <div className="set-ollama-context-error" role="alert">{pull.error}</div>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
