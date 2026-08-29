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
}

/** A preset only seeds the endpoint field. `endpoint: ''` means "the user must
 *  type it" — exactly the case a brand-new compatible service falls into. */
const PRESET_ENDPOINTS: Record<ExternalApiPreset, string> = {
  litellm: 'http://localhost:4000',
  nvidia: 'https://integrate.api.nvidia.com',
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
  api_key: string;
  keyTouched: boolean;
}

function emptyForm(): FormState {
  return {
    display_name: '',
    mention_alias: '',
    endpoint: '',
    origin_preset: 'litellm',
    economy_model: '',
    default_model: '',
    reasoning_model: '',
    api_key: '',
    keyTouched: false,
  };
}

function formFromConnection(c: ExternalApiConnectionView): FormState {
  return {
    display_name: c.display_name,
    mention_alias: c.mention_alias,
    endpoint: c.endpoint ?? '',
    origin_preset: c.origin_preset,
    economy_model: c.economy_model ?? '',
    default_model: c.default_model ?? '',
    reasoning_model: c.reasoning_model ?? '',
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
    // Only send the key when the user actually typed one, so editing a
    // connection without retyping its key keeps the stored credential.
    api_key: form.keyTouched ? form.api_key : null,
  };
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
}) {
  const presets: { id: ExternalApiPreset; label: string }[] = [
    { id: 'litellm', label: 'LiteLLM' },
    { id: 'nvidia', label: 'NVIDIA' },
    { id: 'other', label: t('config.extApi.presetOther') },
  ];
  // An endpoint is mandatory: the "Other" preset clears it, and a connection
  // with no endpoint is not executable. The backend enforces the same rule.
  const canSubmit =
    form.display_name.trim().length > 0 &&
    form.mention_alias.trim().length > 0 &&
    form.endpoint.trim().length > 0 &&
    !submitting;

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

        <label className="set-litellm-field">
          <span className="set-litellm-label">{t('liteLlm.keyLabel')}</span>
          <input
            className="set-litellm-input"
            type="password"
            autoComplete="off"
            placeholder={t('liteLlm.keyOptional')}
            value={form.api_key}
            data-testid="ext-api-key"
            onChange={e => onConnectionChange(prev => ({ ...prev, api_key: e.target.value, keyTouched: true }))}
            aria-label={t('liteLlm.keyLabel')}
          />
        </label>
      </div>

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
        <div className="set-ext-api-tier-panel-title">{t('disc.modelTier')}</div>
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
            return (
              <label className="set-ext-api-tier" key={tier} data-tier={tier}>
                <span className="set-ext-api-tier-label">
                  <span aria-hidden="true">{TIER_ICON[tier]}</span> {t(`disc.tier.${tier}`)}
                </span>
                <select
                  className="set-tier-input"
                  value={value}
                  data-testid={`ext-api-tier-${tier}`}
                  onChange={e => setValue(e.target.value)}
                  aria-label={t(`disc.tier.${tier}`)}
                  disabled={!testResult?.ok || testResult.models.length === 0}
                >
                  <option value="">{testResult?.ok ? t('config.defaultModel') : t('config.extApi.testRequired')}</option>
                  {testResult?.models.map(model => <option key={model} value={model}>{model}</option>)}
                </select>
                {value && modelCostSuffix ? (
                  <span className="text-2xs text-muted">{modelCostSuffix(value)}</span>
                ) : null}
              </label>
            );
          })}
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

export function ExternalApiSection({ t, toast, modelCostSuffix }: ExternalApiSectionProps) {
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
    // Models come from the exact endpoint/key/preset just tested. Never keep
    // a previous catalogue selection after any of those inputs changes.
    setForm(prev => {
      const next = updater(prev);
      return {
        ...next,
        economy_model: '',
        default_model: '',
        reasoning_model: '',
      };
    });
  };

  const testConnection = async () => {
    if (activeTestRef.current) return;
    const request = { id: ++testRequestIdRef.current, scope: 'draft' as const };
    activeTestRef.current = request;
    const generation = ++draftTestGenerationRef.current;
    setTesting(true);
    try {
      const result = await externalApi.test({
        endpoint: form.endpoint.trim() || null,
        api_key: form.keyTouched ? form.api_key : null,
        ...(editingId ? { connection_id: editingId, origin_preset: form.origin_preset } : {}),
      });
      if (draftTestGenerationRef.current === generation) setTestResult(result);
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
      const result = await externalApi.test({
        endpoint: connection.endpoint,
        api_key: null,
        connection_id: connection.id,
        origin_preset: connection.origin_preset,
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
    preset === 'litellm' ? 'LiteLLM' : preset === 'nvidia' ? 'NVIDIA' : t('config.extApi.presetOther');

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
                      disabled={!c.endpoint || testingSavedId === c.id}
                      onClick={() => void testSavedConnection(c)}
                      aria-label={t('config.extApi.testConnection')}
                      data-testid={`ext-api-test-saved-${c.id}`}
                    >
                      {testingSavedId === c.id ? <Loader2 size={11} className="spin" /> : <Check size={11} />}
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
                {c.endpoint && (
                  <div className="set-ext-api-conn-endpoint-row">
                    <Link2 size={13} aria-hidden="true" />
                    <span>
                      <small>{t('liteLlm.endpointLabel')}</small>
                      <code className="set-ext-api-conn-endpoint" title={c.endpoint}>{c.endpoint}</code>
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
