import { useModelCatalogSnapshot } from '../hooks/useModelCatalogSnapshot';
import { modelForAgentTier } from '../lib/constants';
import { useT } from '../lib/I18nContext';
import { catalogModelOptions, catalogTierEntry, modelRuntimeTargetId } from '../lib/modelCatalogSelection';
import type { AgentType, ModelTier, ModelTierConfig, ModelTiersConfig } from '../types/generated';
import { SearchableSelect } from './SearchableSelect';

interface Props {
  agent: AgentType;
  connectionId?: string | null;
  value: string;
  onChange: (model: string) => void;
  tier?: ModelTier;
  modelTiers?: ModelTiersConfig | null;
  targetModelTiers?: ModelTierConfig;
  reasoningEffort?: string;
  onReasoningChange?: (mode: string) => void;
  disabled?: boolean;
}

/** CLI refreshes do not select a model or probe an HTTP connection. */
export function ModelCatalogPicker({
  agent, connectionId, value, onChange, tier = 'default', modelTiers, targetModelTiers,
  reasoningEffort = '', onReasoningChange, disabled = false,
}: Props) {
  const { t } = useT();
  const runtime = modelRuntimeTargetId(agent, connectionId);
  const catalog = useModelCatalogSnapshot(true, [runtime]);
  const view = catalog.data?.targets.find(target => target.runtime_target_id === runtime);
  const target = view && (catalog.error || catalog.loading)
    ? { ...view, stale: true, live_refresh_ok: false } : view;
  const http = Boolean(connectionId) || ['Ollama', 'LiteLlm', 'Nvidia'].includes(agent);
  const configured = targetModelTiers?.[tier] || (http ? targetModelTiers?.default : null)
    || (connectionId ? '' : modelForAgentTier(agent, tier, modelTiers, ''));
  const effective = catalogTierEntry(target, tier, value || configured, http);
  const modelOptions = catalogModelOptions(target, value, t, () => '');
  const modes = [...new Set(effective?.reasoning_modes ?? [])].map(mode => ({
    value: mode, label: mode, disabled: effective?.availability === 'unavailable',
  }));
  if (reasoningEffort && !modes.some(mode => mode.value === reasoningEffort)) {
    modes.push({ value: reasoningEffort, label: `${reasoningEffort} — ${t('modelCatalog.notInCatalog')}`, disabled: true });
  }

  return (
    <div>
      <div className="flex-row gap-3">
        <div className="flex-1">
          <label className="wf-label">{t('wiz.model')}</label>
          <SearchableSelect
            value={value} options={modelOptions} onChange={onChange} disabled={disabled}
            label={t('wiz.model')} placeholder={effective?.model_id || configured || t('config.defaultModel')}
            emptyLabel={t('modelCatalog.empty')} clearLabel={t('config.defaultModel')}
            allowCustomValue customValueHint={t('modelCatalog.notInCatalog')}
          />
        </div>
        {onReasoningChange && <div className="flex-1">
          <label className="wf-label">{t('wiz.reasoningEffort')}</label>
          <SearchableSelect
            value={reasoningEffort} options={modes} onChange={onReasoningChange} disabled={disabled}
            label={t('wiz.reasoningEffort')} placeholder={effective?.default_reasoning_mode || t('config.defaultModel')}
            emptyLabel={t('modelCatalog.empty')} clearLabel={t('config.defaultModel')}
          />
        </div>}
      </div>
      {catalog.error && <p role="alert" className="text-xs text-ghost">
        {t('modelCatalog.loadError')}{' '}
        <button type="button" className="wf-icon-btn" disabled={catalog.loading} onClick={catalog.refetch}>{t('modelCatalog.reload')}</button>
      </p>}
    </div>
  );
}
