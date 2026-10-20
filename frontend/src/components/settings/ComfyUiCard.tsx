import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, Download, ExternalLink, Loader2, RefreshCw, Save, X } from 'lucide-react';
import { comfyui, type ComfyUiPullProgress } from '../../lib/api';
import type { ComfyFamily, ComfyGenSettings, ComfyUiModel, ComfyUiStatus, ComfyUiSuggestion, MediaModality } from '../../types/generated';
import { SearchableSelect } from '../SearchableSelect';
import { comfyUiSlotOptions } from '../../lib/comfyUi';

interface ComfyUiCardProps {
  t: (key: string, ...args: (string | number)[]) => string;
}

const SLOTS: { modality: MediaModality; icon: string; field: 'image_model' | 'video_model' }[] = [
  { modality: 'image', icon: '🖼', field: 'image_model' },
  { modality: 'video', icon: '🎬', field: 'video_model' },
];

const FAMILIES_BY_FOLDER: Record<string, ComfyFamily[]> = {
  checkpoints: ['flux_schnell', 'sdxl'],
  diffusion_models: ['wan21_t2v'],
};

interface SlotDraft { steps: string; cfg: string; sampler: string; scheduler: string }
interface SettingsDraft { endpoint: string; installDir: string; image: SlotDraft; video: SlotDraft }

function slotDraft(settings: ComfyGenSettings): SlotDraft {
  return {
    steps: settings.steps != null ? String(settings.steps) : '',
    cfg: settings.cfg != null ? String(settings.cfg) : '',
    sampler: settings.sampler ?? '',
    scheduler: settings.scheduler ?? '',
  };
}

function settingsDraft(status: ComfyUiStatus): SettingsDraft {
  return {
    endpoint: status.configured_endpoint ?? '',
    installDir: status.configured_install_dir ?? '',
    image: slotDraft(status.image_settings),
    video: slotDraft(status.video_settings),
  };
}

/** Empty fields fall back to the family defaults on the server. */
function slotSettings(draft: SlotDraft): ComfyGenSettings {
  return {
    steps: draft.steps.trim() ? Number(draft.steps) : null,
    cfg: draft.cfg.trim() ? Number(draft.cfg) : null,
    sampler: draft.sampler || null,
    scheduler: draft.scheduler || null,
  };
}

/** The family the slot's model runs with, for the placeholders. */
function slotFamily(status: ComfyUiStatus, modality: MediaModality): ComfyFamily | null {
  if (modality === 'video') return 'wan21_t2v';
  return status.models.find(model => model.folder === 'checkpoints' && model.file === status.image_model)?.family ?? null;
}

function choiceOptions(choices: string[], current: string) {
  const options = choices.map(choice => ({ value: choice, label: choice, keywords: choice }));
  if (current && !choices.includes(current)) options.push({ value: current, label: current, keywords: current });
  return options;
}

/** Tells the media surfaces to re-read the local source. */
function announceChange() {
  window.dispatchEvent(new Event('kronn:comfyui-changed'));
}

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
      announceChange();
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

  // Machine-dependent settings: edited as a draft, saved together.
  const [draft, setDraft] = useState<SettingsDraft | null>(null);
  const [savingSettings, setSavingSettings] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const shown = draft ?? (status ? settingsDraft(status) : null);
  const editSlot = (modality: MediaModality, field: keyof SlotDraft, value: string) => {
    if (!shown) return;
    setDraft({ ...shown, [modality]: { ...shown[modality], [field]: value } });
  };
  const saveSettings = useCallback(async () => {
    if (!shown) return;
    setSavingSettings(true);
    setSettingsError(null);
    try {
      await comfyui.setSettings({
        endpoint: shown.endpoint.trim() || null,
        install_dir: shown.installDir.trim() || null,
        image_settings: slotSettings(shown.image),
        video_settings: slotSettings(shown.video),
      });
      setDraft(null);
      announceChange();
      await refresh();
    } catch (e) {
      setSettingsError(e instanceof Error ? e.message : String(e));
    } finally {
      setSavingSettings(false);
    }
  }, [shown, refresh]);

  const [savingFamily, setSavingFamily] = useState<string | null>(null);
  const pickFamily = useCallback(async (model: ComfyUiModel, family: ComfyFamily | null) => {
    const key = `${model.folder}/${model.file}`;
    setSavingFamily(key);
    setError(null);
    try {
      await comfyui.setModelFamily({ folder: model.folder, file: model.file, family });
      announceChange();
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSavingFamily(null);
    }
  }, [refresh]);

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
            {status.models.some(model => !model.family || model.chosen) && (
              <div className="set-ollama-context-list" data-testid="comfyui-families">
                <div className="text-xs text-muted">{t('comfyui.familiesTitle')}</div>
                {status.models.filter(model => !model.family || model.chosen).map(model => {
                  const key = `${model.folder}/${model.file}`;
                  return (
                    <div key={key} className="set-ollama-tier-row">
                      <code className="set-ollama-tier-label" title={key}>{model.file}</code>
                      <SearchableSelect
                        className="searchable-select--compact"
                        value={model.family ?? ''}
                        options={(FAMILIES_BY_FOLDER[model.folder] ?? []).map(family => ({
                          value: family, label: t(`comfyui.family.${family}`), keywords: family,
                        }))}
                        disabled={savingFamily !== null}
                        onChange={next => void pickFamily(model, (next || null) as ComfyFamily | null)}
                        label={t('comfyui.familyFor', model.file)}
                        placeholder={t('comfyui.familyPick')}
                        emptyLabel={t('comfyui.familyPick')}
                        clearLabel={t('comfyui.familyPick')}
                        testId={`comfyui-family-${model.file}`}
                      />
                    </div>
                  );
                })}
              </div>
            )}
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

          {shown && (
            <details className="set-ollama-context-section" data-testid="comfyui-settings">
              <summary className="set-ollama-context-summary text-xs text-muted">{t('comfyui.settingsTitle')}</summary>
              <div className="set-ollama-context-list">
                <div className="set-ollama-context-card">
                  <div className="set-ollama-context-editor">
                    <label htmlFor="comfyui-endpoint">{t('comfyui.settings.endpoint')}</label>
                    <input
                      id="comfyui-endpoint"
                      type="url"
                      value={shown.endpoint}
                      placeholder={status.endpoint}
                      disabled={savingSettings}
                      onChange={event => setDraft({ ...shown, endpoint: event.target.value })}
                    />
                  </div>
                  <div className="set-ollama-context-editor">
                    <label htmlFor="comfyui-install-dir">{t('comfyui.settings.installDir')}</label>
                    <input
                      id="comfyui-install-dir"
                      type="text"
                      value={shown.installDir}
                      placeholder={status.install_dir}
                      disabled={savingSettings}
                      onChange={event => setDraft({ ...shown, installDir: event.target.value })}
                    />
                  </div>
                  <p className="set-ollama-context-hint">{t('comfyui.settings.installDirHint')}</p>
                </div>
                {SLOTS.map(({ modality, icon }) => {
                  const family = slotFamily(status, modality);
                  const defaults = status.family_defaults.find(entry => entry.family === family)?.settings;
                  const slot = shown[modality];
                  const fallback = (value: string | number | undefined) => defaults ? t('comfyui.settings.default', String(value)) : t('comfyui.settings.auto');
                  return (
                    <div key={modality} className="set-ollama-context-card" data-testid={`comfyui-settings-${modality}`}>
                      <div className="set-ollama-context-head">
                        <span><span aria-hidden="true" style={{ marginRight: 4 }}>{icon}</span>{t(`comfyui.slot.${modality}`)}</span>
                        {family && <span className="text-2xs text-muted">{t(`comfyui.family.${family}`)}</span>}
                      </div>
                      <div className="set-ollama-context-editor">
                        <label htmlFor={`comfyui-steps-${modality}`}>{t('comfyui.settings.steps')}</label>
                        <input
                          id={`comfyui-steps-${modality}`}
                          type="number"
                          min={1}
                          max={200}
                          value={slot.steps}
                          placeholder={fallback(defaults?.steps)}
                          disabled={savingSettings}
                          onChange={event => editSlot(modality, 'steps', event.target.value)}
                        />
                        <label htmlFor={`comfyui-cfg-${modality}`}>{t('comfyui.settings.cfg')}</label>
                        <input
                          id={`comfyui-cfg-${modality}`}
                          type="number"
                          min={0}
                          max={30}
                          step={0.5}
                          value={slot.cfg}
                          placeholder={fallback(defaults?.cfg)}
                          disabled={savingSettings}
                          onChange={event => editSlot(modality, 'cfg', event.target.value)}
                        />
                      </div>
                      <div className="set-ollama-tier-row">
                        <span className="set-ollama-tier-label">{t('comfyui.settings.sampler')}</span>
                        <SearchableSelect
                          className="searchable-select--compact"
                          value={slot.sampler}
                          options={choiceOptions(status.samplers, slot.sampler)}
                          disabled={savingSettings}
                          onChange={next => editSlot(modality, 'sampler', next)}
                          label={t('comfyui.settings.sampler')}
                          placeholder={fallback(defaults?.sampler)}
                          emptyLabel={t('comfyui.offlineModels')}
                          clearLabel={fallback(defaults?.sampler)}
                          testId={`comfyui-sampler-${modality}`}
                        />
                      </div>
                      <div className="set-ollama-tier-row">
                        <span className="set-ollama-tier-label">{t('comfyui.settings.scheduler')}</span>
                        <SearchableSelect
                          className="searchable-select--compact"
                          value={slot.scheduler}
                          options={choiceOptions(status.schedulers, slot.scheduler)}
                          disabled={savingSettings}
                          onChange={next => editSlot(modality, 'scheduler', next)}
                          label={t('comfyui.settings.scheduler')}
                          placeholder={fallback(defaults?.scheduler)}
                          emptyLabel={t('comfyui.offlineModels')}
                          clearLabel={fallback(defaults?.scheduler)}
                          testId={`comfyui-scheduler-${modality}`}
                        />
                      </div>
                    </div>
                  );
                })}
              </div>
              <p className="set-ollama-context-hint">{t('comfyui.settings.hint')}</p>
              {settingsError && <div className="set-ollama-context-error" role="alert">{settingsError}</div>}
              <button
                type="button"
                className="set-ollama-context-action"
                disabled={savingSettings || draft === null}
                onClick={() => void saveSettings()}
                data-testid="comfyui-settings-save"
              >
                {savingSettings ? <Loader2 size={11} className="spin" /> : <Save size={11} />}
                {t('comfyui.settings.save')}
              </button>
            </details>
          )}
        </div>
      )}
    </div>
  );
}
