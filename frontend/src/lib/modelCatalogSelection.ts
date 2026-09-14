import type { SearchableSelectOption } from '../components/SearchableSelect';
import type { AgentType, CatalogModelEntry, ModelCatalogSnapshot, ModelCatalogView, ModelTier, ModelTierConfig, ModelTiersConfig } from '../types/generated';
import { modelForAgentTier } from './constants';

export interface CatalogAgentTarget {
  agent: AgentType;
  connectionId?: string | null;
  modelTiers?: ModelTierConfig;
}

/** Resolve configuration and catalogue in one exact runtime namespace. */
export function resolveCatalogTier(
  snapshot: ModelCatalogSnapshot | null,
  target: CatalogAgentTarget,
  tier: ModelTier,
  familyTiers?: ModelTiersConfig | null,
  explicitModel?: string | null,
) {
  const runtimeId = modelRuntimeTargetId(target.agent, target.connectionId);
  const view = snapshot?.targets.find(candidate => candidate.runtime_target_id === runtimeId);
  const http = Boolean(target.connectionId) || ['Ollama', 'LiteLlm', 'Nvidia'].includes(target.agent);
  const configured = explicitModel?.trim() || target.modelTiers?.[tier]
    || (http ? target.modelTiers?.default : null)
    || (target.connectionId ? '' : modelForAgentTier(target.agent, tier, familyTiers, ''));
  const entry = catalogTierEntry(view, tier, configured, http);
  return {
    configured, entry, view,
    model: entry?.display_alias ?? entry?.display_name ?? entry?.model_id ?? configured,
    unavailable: entry?.availability === 'unavailable',
    provenance: entry ? catalogModelProvenance(entry, view) : null,
  };
}

export type ResolvedCatalogTier = ReturnType<typeof resolveCatalogTier>;

/** Search the displayed target and its already-resolved identities; never refresh or select. */
export function matchesCatalogSearch(query: string, terms: Array<string | null | undefined>): boolean {
  const normalize = (value: string) => value.normalize('NFKD').replace(/\p{Diacritic}/gu, '').toLowerCase();
  return normalize(terms.filter(Boolean).join(' ')).includes(normalize(query.trim()));
}

export function catalogTargetSearchTerms(target: CatalogAgentTarget, tiers: ResolvedCatalogTier[]): Array<string | null | undefined> {
  return [target.agent, target.connectionId, modelRuntimeTargetId(target.agent, target.connectionId),
    ...tiers.flatMap(({ configured, entry }) => [configured, entry?.model_id, entry?.display_name, entry?.display_alias])];
}

const AGENT_RUNTIME_TARGETS: Record<AgentType, string> = {
  ClaudeCode: 'agent:claude-code', Codex: 'agent:codex', OpenCode: 'agent:opencode',
  Vibe: 'agent:vibe', GeminiCli: 'agent:gemini-cli', Kiro: 'agent:kiro',
  CopilotCli: 'agent:copilot-cli', Ollama: 'agent:ollama', LiteLlm: 'agent:litellm',
  Nvidia: 'agent:nvidia', Custom: 'agent:custom',
};

export function modelRuntimeTargetId(agent: AgentType, connectionId?: string | null): string {
  return connectionId ? `http:${connectionId}` : AGENT_RUNTIME_TARGETS[agent];
}

export function catalogTierEntry(
  target: ModelCatalogView | undefined,
  tier: ModelTier,
  configured: string,
  useDefaultTier = false,
): CatalogModelEntry | undefined {
  const models = target?.models.filter(model => model.runtime_target_id === target.runtime_target_id);
  // An unknown explicit identity is still explicit: never substitute another model.
  if (configured) return models?.find(model => model.model_id === configured);
  return models?.find(model => model.tier_assignment === tier)
    ?? (useDefaultTier ? models?.find(model => model.tier_assignment === 'default') : undefined);
}

export function catalogModelProvenance(model: CatalogModelEntry, target: ModelCatalogView | undefined) {
  return model.provenance === 'live' && (target?.stale || !target?.live_refresh_ok)
    ? 'cached' : model.provenance;
}

export function catalogModelOptions(
  target: ModelCatalogView | undefined,
  configured: string,
  t: (key: string, ...args: (string | number)[]) => string,
  observedCost: (model: string) => string,
): SearchableSelectOption[] {
  const options = (target?.models ?? [])
    .filter(model => model.runtime_target_id === target?.runtime_target_id)
    .map(model => {
      const unavailable = model.availability === 'unavailable';
      const provenance = catalogModelProvenance(model, target);
      const cliDefault = target?.runtime_target_id === 'agent:claude-code' && model.model_id === 'default';
      return {
        value: model.model_id,
        label: `${model.display_alias ?? model.display_name}${cliDefault ? ` — ${t('modelCatalog.cliDefault')}` : ''}${unavailable ? ` — ${t('modelCatalog.unavailable')}` : ''}`,
        keywords: `${model.model_id} ${model.display_name} ${model.reasoning_modes.join(' ')}`,
        description: [
          model.model_id,
          t(`modelCatalog.provenance.${provenance}`),
          target?.runtime_target_id === 'agent:claude-code' && ['live', 'cached'].includes(provenance)
            ? t('modelCatalog.accessUnverified') : '',
          t('modelCatalog.lastChecked', model.last_checked_at),
          model.reasoning_modes.length ? `${t('modelCatalog.reasoningModes')}: ${model.reasoning_modes.join(', ')}` : '',
          unavailable ? model.unavailable_detail || model.unavailable_reason : '',
          observedCost(model.model_id) || t(`modelCatalog.costHint.${model.cost_hint ?? 'unknown'}`),
          model.privacy_note,
        ].filter(Boolean).join(' · '),
        disabled: unavailable,
      };
    });
  if (configured && !options.some(option => option.value === configured)) {
    options.push({
      value: configured,
      label: `${configured} — ${t('modelCatalog.notInCatalog')}`,
      keywords: configured,
      description: t('modelCatalog.keepConfigured'),
      disabled: true,
    });
  }
  return options;
}
