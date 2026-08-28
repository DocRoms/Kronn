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

import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { externalApi } from '../../lib/api';
import type {
  ExternalApiConnectionView,
  ExternalApiPreset,
  UpsertExternalApiConnection,
} from '../../lib/api';
import { userError } from '../../lib/userError';
import type { ToastFn } from '../../hooks/useToast';
import { ContextHelp } from '../ContextHelp';
import { Plus, Trash2, Save, X, Pencil, Loader2, Check, Key } from 'lucide-react';
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
  modelCostSuffix,
}: {
  t: ExternalApiSectionProps['t'];
  form: FormState;
  setForm: (updater: (prev: FormState) => FormState) => void;
  onSubmit: () => void;
  onCancel: () => void;
  submitting: boolean;
  modelCostSuffix?: (model: string) => string;
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
              setForm(prev => ({
                ...prev,
                origin_preset: p.id,
                // Pre-fill the endpoint from the preset. "Other" clears it so a
                // brand-new service starts from a blank, user-typed endpoint.
                endpoint: PRESET_ENDPOINTS[p.id],
              }))
            }
          >
            {p.label}
          </button>
        ))}
      </div>

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
          onChange={e => setForm(prev => ({ ...prev, endpoint: e.target.value }))}
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
          onChange={e => setForm(prev => ({ ...prev, api_key: e.target.value, keyTouched: true }))}
          aria-label={t('liteLlm.keyLabel')}
        />
      </label>

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
            <label className="set-ext-api-tier" key={tier}>
              <span className="set-ext-api-tier-label">
                <span aria-hidden="true">{TIER_ICON[tier]}</span> {t(`disc.tier.${tier}`)}
              </span>
              <input
                className="set-tier-input"
                type="text"
                placeholder={t('config.defaultModel')}
                value={value}
                data-testid={`ext-api-tier-${tier}`}
                onChange={e => setValue(e.target.value)}
                aria-label={t(`disc.tier.${tier}`)}
              />
              {value && modelCostSuffix ? (
                <span className="text-2xs text-muted">{modelCostSuffix(value)}</span>
              ) : null}
            </label>
          );
        })}
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

  const load = useCallback(async () => {
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
    setEditingId(null);
    setForm(emptyForm());
    // Seed the default preset's endpoint so the field is never blank on open.
    setForm(prev => ({ ...prev, endpoint: PRESET_ENDPOINTS[prev.origin_preset] }));
    setAdding(true);
  };

  const startEdit = (c: ExternalApiConnectionView) => {
    setAdding(false);
    setEditingId(c.id);
    setForm(formFromConnection(c));
  };

  const cancel = () => {
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
      <div className="set-ext-api-conn-tiers">
        {TIERS.map(tier => (
          <span key={tier} className="set-ext-api-conn-tier" title={t(`disc.tier.${tier}`)}>
            <span aria-hidden="true">{TIER_ICON[tier]}</span>{' '}
            <code>{models[tier] ?? t('config.defaultModel')}</code>
          </span>
        ))}
      </div>
    );
  };

  return (
    <div className="set-ext-api-section" data-testid="external-api-section">
      <div className="set-external-api-heading">
        <span>{t('config.externalApiTitle')}</span>
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
            <p className="set-hint" data-testid="ext-api-empty">
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
                modelCostSuffix={modelCostSuffix}
              />
            ) : (
              <div
                key={c.id}
                className="set-ext-api-conn"
                data-testid="ext-api-connection"
                data-connection-id={c.id}
                data-preset={c.origin_preset}
              >
                <div className="set-ext-api-conn-head">
                  <span className="set-ext-api-conn-name">{c.display_name}</span>
                  <span className="set-ext-api-conn-alias">@{c.mention_alias}</span>
                  <span className="set-ext-api-conn-preset">{presetLabel(c.origin_preset)}</span>
                  <span
                    className="set-ext-api-conn-cred"
                    data-has-credential={c.has_credential}
                    title={c.has_credential ? t('common.copied') : t('liteLlm.keyOptional')}
                  >
                    {c.has_credential ? <Check size={11} /> : <Key size={11} />}
                  </span>
                  <div className="set-ext-api-conn-actions">
                    <button
                      type="button"
                      className="set-icon-btn"
                      onClick={() => startEdit(c)}
                      aria-label={t('common.save')}
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
                {c.endpoint && <code className="set-ext-api-conn-endpoint">{c.endpoint}</code>}
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
              modelCostSuffix={modelCostSuffix}
            />
          ) : (
            <button
              type="button"
              className="set-btn-secondary set-ext-api-add"
              onClick={startAdd}
              data-testid="ext-api-add-connection"
            >
              <Plus size={12} /> {t('config.extApi.addConnection')}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
