// Unified "External API" settings zone (KT-339).
//
// LiteLLM and NVIDIA used to be two separate cards, but from Kronn's point of
// view they are the same thing: an OpenAI-compatible HTTP connection. This
// section replaces both with ONE list of named connections. Adding a third
// compatible service (Groq, Together, a self-hosted vLLM, …) is done entirely
// here: pick the "Other" preset, type an endpoint + key, and save. No enum
// variant, no dedicated card, no new i18n key — the form is generic.
//
// A preset only pre-fills the endpoint; the persisted connection carries its
// own endpoint, mention alias and per-tier models, so several connections
// (e.g. NVIDIA and Groq) coexist with fully independent settings.

import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { externalApi } from '../../lib/api';
import type {
  ExternalApiConnectionView,
  ExternalApiConnectionTestResult,
  ExternalApiPreset,
  UpsertExternalApiConnection,
} from '../../lib/api';
import { userError } from '../../lib/userError';
import type { ToastFn } from '../../hooks/useToast';
import { ContextHelp } from '../ContextHelp';
import { SearchableSelect } from '../SearchableSelect';
import { SecretField } from '../SecretField';
import {
  Check,
  Key,
  Link2,
  Loader2,
  Pencil,
  PlugZap,
  Plus,
  Save,
  Server,
  Trash2,
  X,
} from 'lucide-react';
import '../../pages/SettingsPage.css';

interface ExternalApiSectionProps {
  t: (key: string, ...args: (string | number)[]) => string;
  toast: ToastFn;
  modelCostSuffix?: (model: string) => string;
  onModelTiersChanged?: () => void;
}

/** A preset only seeds the endpoint field. `endpoint: ''` means "the user must
 *  type it" — exactly the case a brand-new compatible service falls into. */
const PRESET_ENDPOINTS: Record<ExternalApiPreset, string> = {
  lite_llm: 'http://localhost:4000',
  nvidia: 'https://integrate.api.nvidia.com',
  open_router: 'https://openrouter.ai/api/v1',
  other: '',
};

const TIERS = ['economy', 'default', 'reasoning'] as const;
type Tier = (typeof TIERS)[number];
const TIER_ICON: Record<Tier, string> = { economy: '⚡', default: '🎯', reasoning: '🧠' };

interface FormState {
  display_name: string;
  mention_alias: string;
  endpoint: string;
  origin_preset: ExternalApiPreset;
  economy_model: string;
  default_model: string;
  reasoning_model: string;
  image_model: string;
  video_model: string;
  api_key: string;
  keyTouched: boolean;
}

function emptyForm(): FormState {
  return {
    display_name: '',
    mention_alias: '',
    endpoint: '',
    origin_preset: 'lite_llm',
    economy_model: '',
    default_model: '',
    reasoning_model: '',
    image_model: '',
    video_model: '',
    api_key: '',
    keyTouched: false,
  };
}

/** Legacy NVIDIA rows may predate the named-connection store and therefore
 *  carry no persisted endpoint. Presets are executable defaults, not merely
 *  form placeholders, so keep those migrated rows visible and editable. */
function endpointForConnection(c: ExternalApiConnectionView): string | null {
  const saved = c.endpoint?.trim();
  return saved || PRESET_ENDPOINTS[c.origin_preset] || null;
}

function formFromConnection(c: ExternalApiConnectionView): FormState {
  return {
    display_name: c.display_name,
    mention_alias: c.mention_alias,
    endpoint: endpointForConnection(c) ?? '',
    origin_preset: c.origin_preset,
    economy_model: c.economy_model ?? '',
    default_model: c.default_model ?? '',
    reasoning_model: c.reasoning_model ?? '',
    image_model: c.image_model ?? '',
    video_model: c.video_model ?? '',
    api_key: '',
    keyTouched: false,
  };
}

function toPayload(form: FormState): UpsertExternalApiConnection {
  return {
    display_name: form.display_name.trim(),
    mention_alias: form.mention_alias.trim(),
    endpoint: form.endpoint.trim() || null,
    origin_preset: form.origin_preset,
    economy_model: form.economy_model.trim() || null,
    default_model: form.default_model.trim() || null,
    reasoning_model: form.reasoning_model.trim() || null,
    image_model: form.image_model.trim() || null,
    video_model: form.video_model.trim() || null,
    // Only send the key when the user actually typed one, so editing a
    // connection without retyping its key keeps the stored credential.
    api_key: form.keyTouched ? form.api_key : null,
  };
}

function modelsForProbe(connection: Pick<FormState, 'origin_preset' | 'economy_model' | 'default_model' | 'reasoning_model'>): string[] {
  if (connection.origin_preset !== 'nvidia' && connection.origin_preset !== 'open_router') return [];
  return [...new Set([
    connection.economy_model.trim(),
    connection.default_model.trim(),
    connection.reasoning_model.trim(),
  ].filter(Boolean))];
}

/** The create/edit form. Same fields for every preset, which is what makes a
 *  new compatible service need no new code. */
function ConnectionForm({
  t,
  form,
  setForm,
  onSubmit,
  onCancel,
  submitting,
  testResult,
  testing,
  onTest,
  onConnectionChange,
  modelCostSuffix,
  title,
  storedCredential = false,
  onRevealStored,
}: {
  t: ExternalApiSectionProps['t'];
  form: FormState;
  setForm: (updater: (prev: FormState) => FormState) => void;
  onSubmit: () => void;
  onCancel: () => void;
  submitting: boolean;
  testResult: ExternalApiConnectionTestResult | null;
  testing: boolean;
  onTest: () => void;
  onConnectionChange: (updater: (prev: FormState) => FormState) => void;
  modelCostSuffix?: (model: string) => string;
  title: string;
  storedCredential?: boolean;
  onRevealStored?: () => Promise<string | null>;
}) {
  const presets: { id: ExternalApiPreset; label: string }[] = [
    { id: 'lite_llm', label: 'LiteLLM' },
    { id: 'nvidia', label: 'NVIDIA' },
    { id: 'open_router', label: 'OpenRouter' },
    { id: 'other', label: t('config.extApi.presetOther') },
  ];
  // An endpoint is mandatory: the "Other" preset clears it, and a connection
  // with no endpoint is not executable. The backend enforces the same rule.
  const openRouterKeyValid =
    form.origin_preset !== 'open_router' ||
    !form.keyTouched ||
    form.api_key.trim().startsWith('sk-or-v1-');
  const canSubmit =
    form.display_name.trim().length > 0 &&
    form.mention_alias.trim().length > 0 &&
    form.endpoint.trim().length > 0 &&
    (!['nvidia', 'open_router'].includes(form.origin_preset) || storedCredential || form.api_key.trim().length > 0) &&
    (!storedCredential || !form.keyTouched || form.api_key.trim().length > 0) &&
    openRouterKeyValid &&
    !submitting;
  const modelsUnlocked = testResult?.ok === true && testResult.models.length > 0;

  return (
    <div className="set-ext-api-form" data-testid="ext-api-form">
      <div className="set-ext-api-form-head">
        <span className="set-ext-api-form-icon" aria-hidden="true"><PlugZap size={16} /></span>
        <div>
          <strong>{title}</strong>
          <small>{t('config.externalApiHelp')}</small>
        </div>
      </div>

      <div className="set-ext-api-presets" role="radiogroup" aria-label={t('config.extApi.preset')}>
        {presets.map(p => (
          <button
            key={p.id}
            type="button"
            role="radio"
            aria-checked={form.origin_preset === p.id}
            data-active={form.origin_preset === p.id}
            className="set-agent-choice set-ext-api-preset"
            data-testid={`ext-api-preset-${p.id}`}
            onClick={() =>
              onConnectionChange(prev => ({
                ...prev,
                origin_preset: p.id,
                display_name: p.id === 'open_router' && !prev.display_name.trim() ? 'OpenRouter' : prev.display_name,
                mention_alias: p.id === 'open_router' && !prev.mention_alias.trim() ? 'openrouter' : prev.mention_alias,
                // Pre-fill the endpoint from the preset. "Other" clears it so a
                // brand-new service starts from a blank, user-typed endpoint.
                endpoint: PRESET_ENDPOINTS[p.id],
              }))
            }
          >
            <span className="set-ext-api-preset-mark" aria-hidden="true" />
            {p.label}
          </button>
        ))}
      </div>

      <div className="set-ext-api-fields">
        <label className="set-litellm-field">
          <span className="set-litellm-label">{t('config.extApi.displayName')}</span>
          <input
            className="set-litellm-input"
            type="text"
            value={form.display_name}
            data-testid="ext-api-display-name"
            onChange={e => setForm(prev => ({ ...prev, display_name: e.target.value }))}
            aria-label={t('config.extApi.displayName')}
          />
        </label>

        <label className="set-litellm-field">
          <span className="set-litellm-label">{t('config.extApi.mentionAlias')}</span>
          <input
            className="set-litellm-input"
            type="text"
            placeholder="@groq"
            value={form.mention_alias}
            data-testid="ext-api-mention-alias"
            onChange={e => setForm(prev => ({ ...prev, mention_alias: e.target.value }))}
            aria-label={t('config.extApi.mentionAlias')}
          />
          <small className="set-hint">{t('config.extApi.aliasHint')}</small>
        </label>

        <label className="set-litellm-field">
          <span className="set-litellm-label">{t('liteLlm.endpointLabel')}</span>
          <input
            className="set-litellm-input"
            type="url"
            inputMode="url"
            placeholder="https://api.example.com/v1"
            value={form.endpoint}
            data-testid="ext-api-endpoint"
            onChange={e => onConnectionChange(prev => ({ ...prev, endpoint: e.target.value }))}
            aria-label={t('liteLlm.endpointLabel')}
          />
        </label>

        <div className="set-litellm-field">
          <span className="set-litellm-label">{t('liteLlm.keyLabel')}</span>
          <div className="set-ext-api-secret">
            <SecretField
              inputClassName="set-litellm-input set-ext-api-secret-input"
              buttonClassName="set-icon-btn set-ext-api-secret-eye"
              inputTestId="ext-api-key"
              ariaLabel={t('liteLlm.keyLabel')}
              placeholder={['nvidia', 'open_router'].includes(form.origin_preset)
                ? t('config.extApi.keyRequired')
                : t('liteLlm.keyOptional')}
              value={form.api_key}
              onChange={value => onConnectionChange(prev => ({ ...prev, api_key: value, keyTouched: true }))}
              stored={storedCredential}
              replacing={form.keyTouched}
              onReplace={() => onConnectionChange(prev => ({ ...prev, api_key: '', keyTouched: true }))}
              onCancelReplace={() => onConnectionChange(prev => ({ ...prev, api_key: '', keyTouched: false }))}
              onRevealStored={onRevealStored}
            />
          </div>
          {storedCredential && !form.keyTouched ? (
            <small className="set-ext-api-secret-hint" data-testid="ext-api-key-stored">
              <Check size={11} aria-hidden="true" /> {t('config.extApi.keyStoredHint')}
            </small>
          ) : null}
          {form.origin_preset === 'open_router' && form.keyTouched && !openRouterKeyValid ? (
            <small className="set-hint" data-testid="ext-api-openrouter-key-format">
              {t('config.extApi.openRouterKeyFormat')}
            </small>
          ) : null}
        </div>
      </div>

      <div className="set-ext-api-test-zone" data-models-unlocked={modelsUnlocked}>
        <div className="set-ext-api-test-actions">
          <button type="button" className="set-btn-secondary" disabled={!form.endpoint.trim() || testing} onClick={onTest} data-testid="ext-api-test">
            {testing ? <Loader2 size={12} className="spin" /> : <Check size={12} />} {t('config.extApi.testConnection')}
          </button>
          {testResult ? (
            <p className="set-hint" data-testid="ext-api-test-result" data-status={testResult.status}>
              {testResult.hint ?? (testResult.models.length > 0 ? t('config.extApi.modelsLoaded', testResult.models.length) : t('config.extApi.noModels'))}
            </p>
          ) : <p className="set-hint" data-testid="ext-api-test-required">{t('config.extApi.testRequired')}</p>}
        </div>

        <div className="set-ext-api-tier-panel">
          <div className="set-ext-api-tier-panel-title">
            <span>{t('disc.modelTier')}</span>
            {!modelsUnlocked ? <small>{t('config.extApi.modelsLocked')}</small> : null}
          </div>
          <div className="set-ext-api-tiers">
            {TIERS.map(tier => {
              // Explicit per-tier read/write keeps the field keys concrete (no
              // computed-union key), so the form type stays exact.
              const value =
                tier === 'economy'
                  ? form.economy_model
                  : tier === 'default'
                    ? form.default_model
                    : form.reasoning_model;
              const setValue = (next: string) =>
                setForm(prev =>
                  tier === 'economy'
                    ? { ...prev, economy_model: next }
                    : tier === 'default'
                      ? { ...prev, default_model: next }
                      : { ...prev, reasoning_model: next },
                );
              const models = [...new Set([
                value,
                ...(testResult?.models ?? []),
              ].filter(Boolean))];
              return (
                <div className="set-ext-api-tier" key={tier} data-tier={tier}>
                  <span className="set-ext-api-tier-label">
                    <span aria-hidden="true">{TIER_ICON[tier]}</span> {t(`disc.tier.${tier}`)}
                  </span>
                  <SearchableSelect
                    className="searchable-select--compact"
                    value={value}
                    options={models.map(model => ({
                      value: model,
                      label: model,
                      keywords: model.replaceAll('/', ' '),
                    }))}
                    onChange={setValue}
                    label={t(`disc.tier.${tier}`)}
                    placeholder={t('config.searchModel')}
                    emptyLabel={t('config.searchModelEmpty')}
                    clearLabel={t('config.defaultModel')}
                    disabled={!modelsUnlocked}
                    testId={`ext-api-tier-${tier}`}
                  />
                  {value && modelCostSuffix ? (
                    <span className="text-2xs text-muted">{modelCostSuffix(value)}</span>
                  ) : null}
                </div>
              );
            })}
          </div>
        </div>

        <div className="set-ext-api-tier-panel" data-testid="ext-api-media-panel">
          <div className="set-ext-api-tier-panel-title">
            <span>{t('config.extApi.mediaTitle')}</span>
            <small>{t('config.extApi.mediaOptional')}</small>
          </div>
          <div className="set-ext-api-tiers">
            {(['image', 'video'] as const).map(modality => {
              const value = modality === 'image' ? form.image_model : form.video_model;
              const setValue = (next: string) =>
                setForm(prev =>
                  modality === 'image'
                    ? { ...prev, image_model: next }
                    : { ...prev, video_model: next },
                );
              // Keep an already-saved value visible even when a provider no
              // longer returns it. This is the same explicit, non-destructive
              // behaviour as the text tiers; the user can see and replace it
              // after a successful connection test.
              const models = [...new Set([
                value,
                ...(testResult?.models ?? []),
              ].filter(Boolean))];
              return (
                <div className="set-ext-api-tier" key={modality} data-tier={modality}>
                  <span className="set-ext-api-tier-label">
                    <span aria-hidden="true">{modality === 'image' ? '🖼' : '🎬'}</span>{' '}
                    {t(`config.extApi.media.${modality}`)}
                  </span>
                  <SearchableSelect
                    className="searchable-select--compact"
                    value={value}
                    options={models.map(model => ({
                      value: model,
                      label: model,
                      keywords: model.replaceAll('/', ' '),
                    }))}
                    onChange={setValue}
                    label={t(`config.extApi.media.${modality}`)}
                    placeholder={t(`config.extApi.mediaPlaceholder.${modality}`)}
                    emptyLabel={t('config.searchModelEmpty')}
                    clearLabel={t('config.defaultModel')}
                    disabled={!modelsUnlocked}
                    testId={`ext-api-media-${modality}`}
                  />
                  {value && modelCostSuffix ? (
                    <span className="text-2xs text-muted">{modelCostSuffix(value)}</span>
                  ) : null}
                </div>
              );
            })}
          </div>
          <div className="set-hint-xs">{t('config.extApi.mediaHint')}</div>
        </div>
      </div>

      <div className="set-ext-api-form-actions">
        <button
          type="button"
          className="set-btn-primary"
          disabled={!canSubmit}
          data-testid="ext-api-save"
          onClick={onSubmit}
        >
          {submitting ? <Loader2 size={12} className="spin" /> : <Save size={12} />} {t('common.save')}
        </button>
        <button type="button" className="set-btn-secondary" onClick={onCancel} data-testid="ext-api-cancel">
          <X size={12} /> {t('common.cancel')}
        </button>
      </div>
    </div>
  );
}

export function ExternalApiSection({ t, toast, modelCostSuffix, onModelTiersChanged }: ExternalApiSectionProps) {
  const [connections, setConnections] = useState<ExternalApiConnectionView[] | null>(null);
  const [adding, setAdding] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [form, setForm] = useState<FormState>(emptyForm());
  const [submitting, setSubmitting] = useState(false);
  const [testResult, setTestResult] = useState<ExternalApiConnectionTestResult | null>(null);
  const [testing, setTesting] = useState(false);
  const [savedTests, setSavedTests] = useState<Record<string, ExternalApiConnectionTestResult | null>>({});
  const [testingSavedId, setTestingSavedId] = useState<string | null>(null);
  // State updates do not protect two synchronous clicks: retain the in-flight
  // request outside React so draft and saved probes share one bounded request.
  // Invalidating a form must release this lock immediately: the old request
  // may never settle, and its eventual completion must not clear a newer run.
  const activeTestRef = useRef<{ id: number; scope: 'draft' | 'saved' } | null>(null);
  const testRequestIdRef = useRef(0);
  const draftTestGenerationRef = useRef(0);
  const savedTestGenerationRef = useRef(0);

  const invalidateSavedTests = () => {
    savedTestGenerationRef.current += 1;
    if (activeTestRef.current?.scope === 'saved') activeTestRef.current = null;
    setSavedTests({});
    setTestingSavedId(null);
  };

  const invalidateDraftTest = () => {
    draftTestGenerationRef.current += 1;
    if (activeTestRef.current?.scope === 'draft') activeTestRef.current = null;
    setTesting(false);
    setTestResult(null);
  };

  const changeConnection = (updater: (prev: FormState) => FormState) => {
    invalidateDraftTest();
    // Keep the previous tier choices visible while the endpoint/key/preset is
    // unverified. The selectors are locked until the changed connection passes
    // a new test, so stale choices cannot be mistaken for a fresh catalogue.
    setForm(updater);
  };

  const testConnection = async () => {
    if (activeTestRef.current) return;
    const request = { id: ++testRequestIdRef.current, scope: 'draft' as const };
    activeTestRef.current = request;
    const generation = ++draftTestGenerationRef.current;
    setTesting(true);
    try {
      // Before the endpoint/key pair is validated, preserved tier choices are
      // display-only and must not turn the initial connectivity check into
      // model executions against a potentially different provider.
      const models = testResult?.ok ? modelsForProbe(form) : [];
      const result = await externalApi.test({
        endpoint: form.endpoint.trim() || null,
        api_key: form.keyTouched ? form.api_key : null,
        origin_preset: form.origin_preset,
        ...(models.length > 0 ? { models } : {}),
        ...(editingId ? { connection_id: editingId } : {}),
      });
      if (draftTestGenerationRef.current === generation) {
        setTestResult(result);
        // OpenRouter's catalogue is large. The explicit preset is primarily
        // here for the requested GLM 5.3 path, so select it for standard and
        // reasoning when present while leaving the economy tier optional.
        if (form.origin_preset === 'open_router' && result.models.includes('z-ai/glm-5.3')) {
          setForm(prev => ({
            ...prev,
            default_model: prev.default_model || 'z-ai/glm-5.3',
            reasoning_model: prev.reasoning_model || 'z-ai/glm-5.3',
          }));
        }
      }
    } catch (e) {
      if (draftTestGenerationRef.current === generation) {
        setTestResult({ ok: false, status: 'transport_error', models: [], hint: userError(e) });
      }
    } finally {
      if (activeTestRef.current?.id === request.id) {
        activeTestRef.current = null;
        if (draftTestGenerationRef.current === generation) setTesting(false);
      }
    }
  };

  const testSavedConnection = async (connection: ExternalApiConnectionView) => {
    if (activeTestRef.current) return;
    const request = { id: ++testRequestIdRef.current, scope: 'saved' as const };
    activeTestRef.current = request;
    const generation = ++savedTestGenerationRef.current;
    setTestingSavedId(connection.id);
    setSavedTests(prev => ({ ...prev, [connection.id]: null }));
    try {
      const models = modelsForProbe({
        origin_preset: connection.origin_preset,
        economy_model: connection.economy_model ?? '',
        default_model: connection.default_model ?? '',
        reasoning_model: connection.reasoning_model ?? '',
      });
      const result = await externalApi.test({
        endpoint: endpointForConnection(connection),
        api_key: null,
        connection_id: connection.id,
        origin_preset: connection.origin_preset,
        ...(models.length > 0 ? { models } : {}),
      });
      if (savedTestGenerationRef.current === generation) {
        setSavedTests(prev => ({ ...prev, [connection.id]: result }));
      }
    } catch (e) {
      if (savedTestGenerationRef.current === generation) {
        setSavedTests(prev => ({
          ...prev,
          [connection.id]: { ok: false, status: 'transport_error', models: [], hint: userError(e) },
        }));
      }
    } finally {
      if (activeTestRef.current?.id === request.id) {
        activeTestRef.current = null;
        if (savedTestGenerationRef.current === generation) setTestingSavedId(null);
      }
    }
  };

  const load = useCallback(async () => {
    invalidateSavedTests();
    try {
      setConnections(await externalApi.list());
    } catch (e) {
      toast(t('common.actionFailed', userError(e)), 'error');
      setConnections([]);
    }
  }, [t, toast]);

  useEffect(() => {
    void load();
  }, [load]);

  const startAdd = () => {
    invalidateDraftTest();
    setEditingId(null);
    setForm(emptyForm());
    // Seed the default preset's endpoint so the field is never blank on open.
    setForm(prev => ({ ...prev, endpoint: PRESET_ENDPOINTS[prev.origin_preset] }));
    setAdding(true);
  };

  const startEdit = (c: ExternalApiConnectionView) => {
    invalidateDraftTest();
    invalidateSavedTests();
    setAdding(false);
    setEditingId(c.id);
    setForm(formFromConnection(c));
  };

  const cancel = () => {
    invalidateDraftTest();
    setAdding(false);
    setEditingId(null);
  };

  const submitCreate = async () => {
    setSubmitting(true);
    try {
      await externalApi.create(toPayload(form));
      onModelTiersChanged?.();
      toast(t('config.saved'), 'success');
      cancel();
      await load();
    } catch (e) {
      toast(t('common.actionFailed', userError(e)), 'error');
    } finally {
      setSubmitting(false);
    }
  };

  const submitEdit = async (id: string) => {
    setSubmitting(true);
    try {
      await externalApi.update(id, toPayload(form));
      onModelTiersChanged?.();
      toast(t('config.saved'), 'success');
      cancel();
      await load();
    } catch (e) {
      toast(t('common.actionFailed', userError(e)), 'error');
    } finally {
      setSubmitting(false);
    }
  };

  const remove = async (c: ExternalApiConnectionView) => {
    if (!confirm(t('config.extApi.deleteConfirm', c.display_name))) return;
    try {
      invalidateSavedTests();
      await externalApi.remove(c.id);
      await load();
    } catch (e) {
      toast(t('common.actionFailed', userError(e)), 'error');
    }
  };

  const presetLabel = (preset: ExternalApiPreset): string =>
    preset === 'lite_llm'
      ? 'LiteLLM'
      : preset === 'nvidia'
        ? 'NVIDIA'
        : preset === 'open_router'
          ? 'OpenRouter'
          : t('config.extApi.presetOther');

  const renderTiers = (c: ExternalApiConnectionView): ReactNode => {
    const models: Record<Tier, string | null> = {
      economy: c.economy_model,
      default: c.default_model,
      reasoning: c.reasoning_model,
    };
    return (
      <div className="set-ext-api-conn-tiers" role="group" aria-label={t('disc.modelTier')}>
        {TIERS.map(tier => (
          <div key={tier} className="set-ext-api-conn-tier" data-tier={tier}>
            <span className="set-ext-api-conn-tier-label">
              <span aria-hidden="true">{TIER_ICON[tier]}</span>
              {t(`disc.tier.${tier}`)}
            </span>
            <code title={models[tier] ?? t('config.defaultModel')}>
              {models[tier] ?? t('config.defaultModel')}
            </code>
          </div>
        ))}
      </div>
    );
  };

  return (
    <div className="set-ext-api-section" data-testid="external-api-section">
      <div className="set-external-api-heading">
        <span className="set-external-api-heading-icon" aria-hidden="true"><PlugZap size={17} /></span>
        <span className="set-external-api-heading-copy">
          <strong>{t('config.externalApiTitle')}</strong>
          <small>{t('config.externalApiHelp')}</small>
        </span>
        <ContextHelp title={t('config.externalApiTitle')}>
          <p>{t('config.externalApiHelp')}</p>
        </ContextHelp>
      </div>

      {connections === null ? (
        <div className="set-ext-api-loading">
          <Loader2 size={14} className="spin" />
        </div>
      ) : (
        <div className="set-ext-api-list" data-testid="ext-api-connections">
          {connections.length === 0 && !adding && (
            <p className="set-hint set-ext-api-empty" data-testid="ext-api-empty">
              {t('config.extApi.noConnections')}
            </p>
          )}

          {connections.map(c =>
            editingId === c.id ? (
              <ConnectionForm
                key={c.id}
                t={t}
                form={form}
                setForm={updater => setForm(updater)}
                onSubmit={() => void submitEdit(c.id)}
                onCancel={cancel}
                submitting={submitting}
                testResult={testResult}
                testing={testing}
                onTest={() => void testConnection()}
                onConnectionChange={changeConnection}
                modelCostSuffix={modelCostSuffix}
                title={c.display_name}
                storedCredential={c.has_credential}
                onRevealStored={() => externalApi.reveal(c.id)}
              />
            ) : (
              <div
                key={c.id}
                className="set-ext-api-conn"
                data-testid="ext-api-connection"
                data-connection-id={c.id}
                data-preset={c.origin_preset}
                role="group"
                aria-label={c.display_name}
              >
                <div className="set-ext-api-conn-head">
                  <div className="set-ext-api-conn-identity">
                    <span className="set-ext-api-conn-icon" aria-hidden="true"><Server size={16} /></span>
                    <span className="set-ext-api-conn-heading">
                      <span className="set-ext-api-conn-title-row">
                        <span className="set-ext-api-conn-name">{c.display_name}</span>
                        <span className="set-ext-api-conn-preset">{presetLabel(c.origin_preset)}</span>
                      </span>
                      <span className="set-ext-api-conn-alias">@{c.mention_alias}</span>
                    </span>
                  </div>
                  <div className="set-ext-api-conn-actions">
                    <span
                      className="set-ext-api-conn-cred"
                      data-has-credential={c.has_credential}
                    >
                      {c.has_credential ? <Check size={11} /> : <Key size={11} />}
                      {t(c.has_credential ? 'config.extApi.keyConfigured' : 'config.extApi.keyOptionalStatus')}
                    </span>
                    <button
                      type="button"
                      className="set-icon-btn"
                      disabled={!endpointForConnection(c) || testingSavedId === c.id}
                      onClick={() => void testSavedConnection(c)}
                      aria-label={t('config.extApi.testConnection')}
                      data-testid={`ext-api-test-saved-${c.id}`}
                    >
                      {testingSavedId === c.id ? <Loader2 size={11} className="spin" /> : <PlugZap size={11} />}
                    </button>
                    <button
                      type="button"
                      className="set-icon-btn"
                      onClick={() => startEdit(c)}
                      aria-label={t('config.extApi.editConnection')}
                      data-testid={`ext-api-edit-${c.id}`}
                    >
                      <Pencil size={11} />
                    </button>
                    <button
                      type="button"
                      className="set-icon-btn text-ghost"
                      onClick={() => void remove(c)}
                      aria-label={t('common.delete')}
                      data-testid={`ext-api-delete-${c.id}`}
                    >
                      <Trash2 size={11} />
                    </button>
                  </div>
                </div>
                {endpointForConnection(c) && (
                  <div className="set-ext-api-conn-endpoint-row">
                    <Link2 size={13} aria-hidden="true" />
                    <span>
                      <small>{t('liteLlm.endpointLabel')}</small>
                      <code className="set-ext-api-conn-endpoint" title={endpointForConnection(c) ?? undefined}>
                        {endpointForConnection(c)}
                      </code>
                    </span>
                  </div>
                )}
                {savedTests[c.id] ? (
                  <p className="set-hint" data-testid={`ext-api-saved-test-result-${c.id}`} data-status={savedTests[c.id]?.status}>
                    {savedTests[c.id]?.hint ?? (
                      savedTests[c.id]?.models.length
                        ? t('config.extApi.modelsLoaded', savedTests[c.id]?.models.length ?? 0)
                        : t('config.extApi.noModels')
                    )}
                  </p>
                ) : null}
                {savedTests[c.id]?.ok && savedTests[c.id]?.models.length ? (
                  <div className="set-ext-api-conn-models" data-testid={`ext-api-saved-models-${c.id}`}>
                    {savedTests[c.id]?.models.map(model => <code key={model}>{model}</code>)}
                  </div>
                ) : null}
                {renderTiers(c)}
              </div>
            ),
          )}

          {adding ? (
            <ConnectionForm
              t={t}
              form={form}
              setForm={updater => setForm(updater)}
              onSubmit={() => void submitCreate()}
              onCancel={cancel}
              submitting={submitting}
              testResult={testResult}
              testing={testing}
              onTest={() => void testConnection()}
              onConnectionChange={changeConnection}
              modelCostSuffix={modelCostSuffix}
              title={t('config.extApi.addConnection')}
            />
          ) : (
            <button
              type="button"
              className="set-btn-secondary set-ext-api-add"
              onClick={startAdd}
              data-testid="ext-api-add-connection"
            >
              <span className="set-ext-api-add-icon" aria-hidden="true"><Plus size={15} /></span>
              <span>{t('config.extApi.addConnection')}</span>
            </button>
          )}
        </div>
      )}
    </div>
  );
}
