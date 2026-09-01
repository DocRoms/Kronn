import { useEffect, useMemo, useState } from 'react';
import { Plus, RefreshCw, Trash2, X } from 'lucide-react';
import { modelCatalogApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import type {
  AgentType,
  CatalogModelEntry,
  ModelCatalogSnapshot,
  ModelTier,
} from '../../types/generated';

interface ManualForm {
  runtimeTargetId: string;
  agentType: AgentType;
  modelId: string;
  displayName: string;
  capabilities: string[];
  reasoningModes: string;
  tier: ModelTier | '';
}

const blankForm = (snapshot: ModelCatalogSnapshot | null): ManualForm => {
  const target = snapshot?.targets[0];
  return {
    runtimeTargetId: target?.runtime_target_id ?? 'agent:claude-code',
    agentType: target?.agent_type ?? 'ClaudeCode',
    modelId: '',
    displayName: '',
    capabilities: ['chat'],
    reasoningModes: '',
    tier: '',
  };
};

export function ModelCatalogSection() {
  const { t } = useT();
  const [snapshot, setSnapshot] = useState<ModelCatalogSnapshot | null>(null);
  const [form, setForm] = useState<ManualForm | null>(null);
  const [editing, setEditing] = useState<CatalogModelEntry | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = async () => {
    const value = await modelCatalogApi.list();
    setSnapshot(value);
    return value;
  };
  useEffect(() => { void load().catch(err => setError(String(err))); }, []);

  const models = useMemo(
    () => snapshot?.targets.flatMap(target => target.models) ?? [],
    [snapshot],
  );

  const openCreate = () => {
    setEditing(null);
    setForm(blankForm(snapshot));
    setError(null);
  };
  const openEdit = (entry: CatalogModelEntry) => {
    setEditing(entry);
    setForm({
      runtimeTargetId: entry.runtime_target_id,
      agentType: entry.agent_type,
      modelId: entry.model_id,
      displayName: entry.display_alias ?? entry.display_name,
      capabilities: entry.capabilities,
      reasoningModes: entry.reasoning_modes.join(', '),
      tier: entry.tier_assignment ?? '',
    });
    setError(null);
  };
  const save = async () => {
    if (!form || !form.modelId.trim() || !form.displayName.trim()) return;
    setBusy(true);
    setError(null);
    const request = {
      runtime_target_id: form.runtimeTargetId,
      agent_type: form.agentType,
      model_id: form.modelId.trim(),
      display_name: form.displayName.trim(),
      capabilities: form.capabilities,
      reasoning_modes: form.reasoningModes.split(',').map(value => value.trim()).filter(Boolean),
      default_reasoning_mode: null,
      tier_assignment: form.tier || null,
    };
    try {
      if (editing) await modelCatalogApi.updateManual(request);
      else await modelCatalogApi.createManual(request);
      await load();
      setForm(null);
      setEditing(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };
  const remove = async (entry: CatalogModelEntry) => {
    setBusy(true);
    setError(null);
    try {
      await modelCatalogApi.deleteManual({
        runtime_target_id: entry.runtime_target_id,
        model_id: entry.model_id,
      });
      await load();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="set-section set-model-catalog" data-testid="model-catalog-section">
      <div className="set-section-header-lg">
        <div>
          <h3>{t('modelCatalog.title')}</h3>
          <p className="set-hint">{t('modelCatalog.description')}</p>
        </div>
        <button type="button" className="set-btn-secondary" onClick={openCreate}>
          <Plus size={13} /> {t('modelCatalog.add')}
        </button>
      </div>

      {error && <p className="set-hint" data-status="error">{error}</p>}

      {form && (
        <div className="set-ext-api-form set-model-catalog-form">
          <div className="set-ext-api-fields">
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.target')}</span>
              <select
                className="set-litellm-input"
                value={form.runtimeTargetId}
                disabled={Boolean(editing)}
                onChange={event => {
                  const target = snapshot?.targets.find(value => value.runtime_target_id === event.target.value);
                  if (target) setForm(current => current && ({
                    ...current,
                    runtimeTargetId: target.runtime_target_id,
                    agentType: target.agent_type,
                  }));
                }}
              >
                {snapshot?.targets.map(target => (
                  <option key={target.runtime_target_id} value={target.runtime_target_id}>
                    {target.target_label ?? target.runtime_target_id}
                  </option>
                ))}
              </select>
            </label>
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.modelId')}</span>
              <input className="set-litellm-input" value={form.modelId} disabled={Boolean(editing)} onChange={event => setForm(current => current && ({ ...current, modelId: event.target.value }))} />
            </label>
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.displayName')}</span>
              <input className="set-litellm-input" value={form.displayName} onChange={event => setForm(current => current && ({ ...current, displayName: event.target.value }))} />
            </label>
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.tier')}</span>
              <select className="set-litellm-input" value={form.tier} onChange={event => setForm(current => current && ({ ...current, tier: event.target.value as ModelTier | '' }))}>
                <option value="">{t('modelCatalog.noTier')}</option>
                <option value="economy">⚡ {t('disc.tier.economy')}</option>
                <option value="default">🎯 {t('disc.tier.default')}</option>
                <option value="reasoning">🧠 {t('disc.tier.reasoning')}</option>
              </select>
            </label>
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.reasoningModes')}</span>
              <input className="set-litellm-input" value={form.reasoningModes} placeholder="low, medium, high" onChange={event => setForm(current => current && ({ ...current, reasoningModes: event.target.value }))} />
            </label>
          </div>
          <div className="set-ext-api-test-actions">
            {(['chat', 'image', 'video'] as const).map(capability => (
              <label key={capability} className="set-model-catalog-capability">
                <input type="checkbox" checked={form.capabilities.includes(capability)} onChange={event => setForm(current => current && ({
                  ...current,
                  capabilities: event.target.checked
                    ? [...current.capabilities, capability]
                    : current.capabilities.filter(value => value !== capability),
                }))} />
                {capability}
              </label>
            ))}
            <button type="button" className="set-btn-primary" disabled={busy} onClick={() => void save()}>{t('common.save')}</button>
            <button type="button" className="set-icon-btn" onClick={() => setForm(null)} aria-label={t('common.cancel')}><X size={13} /></button>
          </div>
        </div>
      )}

      <div className="set-model-catalog-targets">
        {snapshot?.targets.map(target => (
          <article key={target.runtime_target_id} className="set-ext-api-conn-card">
            <div className="set-ext-api-conn-head">
              <div>
                <strong>{target.target_label ?? target.runtime_target_id}</strong>
                <small>{target.stale ? t('modelCatalog.stale') : t('modelCatalog.current')}</small>
              </div>
              {!target.runtime_target_id.startsWith('http:') && (
                <button type="button" className="set-icon-btn" disabled={busy} aria-label={t('modelCatalog.recheck')} onClick={async () => {
                  setBusy(true);
                  try {
                    await modelCatalogApi.refresh({ runtime_target_id: target.runtime_target_id, agent_type: target.agent_type, force: true });
                    await load();
                  } catch (err) { setError(String(err)); } finally { setBusy(false); }
                }}><RefreshCw size={12} /></button>
              )}
            </div>
            <div className="set-ext-api-conn-models">
              {target.models.length === 0 && <span className="set-hint">{t('modelCatalog.empty')}</span>}
              {target.models.map(model => (
                <div key={model.id} className="set-model-catalog-model" data-availability={model.availability}>
                  <button type="button" className="set-model-catalog-model-open" onClick={() => openEdit(model)}>
                    <span>{model.display_alias ?? model.display_name}</span>
                    <small>{model.model_id} · {t(`modelCatalog.provenance.${model.provenance}`)}</small>
                    {model.availability === 'unavailable' && <em>{t('modelCatalog.unavailable')}</em>}
                  </button>
                  {(model.provenance === 'manual' || model.provenance === 'migrated') && (
                    <button type="button" className="set-icon-btn" aria-label={t('common.delete')} onClick={() => void remove(model)}><Trash2 size={10} /></button>
                  )}
                </div>
              ))}
            </div>
          </article>
        ))}
      </div>
      {models.length > 0 && <p className="set-hint">{t('modelCatalog.count', models.length)}</p>}
    </section>
  );
}
