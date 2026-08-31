import React, { useState, useEffect, useRef, type CSSProperties, type ReactNode } from 'react';
import { config as configApi, agents as agentsApi, usage as usageApi, nvidia as nvidiaApi } from '../../lib/api';
import { userError } from '../../lib/userError';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import { OllamaCard } from './OllamaCard';
import { ExternalApiSection } from './ExternalApiSection';
import { CompressionSection } from './CompressionSection';
import { ContextHelp } from '../ContextHelp';
import { SearchableSelect } from '../SearchableSelect';
import { useApi } from '../../hooks/useApi';
import type { AgentConfig, AgentDetection, AgentsConfig, AgentType, ModelTiersConfig, UsageReport } from '../../types/generated';

/** Where each agent's config lives, and how many runs it allows by default.
 *  `null` = unlimited: a remote endpoint someone else scales is not this
 *  machine's problem, so it is only capped when the operator asks. Ollama
 *  serves ONE inference slot — a second run does not compute sooner, it queues
 *  and throws away the KV cache the first one warmed. A CLI is a process. */
/** Only the keys of `AgentsConfig` that hold an `AgentConfig` — `model_tiers`
 *  lives in the same object but is a different shape. */
type AgentConfigKey = {
  [K in keyof AgentsConfig]: AgentsConfig[K] extends AgentConfig ? K : never
}[keyof AgentsConfig];

const AGENT_CONCURRENCY: Partial<Record<AgentType, { key: AgentConfigKey; fallback: number | null }>> = {
  ClaudeCode: { key: 'claude_code', fallback: 5 },
  Codex: { key: 'codex', fallback: 5 },
  GeminiCli: { key: 'gemini_cli', fallback: 5 },
  Kiro: { key: 'kiro', fallback: 5 },
  Vibe: { key: 'vibe', fallback: 5 },
  CopilotCli: { key: 'copilot_cli', fallback: 5 },
  Ollama: { key: 'ollama', fallback: 1 },
  LiteLlm: { key: 'lite_llm', fallback: null },
  Nvidia: { key: 'nvidia', fallback: null },
};
import type { ToastFn } from '../../hooks/useToast';
import { isUpdateAvailable } from '../../lib/version';
import {
  AGENT_LABELS,
  agentColor,
  agentMentionColors,
  type AgentMentionColors,
} from '../../lib/constants';
import {
  Key, AlertTriangle, Save,
  Plus, Trash2, Download, Check,
  Loader2, RefreshCw, X, Eye, EyeOff, Play, StopCircle,
  ExternalLink, FolderSearch, ArrowUpCircle, Copy, Gauge, FileText, Palette, GitFork, Info, Layers,
} from 'lucide-react';
import {
  ALL_USAGE_FILTER,
  CCUSAGE_GITHUB_URL,
  analyzeUsageReport,
  observedModelCostPerMillion,
  usageModels,
} from './usageFilters';
import '../../pages/SettingsPage.css';

function formatObservedCost(usdPerMillion: number): string {
  if (usdPerMillion === 0) return '$0.00';
  if (usdPerMillion < 0.01) return '< $0.01';
  if (usdPerMillion >= 100) return `$${usdPerMillion.toFixed(0)}`;
  if (usdPerMillion >= 10) return `$${usdPerMillion.toFixed(1)}`;
  return `$${usdPerMillion.toFixed(2)}`;
}

function formatCostMultiplier(multiplier: number): string {
  if (multiplier === 0) return '0';
  if (multiplier < 0.1) return multiplier.toFixed(2);
  if (multiplier < 10) return Number(multiplier.toFixed(1)).toString();
  return Math.round(multiplier).toString();
}

interface AgentsSectionProps {
  agents: AgentDetection[];
  agentAccess: AgentsConfig | null;
  configLanguage: string | null;
  refetchAgents: () => void;
  refetchAgentAccess: () => void;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
  usagePanel?: ReactNode;
  /** Backend runs in the Docker container → an install would land in the
   *  container, not on the host. We disable the Install button and point to
   *  the host-side `kronn` CLI instead. Default false (native/Tauri). */
  inDocker?: boolean;
}

export function AgentsSection({
  agents,
  agentAccess,
  refetchAgents,
  refetchAgentAccess,
  toast,
  t,
  usagePanel,
  inDocker = false,
}: AgentsSectionProps) {
  const [installing, setInstalling] = useState<string | null>(null);
  const [newKeyInputs, setNewKeyInputs] = useState<Record<string, { name: string; value: string }>>({});
  const [addingKeyFor, setAddingKeyFor] = useState<string | null>(null);
  const [tokenVisible, setTokenVisible] = useState<Set<string>>(new Set());
  const [tierEditing, setTierEditing] = useState<Record<string, { economy: string; default: string; reasoning: string }>>({});
  // KT-337 — NVIDIA's catalogue is fetched, never hardcoded: ~100 ids across 25
  // vendors, and several are listed but not callable by this account. Loaded on
  // demand (one click) rather than on mount, so opening Settings costs nothing.
  const [nvidiaCatalogue, setNvidiaCatalogue] = useState<string[] | null>(null);
  const [nvidiaLoading, setNvidiaLoading] = useState(false);
  // Verdict per model id, from a real probe. The catalogue cannot answer this.
  const [nvidiaProbes, setNvidiaProbes] = useState<Record<string, { verdict: string; detail: string }>>({});
  // Draft text for the NVIDIA tier inputs, keyed `<agent>:<field>`. The field was
  // uncontrolled (`defaultValue`), so it stopped reflecting a saved value after the
  // first mount and a pick in the datalist only committed once focus left — which
  // read as "I can no longer change the model". Controlled + committed on an exact
  // catalogue match, on Enter, or on blur.
  const [nvidiaDraft, setNvidiaDraft] = useState<Record<string, string>>({});
  // Connection state for the header line, so NVIDIA reads like LiteLLM and Ollama
  // ("online — N models") instead of showing an install state that means nothing
  // for a remote endpoint. `hasKey` is separate on purpose: the catalogue answers
  // WITHOUT a key, so "online" alone would describe an unusable setup as healthy.
  const [nvidiaStatus, setNvidiaStatus] = useState<{ online: boolean; count: number; hasKey: boolean } | null>(null);
  // `disabled={nvidiaLoading}` is closure-stale between two synchronous clicks, so
  // it cannot be the only gate on an async handler (docs/AGENTS.md). Both NVIDIA
  // buttons go through the ref-backed guard; the loading flag stays for the label.
  const loadNvidiaCatalogue = useAsyncGuard(async () => {
    setNvidiaLoading(true);
    try {
      const res = await nvidiaApi.models();
      if (res) {
        setNvidiaCatalogue(res.models.map(m => m.id));
        setNvidiaStatus({ online: true, count: res.models.length, hasKey: res.has_key });
        if (!res.has_key) toast(t('config.nvidiaNoKey'), 'info');
      }
    } catch (e) {
      toast(String(e), 'error');
    } finally {
      setNvidiaLoading(false);
    }
  });
  const probeNvidiaModel = useAsyncGuard(async (model: string) => {
    try {
      const res = await nvidiaApi.probe(model);
      if (res) {
        setNvidiaProbes(prev => ({ ...prev, [model]: { verdict: String(res.verdict), detail: res.detail } }));
        toast(res.detail, res.verdict === 'Usable' ? 'success' : 'info');
      }
    } catch (e) {
      toast(String(e), 'error');
    }
  });
  const [mentionColorOverrides, setMentionColorOverrides] = useState<AgentMentionColors>({});
  const [usageReport, setUsageReport] = useState<UsageReport | null>(null);
  const [costDisplayMode, setCostDisplayMode] = useState<'absolute' | 'relative'>('absolute');
  const [costReferenceModel, setCostReferenceModel] = useState('');
  // When set to an agent name, the per-agent update modal is shown. The
  // modal is small + global to the section (rather than per-row state) so
  // we never re-render rows on its open/close — keeps the agent grid
  // scroll position stable.
  const [updateModalFor, setUpdateModalFor] = useState<AgentDetection | null>(null);

  const { data: tokenConfig, refetch: refetchTokens } = useApi(() => configApi.getTokens(), []);

  // The model tier is a creation-time default for new discussions / QPs /
  // workflow Agent steps. The summary strategy seeds new discussions too,
  // but its Off value also acts as the backend's global auto-summary kill
  // switch for existing discussions. Loaded once on mount, written back on
  // every change via setServerConfig PATCH.
  const [defaultTier, setDefaultTier] = useState<'economy' | 'default' | 'reasoning' | null>(null);
  const [defaultSummaryStrategy, setDefaultSummaryStrategy] = useState<'Auto' | 'OnDemand' | 'Off' | null>(null);
  const [agentHandoffsEnabled, setAgentHandoffsEnabled] = useState<boolean | null>(null);
  const [agentHandoffPaidLimit, setAgentHandoffPaidLimit] = useState(1);
  const [agentHandoffPaidUnlimited, setAgentHandoffPaidUnlimited] = useState(false);
  const [agentHandoffBlockedAgents, setAgentHandoffBlockedAgents] = useState<AgentType[]>([]);
  useEffect(() => {
    configApi.getServerConfig().then(cfg => {
      if (cfg) {
        setDefaultTier(cfg.default_model_tier ?? 'default');
        setDefaultSummaryStrategy(cfg.default_summary_strategy ?? 'Off');
        setAgentHandoffsEnabled(cfg.agent_handoffs_enabled ?? false);
        setAgentHandoffPaidLimit(cfg.agent_handoff_paid_limit ?? 1);
        setAgentHandoffPaidUnlimited(cfg.agent_handoff_paid_unlimited ?? false);
        setAgentHandoffBlockedAgents(cfg.agent_handoff_blocked_agents ?? []);
      }
    }).catch(() => {
      setDefaultTier('default');
      setDefaultSummaryStrategy('Off');
      setAgentHandoffsEnabled(false);
    });
  }, []);

  useEffect(() => {
    let active = true;
    usageApi.get('monthly')
      .then(report => {
        if (active && report.rows.length > 0) setUsageReport(report);
      })
      .catch(() => {
        // Model prices are optional context; the dedicated ccUsage card owns
        // the actionable unavailable/error state.
      });
    return () => { active = false; };
  }, []);

  const observedCostModels = usageReport
    ? usageModels(usageReport, ALL_USAGE_FILTER).flatMap(model => {
        const cost = observedModelCostPerMillion(usageReport, model);
        return cost === null ? [] : [{ model, cost }];
      })
    : [];
  const relativeReferenceModels = observedCostModels.filter(model => model.cost > 0);
  const defaultReferenceModel = usageReport
    ? analyzeUsageReport(usageReport)?.mostUsed[0]?.modelName ?? ''
    : '';
  const effectiveReferenceModel = costReferenceModel
    || (relativeReferenceModels.some(model => model.model === defaultReferenceModel)
      ? defaultReferenceModel
      : relativeReferenceModels[0]?.model ?? '');
  const referenceCost = usageReport && effectiveReferenceModel
    ? observedModelCostPerMillion(usageReport, effectiveReferenceModel)
    : null;

  const modelCostSuffix = (model: string): string => {
    if (!usageReport) return '';
    const observedCost = observedModelCostPerMillion(usageReport, model);
    if (observedCost === null) return '';
    if (costDisplayMode === 'relative') {
      return referenceCost !== null && referenceCost > 0
        ? ` · ≈ ×${formatCostMultiplier(observedCost / referenceCost)}`
        : '';
    }
    return ` · ≈ ${t('config.modelCostObserved', formatObservedCost(observedCost))}`;
  };

  const mentionColors: AgentMentionColors = {
    ...agentMentionColors(agentAccess),
    ...mentionColorOverrides,
  };

  const saveMentionColor = async (agent: AgentType, color: string) => {
    const previous = mentionColorOverrides[agent];
    setMentionColorOverrides(current => ({ ...current, [agent]: color }));
    try {
      await configApi.setAgentMentionColor({ agent, color });
      refetchAgentAccess();
      window.dispatchEvent(new Event('kronn:agent-mention-colors-changed'));
      toast(t('config.saved'), 'success');
    } catch {
      setMentionColorOverrides(current => {
        const next = { ...current };
        if (previous) next[agent] = previous;
        else delete next[agent];
        return next;
      });
      toast(t('config.saveError'), 'error');
    }
  };

  const saveDefaultTier = async (tier: 'economy' | 'default' | 'reasoning') => {
    // Optimistic update so the dropdown feels snappy ; revert on error.
    const previous = defaultTier;
    setDefaultTier(tier);
    try {
      await configApi.setServerConfig({ default_model_tier: tier });
      toast(t('config.saved'), 'success');
    } catch {
      setDefaultTier(previous);
      toast(t('config.saveError'), 'error');
    }
  };

  const saveDefaultSummary = async (strategy: 'Auto' | 'OnDemand' | 'Off') => {
    const previous = defaultSummaryStrategy;
    setDefaultSummaryStrategy(strategy);
    try {
      await configApi.setServerConfig({ default_summary_strategy: strategy });
      toast(t('config.saved'), 'success');
    } catch {
      setDefaultSummaryStrategy(previous);
      toast(t('config.saveError'), 'error');
    }
  };

  const saveAgentHandoffsEnabled = async (enabled: boolean) => {
    const previous = agentHandoffsEnabled;
    setAgentHandoffsEnabled(enabled);
    try {
      await configApi.setServerConfig({ agent_handoffs_enabled: enabled });
      toast(t('config.saved'), 'success');
    } catch {
      setAgentHandoffsEnabled(previous);
      toast(t('config.saveError'), 'error');
    }
  };

  const saveAgentHandoffPaidLimit = async (value: number | 'unlimited') => {
    const previousLimit = agentHandoffPaidLimit;
    const previousUnlimited = agentHandoffPaidUnlimited;
    const unlimited = value === 'unlimited';
    if (!unlimited) setAgentHandoffPaidLimit(value);
    setAgentHandoffPaidUnlimited(unlimited);
    try {
      await configApi.setServerConfig({
        agent_handoff_paid_unlimited: unlimited,
        ...(!unlimited ? { agent_handoff_paid_limit: value } : {}),
      });
      toast(t('config.saved'), 'success');
    } catch {
      setAgentHandoffPaidLimit(previousLimit);
      setAgentHandoffPaidUnlimited(previousUnlimited);
      toast(t('config.saveError'), 'error');
    }
  };

  const saveAgentHandoffTarget = async (agent: AgentType, allowed: boolean) => {
    const previous = agentHandoffBlockedAgents;
    const next = allowed
      ? previous.filter(blocked => blocked !== agent)
      : previous.includes(agent) ? previous : [...previous, agent];
    setAgentHandoffBlockedAgents(next);
    try {
      await configApi.setServerConfig({ agent_handoff_blocked_agents: next });
      toast(t('config.saved'), 'success');
    } catch {
      setAgentHandoffBlockedAgents(previous);
      toast(t('config.saveError'), 'error');
    }
  };

  // Load model tiers once. Pre-fix the loop only seeded 5 of the 7 agents
  // (copilot_cli + ollama were missing), so when the user opened the
  // tier dropdowns for those two agents the inputs showed empty even
  // when the backend already had saved values — and a subsequent save
  // wiped the side that wasn't being edited (saveTiers reads from the
  // editing map and writes both economy + reasoning back to the API).
  useEffect(() => {
    let cancelled = false;
    nvidiaApi.models()
      .then(res => {
        if (cancelled || !res) return;
        const ids = res.models.map(m => m.id);
        setNvidiaCatalogue(ids);
        setNvidiaStatus({ online: true, count: ids.length, hasKey: res.has_key });
      })
      .catch(() => { if (!cancelled) setNvidiaStatus({ online: false, count: 0, hasKey: false }); });
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    configApi.getModelTiers().then(tiers => {
      if (tiers) {
        const editing: Record<string, { economy: string; default: string; reasoning: string }> = {};
        for (const key of ['claude_code', 'codex', 'gemini_cli', 'kiro', 'vibe', 'copilot_cli', 'ollama', 'lite_llm', 'nvidia'] as const) {
          editing[key] = { economy: tiers[key]?.economy ?? '', default: tiers[key]?.default ?? '', reasoning: tiers[key]?.reasoning ?? '' };
        }
        setTierEditing(editing);
      }
    }).catch(() => {});
  }, []);

  // System model-error messages leave a short-lived deep-link target before
  // navigating here. The dedicated LiteLLM/Ollama cards load asynchronously,
  // so retry briefly, then focus + animate the exact agent/tier picker rather
  // than dropping the user at the top of a long settings section.
  useEffect(() => {
    if (Object.keys(tierEditing).length === 0) return;
    let target: { agentType?: string; tier?: string } | null = null;
    try {
      target = JSON.parse(sessionStorage.getItem('kronn:model-config-target') ?? 'null');
    } catch { /* malformed/stale browser state: ignore */ }
    if (!target?.agentType || !['economy', 'default', 'reasoning'].includes(target.tier ?? '')) return;

    let attempts = 0;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const focusTarget = () => {
      const select = Array.from(document.querySelectorAll<HTMLElement>('[data-model-tier-agent]'))
        .find(element => element.dataset.modelTierAgent === target?.agentType
          && element.dataset.modelTier === target?.tier);
      if (!select && attempts++ < 30) {
        timer = setTimeout(focusTarget, 200);
        return;
      }
      if (!select) return;
      try { sessionStorage.removeItem('kronn:model-config-target'); } catch { /* ignore */ }
      select.scrollIntoView?.({ behavior: 'smooth', block: 'center' });
      select.focus({ preventScroll: true });
      select.classList.add('set-model-tier-focus');
      timer = setTimeout(() => select.classList.remove('set-model-tier-focus'), 2200);
    };
    focusTarget();
    return () => { if (timer) clearTimeout(timer); };
  }, [tierEditing]);

  // Synchronous re-entry guard — `setInstalling(...)` is async-rendered,
  // so two fast clicks on the same install button (or two different ones)
  // would both pass the closure-stale `disabled={installing !== null}`
  // check and fire two parallel `agentsApi.install()` calls. The ref blocks
  // the second invocation before the network round-trip starts.
  const installingRef = useRef(false);
  const handleInstallAgent = async (agent: AgentDetection) => {
    if (installingRef.current) return;
    installingRef.current = true;
    setInstalling(agent.name);
    try {
      await agentsApi.install(agent.agent_type);
      refetchAgents();
    } catch (e) {
      // Surface the failure so the user knows the install didn't go
      // through (network glitch, missing npm/uvx, permission issues,
      // …). Pre-fix this swallowed the error and the button just
      // reverted with no feedback. Mirrors the uninstall path's
      // toast pattern just below.
      toast(t('config.installFailed', agent.name, userError(e)), 'error');
    } finally {
      installingRef.current = false;
      setInstalling(null);
    }
  };

  /** Compact, header-mounted like the mention colour: every card already has a
   *  slot for it, including Ollama's and LiteLLM's dedicated ones. */
  const renderConcurrencyControl = (agentType: AgentType) => {
    const slot = AGENT_CONCURRENCY[agentType];
    if (!slot) return null;
    const current = agentAccess?.[slot.key]?.concurrency ?? null;
    return (
      <label
        className="set-agent-concurrency-control"
        title={t('agents.concurrencyHint')}
      >
        <Layers size={12} aria-hidden="true" />
        <span>{t('agents.concurrency')}</span>
        <input
          type="number"
          min={1}
          max={20}
          // Empty = the family default, which for a remote provider IS
          // unlimited. Never 0: "never run" is what disabling the agent is for.
          placeholder={slot.fallback === null ? '\u221e' : String(slot.fallback)}
          value={current ?? ''}
          aria-label={t('agents.concurrency')}
          data-testid={`agent-concurrency-${agentType}`}
          onChange={async (event) => {
            const raw = event.target.value.trim();
            const parsed = Number(raw);
            if (raw !== '' && !Number.isFinite(parsed)) return;
            const next = raw === '' ? null : Math.min(20, Math.max(1, parsed));
            try {
              await configApi.setAgentConcurrency({ agent: agentType, concurrency: next });
            } catch (err) {
              console.warn('Settings action failed:', err);
              toast(t('common.actionFailed', userError(err)), 'error');
            }
            refetchAgentAccess();
          }}
        />
      </label>
    );
  };

  const renderMentionColorControl = (agentType: AgentType) => {
    const color = agentColor(agentType, mentionColors);
    return (
      <label
        className="set-agent-mention-control"
        style={{ '--mention-color': color } as CSSProperties}
        title={t('config.mentionColorFor', AGENT_LABELS[agentType])}
      >
        <Palette size={12} aria-hidden="true" />
        <span>@{AGENT_LABELS[agentType]}</span>
        <input
          type="color"
          value={color}
          onChange={event => saveMentionColor(agentType, event.target.value)}
          aria-label={t('config.mentionColorFor', AGENT_LABELS[agentType])}
          data-testid={`mention-color-${agentType}`}
        />
      </label>
    );
  };

  // KT-339 — LiteLLM and NVIDIA are no longer two standalone fleet cards: they
  // are connections in the unified External API zone rendered below. Filter
  // them out of the CLI/agent fleet list so they stop appearing separately.
  const EXTERNAL_API_AGENTS: AgentType[] = ['LiteLlm', 'Nvidia'];
  const fleetAgents = agents.filter(agent => !EXTERNAL_API_AGENTS.includes(agent.agent_type));

  const activeAgentCount = fleetAgents.filter(agent => agent.enabled && (agent.installed || agent.runtime_available)).length;

  return (
    <div className="set-agents-section">
        {(() => {
          const isWSL = agents.some(a => a.host_label === 'WSL');
          const hasDockerAgent = agents.some(a => a.installed && !a.host_managed);
          return isWSL && hasDockerAgent ? (
            <div className="set-wsl-warning">
              <AlertTriangle size={12} className="text-warning flex-shrink-0" style={{ marginTop: 2 }} />
              <span className="text-sm text-tertiary" style={{ lineHeight: 1.4 }}>{t('config.wslWarning')}</span>
            </div>
          ) : null;
        })()}

        <section className="set-agent-defaults" data-testid="agent-defaults">
          <div className="set-agent-defaults-head">
            <div>
              <div className="font-semibold text-base">{t('config.agentDefaultsTitle')}</div>
              <p className="set-hint">{t('config.agentDefaultsHint')}</p>
            </div>
          </div>
          <div className="set-agent-defaults-grid">
            <div className="set-agent-default-card" data-testid="default-tier-section">
              <div className="set-agent-default-title">
                <Gauge size={15} aria-hidden="true" />
                <span>{t('config.defaultTierLabel')}</span>
              </div>
              <p>{t('config.defaultTierHint')}</p>
              <div className="set-agent-choice-list" role="radiogroup" aria-label={t('config.defaultTierLabel')}>
                {(['economy', 'default', 'reasoning'] as const).map(tier => {
                  const labels = {
                    economy: t('disc.tier.economy'),
                    default: t('disc.tier.default'),
                    reasoning: t('disc.tier.reasoning'),
                  };
                  const icons = {
                    economy: '⚡',
                    default: '🎯',
                    reasoning: '🧠',
                  };
                  const active = defaultTier === tier;
                  return (
                    <button
                      key={tier}
                      type="button"
                      role="radio"
                      aria-checked={active}
                      className="set-agent-choice"
                      data-active={active}
                      data-testid={`default-tier-btn-${tier}`}
                      onClick={() => saveDefaultTier(tier)}
                      disabled={defaultTier === null}
                    >
                      <span className="set-agent-choice-mark" aria-hidden="true" />
                      <span>
                        <strong>
                          <span className="set-agent-choice-icon" aria-hidden="true">{icons[tier]}</span>
                          {labels[tier]}
                        </strong>
                        <small>{t(`config.defaultTier.${tier}Hint`)}</small>
                      </span>
                    </button>
                  );
                })}
              </div>
            </div>

            <div className="set-agent-default-card" data-testid="default-summary-section">
              <div className="set-agent-default-title">
                <FileText size={15} aria-hidden="true" />
                <span>{t('config.defaultSummaryLabel')}</span>
                <ContextHelp title={t('config.defaultSummaryInfoTitle')} align="end">
                  <p>{t('config.defaultSummaryInfoHistory')}</p>
                  <p>{t('config.defaultSummaryInfoMcp')}</p>
                </ContextHelp>
              </div>
              <p>{t('config.defaultSummaryHint')}</p>
              <div className="set-agent-choice-list" role="radiogroup" aria-label={t('config.defaultSummaryLabel')}>
                {(['Off', 'Auto', 'OnDemand'] as const).map(strategy => {
                  const labels = {
                    Off: t('config.summaryOff'),
                    Auto: t('config.summaryAuto'),
                    OnDemand: t('config.summaryOnDemand'),
                  };
                  const active = defaultSummaryStrategy === strategy;
                  return (
                    <button
                      key={strategy}
                      type="button"
                      role="radio"
                      aria-checked={active}
                      className="set-agent-choice"
                      data-active={active}
                      data-testid={`default-summary-btn-${strategy.toLowerCase()}`}
                      onClick={() => saveDefaultSummary(strategy)}
                      disabled={defaultSummaryStrategy === null || strategy === 'OnDemand'}
                      data-coming-soon={strategy === 'OnDemand'}
                    >
                      <span className="set-agent-choice-mark" aria-hidden="true" />
                      <span>
                        <strong>
                          {labels[strategy]}
                          {strategy === 'OnDemand' && (
                            <span className="set-agent-choice-soon">{t('config.comingSoon')}</span>
                          )}
                        </strong>
                        <small>{t(`config.defaultSummary.${strategy.toLowerCase()}Hint`)}</small>
                      </span>
                    </button>
                  );
                })}
              </div>
            </div>

            <div
              id="settings-agent-handoffs"
              className="set-agent-default-card set-agent-handoff-card"
              data-testid="agent-handoff-section"
            >
              <div className="set-agent-default-title">
                <GitFork size={15} aria-hidden="true" />
                <span>{t('config.agentHandoffTitle')}</span>
                <button
                  type="button"
                  className="set-agent-handoff-toggle"
                  data-enabled={agentHandoffsEnabled === true}
                  aria-pressed={agentHandoffsEnabled === true}
                  disabled={agentHandoffsEnabled === null}
                  onClick={() => void saveAgentHandoffsEnabled(!agentHandoffsEnabled)}
                >
                  <span>{agentHandoffsEnabled ? t('config.agentHandoffOn') : t('config.agentHandoffOff')}</span>
                  <span className="set-toggle-track" data-on={agentHandoffsEnabled === true}>
                    <span className="set-toggle-thumb" data-on={agentHandoffsEnabled === true} />
                  </span>
                </button>
              </div>
              {agentHandoffsEnabled === true && (
                <div className="set-agent-handoff-expanded" data-testid="agent-handoff-details">
                  <p>{t('config.agentHandoffHint')}</p>
                  <label className="set-agent-handoff-limit">
                    <span>{t('config.agentHandoffPaidLimit')}</span>
                    <select
                      value={agentHandoffPaidUnlimited ? 'unlimited' : String(agentHandoffPaidLimit)}
                      onChange={event => void saveAgentHandoffPaidLimit(
                        event.target.value === 'unlimited' ? 'unlimited' : Number(event.target.value),
                      )}
                    >
                      {[0, 1, 2, 3, 4, 5].map(limit => (
                        <option key={limit} value={limit}>
                          {limit === 0
                            ? t('config.agentHandoffPaidLimitZero')
                            : limit === 1
                              ? t('config.agentHandoffPaidLimitOne')
                              : t('config.agentHandoffPaidLimitMany', limit)}
                        </option>
                      ))}
                      <option value="unlimited">{t('config.agentHandoffPaidLimitUnlimited')}</option>
                    </select>
                    <small>{t('config.agentHandoffPaidLimitHint')}</small>
                  </label>
                  {agentHandoffPaidUnlimited && (
                    <div className="set-agent-handoff-warning" role="alert">
                      <AlertTriangle size={16} aria-hidden="true" />
                      <span>
                        <strong>{t('config.agentHandoffUnlimitedWarningTitle')}</strong>
                        <small>{t('config.agentHandoffUnlimitedWarning')}</small>
                      </span>
                    </div>
                  )}
                  <div className="set-agent-handoff-targets">
                    <div className="set-agent-handoff-targets-copy">
                      <strong>{t('config.agentHandoffTargetsTitle')}</strong>
                      <small>{t('config.agentHandoffTargetsHint')}</small>
                    </div>
                    <div className="set-agent-handoff-target-grid">
                      {agents.map(agent => {
                        const allowed = !agentHandoffBlockedAgents.includes(agent.agent_type);
                        const local = agent.agent_type === 'Ollama';
                        return (
                          <button
                            key={agent.agent_type}
                            type="button"
                            className="set-agent-handoff-target"
                            data-allowed={allowed}
                            aria-pressed={allowed}
                            onClick={() => void saveAgentHandoffTarget(agent.agent_type, !allowed)}
                          >
                            <span className="set-agent-handoff-target-check" aria-hidden="true">
                              {allowed ? <Check size={11} /> : <X size={11} />}
                            </span>
                            <span>
                              <strong>{AGENT_LABELS[agent.agent_type] ?? agent.name}</strong>
                              <small>{t(local ? 'config.agentHandoffTargetLocal' : 'config.agentHandoffTargetPaid')}</small>
                            </span>
                            <span className="set-agent-handoff-target-state">
                              {t(allowed
                                ? 'config.agentHandoffTargetAllowed'
                                : 'config.agentHandoffTargetBlocked')}
                            </span>
                          </button>
                        );
                      })}
                    </div>
                  </div>
                  <div className="set-agent-handoff-cli-note" role="note">
                    <Info size={15} aria-hidden="true" />
                    <span>
                      <strong>{t('config.agentHandoffCliTitle')}</strong>
                      <small>{t('config.agentHandoffCliHint')}</small>
                    </span>
                  </div>
                </div>
              )}
            </div>
          </div>
        </section>

        <div className="set-agent-economy-grid">
          <CompressionSection agents={agents} onActivated={refetchAgents} toast={toast} t={t} />
          {usagePanel}
        </div>

        <div className="set-agent-list-head">
          <div>
            <div className="set-agent-list-title">
              <span className="font-semibold text-base">{t('config.agentFleetTitle')}</span>
              <span className="set-agent-active-count">{t('config.agentActiveCount', activeAgentCount, fleetAgents.length)}</span>
            </div>
            <p className="set-hint">{t('config.agentFleetHint')}</p>
          </div>
          <div className="set-agent-list-actions">
            <button
              className="set-discover-btn"
              title={t('config.discoverKeys')}
              onClick={async () => {
                try {
                  const res = await configApi.discoverKeys();
                  if (res.imported_count > 0) {
                    toast(t('config.discoverKeysFound').replace('{0}', String(res.discovered.length)).replace('{1}', String(res.imported_count)), 'success');
                    refetchTokens();
                  } else if (res.discovered.length > 0) {
                    toast(t('config.discoverKeysFound').replace('{0}', String(res.discovered.length)).replace('{1}', '0'), 'info');
                  } else {
                    toast(t('config.discoverKeysNone'), 'info');
                  }
                } catch { toast(t('config.discoverKeysNone'), 'error'); }
              }}
            >
              <FolderSearch size={10} /> {t('config.discoverKeys')}
            </button>
            <button className="set-icon-btn" onClick={() => refetchAgents()} title={t('config.refresh')} aria-label={t('config.refresh')}>
              <RefreshCw size={12} />
            </button>
          </div>
        </div>

        {observedCostModels.length > 0 && (
          <div className="set-agent-cost-display" data-testid="model-cost-display">
            <div className="set-agent-cost-display-copy">
              <strong>
                {t('config.modelCostDisplayTitle')}{' '}
                <a
                  href={CCUSAGE_GITHUB_URL}
                  target="_blank"
                  rel="noreferrer"
                  className="set-compression-link"
                >
                  ccusage <ExternalLink size={10} />
                </a>
              </strong>
              <span>{t('config.modelCostDisplayHint')}</span>
            </div>
            <div className="set-agent-cost-display-controls">
              <div
                className="set-agent-cost-mode"
                role="radiogroup"
                aria-label={t('config.modelCostDisplayTitle')}
              >
                <button
                  type="button"
                  role="radio"
                  aria-checked={costDisplayMode === 'absolute'}
                  data-active={costDisplayMode === 'absolute'}
                  data-testid="model-cost-mode-absolute"
                  onClick={() => setCostDisplayMode('absolute')}
                >
                  {t('config.modelCostModeAbsolute')}
                </button>
                <button
                  type="button"
                  role="radio"
                  aria-checked={costDisplayMode === 'relative'}
                  data-active={costDisplayMode === 'relative'}
                  data-testid="model-cost-mode-relative"
                  disabled={relativeReferenceModels.length === 0}
                  onClick={() => setCostDisplayMode('relative')}
                >
                  {t('config.modelCostModeRelative')}
                </button>
              </div>
              {costDisplayMode === 'relative' && effectiveReferenceModel && (
                <label className="set-agent-cost-reference">
                  <span>{t('config.modelCostReference')}</span>
                  <select
                    value={effectiveReferenceModel}
                    onChange={event => setCostReferenceModel(event.target.value)}
                    data-testid="model-cost-reference"
                  >
                    {relativeReferenceModels.map(model => (
                      <option key={model.model} value={model.model}>
                        {model.model} · ≈ {formatObservedCost(model.cost)}/M
                      </option>
                    ))}
                  </select>
                </label>
              )}
            </div>
          </div>
        )}

        {inDocker && (
          <div className="set-agent-runtime-warning" role="note">
            ⚠️ {t('config.dockerInstallNote')}
          </div>
        )}

        {fleetAgents.map(agent => {
          // KT-339 — LiteLLM, NVIDIA and any other OpenAI-compatible service are
          // now named connections in the unified External API zone below, so the
          // fleet loop never renders them as their own cards.

          // Ollama gets its own dedicated card with health check + model picker
          if (agent.agent_type === 'Ollama') {
            return (
              <div
                key="ollama"
                className="set-agent-row set-agent-row-ollama"
                data-agent-type="Ollama"
                style={{ '--agent-color': agentColor('Ollama', mentionColors) } as CSSProperties}
              >
                <OllamaCard
                  t={t}
                  modelCostSuffix={modelCostSuffix}
                  headerAccessory={<>{renderMentionColorControl('Ollama')}{renderConcurrencyControl('Ollama')}</>}
                />
              </div>
            );
          }
          const permFlag: Record<string, { flag: string; descKey: string }> = {
            ClaudeCode: { flag: '--dangerously-skip-permissions', descKey: 'config.fullAccess' },
            Codex: { flag: '--sandbox=danger-full-access', descKey: 'config.fullAccess' },
            GeminiCli: { flag: '--yolo', descKey: 'config.fullAccess' },
            CopilotCli: { flag: '--allow-all-tools', descKey: 'config.fullAccess' },
          };
          const perm = permFlag[agent.agent_type];
          const tokenField: Record<string, { key: string; hint: string; url: string }> = {
            ClaudeCode: { key: 'anthropic', hint: 'ANTHROPIC_API_KEY', url: 'https://console.anthropic.com/settings/keys' },
            Codex: { key: 'openai', hint: 'OPENAI_API_KEY', url: 'https://platform.openai.com/api-keys' },
            GeminiCli: { key: 'google', hint: 'GEMINI_API_KEY', url: 'https://aistudio.google.com/apikey' },
            Vibe: { key: 'mistral', hint: 'MISTRAL_API_KEY', url: 'https://console.mistral.ai/api-keys' },
            CopilotCli: { key: 'github', hint: 'GH_TOKEN', url: 'https://github.com/settings/tokens' },
            // KT-337 — NVIDIA has no CLI to log into: the key IS the whole setup,
            // so the generic token field is all it needs to become usable.
            Nvidia: { key: 'nvidia', hint: 'NVIDIA_API_KEY', url: 'https://build.nvidia.com/settings/api-keys' },
          };
          const tf = tokenField[agent.agent_type];
          const authReady = agent.auth_ready !== false;
          const isFullAccess = agent.agent_type === 'ClaudeCode'
            ? agentAccess?.claude_code?.full_access ?? false
            : agent.agent_type === 'Codex'
              ? agentAccess?.codex?.full_access ?? false
              : agent.agent_type === 'GeminiCli'
                ? agentAccess?.gemini_cli?.full_access ?? false
                : agent.agent_type === 'Vibe'
                  ? agentAccess?.vibe?.full_access ?? false
                  : agent.agent_type === 'CopilotCli'
                    ? agentAccess?.copilot_cli?.full_access ?? false
                    : false;

          return (
          <React.Fragment key={agent.name}>
          <div
            className="set-agent-row"
            data-agent-type={agent.agent_type}
            style={{ '--agent-color': agentColor(agent.agent_type, mentionColors) } as CSSProperties}
          >
            <div className="set-agent-card-header">
              <div className="set-agent-identity">
                <div
                  className="set-dot"
                  data-state={!agent.enabled
                    ? 'disabled'
                    : !authReady
                      ? 'auth-required'
                      : agent.installed || agent.runtime_available
                        ? 'ready'
                        : 'missing'}
                  aria-hidden="true"
                />
                <span className="set-sr-only">
                  {!authReady && agent.enabled
                    ? t('config.agentAuthRequired')
                    : agent.enabled && (agent.installed || agent.runtime_available)
                      ? t('config.enabled')
                    : t('config.disabled')}
                </span>
                <div className="set-agent-heading">
                  <div className="set-agent-title-row">
                  <span className="set-agent-title">{agent.name}</span>
                  <span className="set-origin-badge">{agent.origin}</span>
                  {agent.version && <code className="set-code text-xs">v{agent.version}</code>}
                  {/* Lenient semver compare (mirror of backend `versions.rs`).
                   *  Pre-fix this used `!==` which fired on `v2.0.51` vs
                   *  `2.0.51` etc. Now clicking the pill opens an upgrade
                   *  modal with a copyable command (= the agent's
                   *  install_command, since npm/curl install scripts are
                   *  idempotent re-runs). */}
                  {agent.installed && agent.version && agent.latest_version
                    && isUpdateAvailable(agent.version, agent.latest_version) && (
                    <button
                      type="button"
                      className="set-update-badge set-update-badge-cta"
                      onClick={() => setUpdateModalFor(agent)}
                      aria-label={t('config.agentUpdateAvailableAria', agent.name, agent.latest_version)}
                      title={t('config.agentUpdateAvailableTitle', agent.version, agent.latest_version)}
                    >
                      <ArrowUpCircle size={10} /> {agent.latest_version}
                    </button>
                  )}
                  {/* Provider account / usage dashboard — handy for "am I
                   *  about to hit my rate limit / quota?" without leaving
                   *  Kronn for the docs. User-reported 2026-05-10. Each
                   *  link opens in a new tab; we only surface it when we
                   *  have a meaningful URL for the agent. Kiro's "account"
                   *  is AWS Builder ID — same console as the install
                   *  flow. */}
                  {(() => {
                    const accountUrl = ({
                      ClaudeCode:  'https://console.anthropic.com/settings/usage',
                      Codex:       'https://platform.openai.com/usage',
                      GeminiCli:   'https://aistudio.google.com/app/usage',
                      // Vibe runs on Mistral — the workspace admin
                      // console is where you see consumption + plan
                      // limits (the user-reported correct link, the
                      // earlier `console.mistral.ai/usage` 404s).
                      Vibe:        'https://admin.mistral.ai/organization/workspaces',
                      Kiro:        'https://kiro.dev/account',
                      CopilotCli:  'https://github.com/settings/copilot',
                      Ollama:      '',
                    } as Record<string, string>)[agent.agent_type];
                    if (!accountUrl) return null;
                    return (
                      <a
                        href={accountUrl}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="set-agent-account-link"
                        title={t('config.viewAccount', agent.name)}
                        aria-label={t('config.viewAccount', agent.name)}
                      >
                        ↗ {t('config.account')}
                      </a>
                    );
                  })()}
                  {(agent.installed || agent.runtime_available) && (() => {
                    // Agents RTK doesn't support or can't hook. Mirrors
                    // `rtk_flag_for` in backend/src/api/rtk.rs and
                    // `RTK_APPLICABLE` in CompressionSection.tsx.
                    //   - Vibe: API-only (no shell to hook), "planned" in RTK docs.
                    //   - Kiro: not in RTK's supported agents list.
                    //   - CopilotCli: RTK's `--copilot` flag targets VS Code
                    //     Copilot Chat, not the `@github/copilot` CLI.
                    // Ollama has its own card and never hits this branch.
                    const notApplicable = agent.agent_type === 'Vibe'
                      || agent.agent_type === 'Kiro'
                      || agent.agent_type === 'CopilotCli';
                    if (notApplicable) {
                      return (
                        <span
                          className="set-agent-rtk-badge"
                          data-state="not-applicable"
                          title={t('config.rtk.badgeNotApplicable')}
                        >
                          {t('config.rtk.badgeNotApplicable')}
                        </span>
                      );
                    }
                    const state = !agent.rtk_available ? 'not-installed'
                      : agent.rtk_hook_configured ? 'active'
                      : 'missing';
                    const label = state === 'active' ? t('config.rtk.badgeActive')
                      : state === 'missing' ? t('config.rtk.badgeMissing')
                      : t('config.rtk.badgeNotInstalled');
                    return (
                      <a
                        href="https://github.com/rtk-ai/rtk"
                        target="_blank"
                        rel="noreferrer"
                        className="set-agent-rtk-badge"
                        data-state={state}
                        title={t('config.rtk.aboutLink')}
                      >
                        {label}
                      </a>
                    );
                  })()}
                  </div>
                  <div className="set-agent-runtime-state">
                    {agent.agent_type === 'Nvidia' && (
                      <div className="text-xs mt-2">
                        {nvidiaStatus === null
                          ? <span className="text-ghost">…</span>
                          : !nvidiaStatus.online
                            ? <span style={{ color: 'rgba(var(--kr-danger-rgb), 0.7)' }}>{t('config.nvidiaOffline')}</span>
                            : !nvidiaStatus.hasKey
                              ? <span style={{ color: 'rgba(var(--kr-warning-amber-rgb), 0.8)' }}>{t('config.nvidiaKeyMissing')}</span>
                              : <span style={{ color: 'rgba(var(--kr-success-rgb), 0.6)' }}>
                                  {t('liteLlm.online')} — {nvidiaStatus.count} {t('config.nvidiaCatalogueModels')}
                                </span>}
                      </div>
                    )}
                    {!agent.installed && !agent.runtime_available && (
                      <div className="text-xs text-faint mt-2">
                        <code className="set-code">{agent.install_command}</code>
                      </div>
                    )}
                    {!agent.installed && agent.runtime_available && (
                      <div className="text-xs mt-2" style={{ color: 'rgba(var(--kr-success-rgb), 0.5)' }}>
                        runtime OK <span className="text-ghost">— via npx</span>
                      </div>
                    )}
                    {agent.runtime_warning && (
                      <div className="set-agent-runtime-warning" role="note">
                        ⚠️ {t(`agentRuntimeWarning.${agent.runtime_warning}`)}
                      </div>
                    )}
                    {!authReady && agent.enabled && (
                      <div className="set-agent-auth-required" role="note">
                        <AlertTriangle size={13} aria-hidden="true" />
                        <span>
                          <strong>{t('config.agentAuthRequired')}</strong>
                          {' — '}
                          {t('config.agentAuthRequiredHint')}
                        </span>
                        {agent.auth_setup_command && (
                          <button
                            type="button"
                            className="set-agent-auth-command"
                            onClick={() => {
                              navigator.clipboard.writeText(agent.auth_setup_command ?? '').catch(() => {});
                              toast(t('common.copied'), 'success');
                            }}
                          >
                            <Copy size={11} aria-hidden="true" />
                            <code>{agent.auth_setup_command}</code>
                          </button>
                        )}
                      </div>
                    )}
                  </div>
                </div>
                </div>
              {/* Only a binary actually installed in the container gets the
                  enable/uninstall controls. An agent reachable only via the
                  npx runtime is NOT considered installed — it keeps the
                  "runtime OK — via npx" hint above and is offered for a real
                  install, so the user isn't told an agent they never installed
                  is "Activé". */}
              <div className="set-agent-actions">
                {renderMentionColorControl(agent.agent_type)}
                {renderConcurrencyControl(agent.agent_type)}
                {agent.installed ? (
                  <>
                  <button
                    className="set-enable-btn"
                    data-on={agent.enabled}
                    title={agent.enabled ? t('config.toggleDisable') : t('config.toggleEnable')}
                    onClick={async () => {
                      try {
                        await agentsApi.toggle(agent.agent_type);
                      } catch (e) { toast(String(e) || t('config.saveError'), 'error'); }
                      refetchAgents();
                    }}
                    disabled={installing !== null}
                  >
                    {agent.enabled ? t('config.enabled') : t('config.disabled')}
                  </button>
                  {agent.host_managed && (
                    <span className="text-2xs text-faint" style={{ marginLeft: 2 }} title={t('config.hostManaged')}>{agent.host_label ?? 'host'}</span>
                  )}
                  <button
                    className="set-icon-btn text-ghost"
                    title={t('config.uninstall')}
                    aria-label={t('config.uninstall')}
                    onClick={async () => {
                      if (!confirm(t('config.uninstallConfirm', agent.name))) return;
                      setInstalling(agent.name);
                      try {
                        await agentsApi.uninstall(agent.agent_type);
                        const updated = await agentsApi.detect();
                        const still = updated?.find((a: AgentDetection) => a.agent_type === agent.agent_type);
                        if (still?.installed && still?.enabled) {
                          toast(t('config.uninstallFailed'), 'error');
                        }
                      } catch {
                        toast(t('config.uninstallFailed'), 'error');
                      } finally {
                        refetchAgents();
                        setInstalling(null);
                      }
                    }}
                    disabled={installing !== null}
                  >
                    <Trash2 size={11} />
                  </button>
                  </>
                ) : (
                  <button
                    className="set-install-btn set-agent-install-btn"
                    onClick={() => handleInstallAgent(agent)}
                    disabled={installing !== null || inDocker}
                    title={inDocker ? t('config.dockerInstallTooltip', agent.install_command ?? '') : undefined}
                  >
                    {installing === agent.name ? (
                      <><Loader2 size={10} className="set-spin" /> ...</>
                    ) : (
                      <><Download size={10} /> Installer</>
                    )}
                  </button>
                )}
              </div>
            </div>
            <div className="set-agent-card-body">
            {perm && (agent.installed || agent.runtime_available) && (
              <div className="set-agent-panel set-agent-panel-access">
                <div className="set-agent-section-title">
                  <span>{t('config.fullAccessBadge')}</span>
                  <code>{perm.flag}</code>
                </div>
                <div
                  role="switch"
                  aria-checked={isFullAccess}
                  tabIndex={0}
                  className="set-agent-access-switch"
                  onClick={async () => {
                    try { await configApi.setAgentAccess({ agent: agent.agent_type, full_access: !isFullAccess }); } catch (err) { console.warn('Settings action failed:', err); toast(t('common.actionFailed', userError(err)), 'error'); }
                    refetchAgentAccess();
                  }}
                  onKeyDown={async (e) => {
                    if (e.key === ' ' || e.key === 'Enter') {
                      e.preventDefault();
                      try { await configApi.setAgentAccess({ agent: agent.agent_type, full_access: !isFullAccess }); } catch (err) { console.warn('Settings action failed:', err); toast(t('common.actionFailed', userError(err)), 'error'); }
                      refetchAgentAccess();
                    }
                  }}
                >
                  <div className="set-toggle-track" data-on={isFullAccess}>
                    <div className="set-toggle-thumb" data-on={isFullAccess} style={{ left: isFullAccess ? 16 : 1 }} />
                  </div>
                  <span className={isFullAccess ? 'text-accent' : 'text-muted'}>
                    {isFullAccess ? t('config.enabled') : t('config.disabled')}
                  </span>
                </div>
                <p>{t(perm.descKey)}</p>
              </div>
            )}
            {tf && (agent.installed || agent.runtime_available) && (() => {
              const providerKeys = tokenConfig?.keys?.filter(k => k.provider === tf.key) ?? [];
              const isDisabled = tokenConfig?.disabled_overrides?.includes(tf.key);
              const isAdding = addingKeyFor === tf.key;
              const newInput = newKeyInputs[tf.key] ?? { name: '', value: '' };
              return (
              <div className="set-agent-panel set-agent-panel-auth">
                <div className="set-agent-section-title">
                  <span>{t('config.apiKeys')}</span>
                  <a
                    href={tf.url}
                    target="_blank"
                    rel="noopener noreferrer"
                    title={t('config.getKey')}
                  >
                    <ExternalLink size={10} />
                  </a>
                </div>
                {/* Provider-level override toggle */}
                {providerKeys.length > 0 && (
                  <div className="flex-row gap-3 mb-2">
                    <Key size={10} className="text-ghost flex-shrink-0" />
                    <button
                      className="set-icon-btn set-icon-btn-bare"
                      title={isDisabled ? t('config.enableOverride') : t('config.disableOverride')}
                      aria-label={isDisabled ? t('config.enableOverride') : t('config.disableOverride')}
                      onClick={async () => {
                        try { await configApi.toggleTokenOverride(tf.key); } catch (err) { console.warn('Settings action failed:', err); toast(t('common.actionFailed', userError(err)), 'error'); }
                        refetchTokens();
                      }}
                    >
                      {isDisabled
                        ? <Play size={10} className="text-ghost" />
                        : <StopCircle size={10} style={{ color: 'rgba(var(--kr-success-rgb), 0.5)' }} />}
                    </button>
                    <span className="text-xs" style={{ color: isDisabled ? 'var(--kr-text-ghost)' : 'rgba(var(--kr-success-rgb), 0.6)' }}>
                      {isDisabled ? t('config.overrideDisabled') : t('config.overrideActive')}
                    </span>
                  </div>
                )}

                {/* Existing keys list */}
                {providerKeys.map(k => {
                  const isVis = tokenVisible.has(k.id);
                  return (
                  <div key={k.id} className="set-key-row" style={{ opacity: isDisabled ? 0.4 : 1 }}>
                    {k.active ? (
                      <Check size={9} style={{ color: 'rgba(var(--kr-success-rgb), 0.7)' }} className="flex-shrink-0" />
                    ) : (
                      <button className="set-icon-btn set-icon-btn-bare" title={t('config.activateKey')} aria-label={t('config.activateKey')}
                        onClick={async () => { try { await configApi.activateApiKey(k.id); } catch (err) { console.warn('Settings action failed:', err); toast(t('common.actionFailed', userError(err)), 'error'); } refetchTokens(); }}>
                        <div style={{ width: 9, height: 9, borderRadius: '50%', border: '1px solid var(--kr-text-ghost)' }} />
                      </button>
                    )}
                    <span className="text-xs text-tertiary truncate" style={{ minWidth: 60 }}>
                      {k.name}
                    </span>
                    <span
                      className={`set-token-badge ${isVis ? 'mono' : ''}`}
                      data-disabled={isDisabled}
                    >
                      {isVis ? k.masked_value : k.masked_value.replace(/[^.]/g, '\u2022')}
                    </span>
                    <button className="set-icon-btn set-icon-btn-bare" title={isVis ? 'Hide' : 'Show'} aria-label={isVis ? 'Hide API key' : 'Show API key'}
                      onClick={() => setTokenVisible(prev => {
                        const next = new Set(prev);
                        if (next.has(k.id)) next.delete(k.id); else next.add(k.id);
                        return next;
                      })}>
                      {isVis ? <EyeOff size={9} className="text-accent" /> : <Eye size={9} className="text-ghost" />}
                    </button>
                    <button className="set-icon-btn set-icon-btn-bare" title={t('config.deleteKey')} aria-label={t('config.deleteKey')}
                      onClick={async () => {
                        if (confirm(t('config.deleteKeyConfirm').replace('{0}', k.name))) {
                          try { await configApi.deleteApiKey(k.id); } catch (err) { console.warn('Settings action failed:', err); toast(t('common.actionFailed', userError(err)), 'error'); }
                          refetchTokens();
                        }
                      }}>
                      <Trash2 size={9} className="text-error" style={{ opacity: 0.5 }} />
                    </button>
                  </div>
                  );
                })}

                {/* No keys yet */}
                {providerKeys.length === 0 && !isAdding && (
                  <div className="flex-row gap-4" style={{ paddingLeft: 16 }}>
                    <Key size={10} className="text-ghost flex-shrink-0" />
                    <span className="text-xs text-faint">{t('config.localAuth')}</span>
                  </div>
                )}

                {/* Add key button / form */}
                {isAdding ? (
                  <div className="flex-row gap-3" style={{ padding: '4px 0 2px 16px' }}>
                    <input
                      type="text"
                      className="set-input set-input-xs"
                      style={{ width: 100 }}
                      placeholder={t('config.keyName')}
                      value={newInput.name}
                      onChange={e => setNewKeyInputs(prev => ({ ...prev, [tf.key]: { ...newInput, name: e.target.value } }))}
                    />
                    <input
                      type="password"
                      className="set-input set-input-xs flex-1"
                      style={{ maxWidth: 180 }}
                      placeholder={tf.hint}
                      value={newInput.value}
                      onChange={e => setNewKeyInputs(prev => ({ ...prev, [tf.key]: { ...newInput, value: e.target.value } }))}
                    />
                    {newInput.value && (
                      <button className="set-icon-btn text-accent" style={{ fontSize: 10 }} aria-label="Save API key"
                        onClick={async () => {
                          try {
                            await configApi.saveApiKey({
                              id: null,
                              name: newInput.name || t('config.defaultKeyName'),
                              provider: tf.key,
                              value: newInput.value,
                            });
                            setNewKeyInputs(prev => ({ ...prev, [tf.key]: { name: '', value: '' } }));
                            setAddingKeyFor(null);
                            refetchTokens();
                            if (confirm(t('config.syncTokensConfirm'))) {
                              const synced = await configApi.syncAgentTokens();
                              if (synced.length > 0) {
                                toast(t('config.syncTokensDone').replace('{0}', synced.join(', ')), 'success');
                              } else {
                                toast(t('config.syncTokensNone'), 'info');
                              }
                            }
                          } catch (e) { toast(String(e) || t('config.saveError'), 'error'); }
                        }}>
                        <Save size={10} />
                      </button>
                    )}
                    <button className="set-icon-btn set-icon-btn-bare" onClick={() => setAddingKeyFor(null)} aria-label="Cancel">
                      <X size={10} className="text-dim" />
                    </button>
                  </div>
                ) : (
                  <button
                    className="set-icon-btn text-dim"
                    style={{ fontSize: 10, padding: '2px 0 0 16px', gap: 4 }}
                    onClick={() => {
                      setAddingKeyFor(tf.key);
                      setNewKeyInputs(prev => ({
                        ...prev,
                        [tf.key]: { name: providerKeys.length === 0 ? t('config.defaultKeyName') : '', value: '' },
                      }));
                    }}
                  >
                    <Plus size={9} /> {t('config.addKey')}
                  </button>
                )}
              </div>
              );
            })()}
            {/* Model tier configuration */}
            {(agent.installed || agent.runtime_available) && (() => {
              const agentKey = ({
                ClaudeCode: 'claude_code',
                Codex: 'codex',
                GeminiCli: 'gemini_cli',
                Kiro: 'kiro',
                Vibe: 'vibe',
                CopilotCli: 'copilot_cli',
                // KT-337 — NVIDIA joins the generic tier block. Its catalogue is
                // fetched, not hardcoded: ~100 ids across 25 vendors, several of
                // which this account cannot call, so a baked-in list would lie.
                Nvidia: 'nvidia',
              } as Partial<Record<AgentType, string>>)[agent.agent_type];
              if (!agentKey) return null;
              const editing = tierEditing[agentKey];
              if (!editing) return null;

              // `fallback*` = the backend's BUILT-IN model when no override is
              // set (runner.rs::resolve_model_flag) — shown in the empty-value
              // option label so "Par défaut (…)" never lies about what actually
              // runs. `null` = the agent's own default (no --model flag).
              const knownModels: Record<string, {
                options: string[];
                fallbackEconomy: string | null; fallbackDefault: string | null; fallbackReasoning: string | null;
                modelsUrl: string;
              }> = {
                claude_code: {
                  options: ['haiku', 'sonnet', 'fable', 'opus'],
                  fallbackEconomy: 'haiku', fallbackDefault: 'sonnet', fallbackReasoning: 'opus',
                  modelsUrl: 'https://docs.anthropic.com/en/docs/about-claude/models',
                },
                codex: {
                  options: [
                    'gpt-5.6-luna', 'gpt-5.6-terra', 'gpt-5.6-sol',
                    'gpt-5.5', 'gpt-5.4', 'gpt-5.4-mini', 'gpt-5-codex-mini',
                  ],
                  fallbackEconomy: 'gpt-5.6-luna', fallbackDefault: null, fallbackReasoning: 'gpt-5.6-sol',
                  modelsUrl: 'https://developers.openai.com/codex/models',
                },
                gemini_cli: {
                  options: [
                    'gemini-2.5-flash-lite', 'gemini-2.5-flash',
                    'gemini-3-flash-preview', 'gemini-2.5-pro', 'gemini-3.1-pro-preview',
                  ],
                  fallbackEconomy: 'gemini-2.5-flash', fallbackDefault: null, fallbackReasoning: 'gemini-3.1-pro-preview',
                  modelsUrl: 'https://ai.google.dev/gemini-api/docs/models',
                },
                kiro: { options: [], fallbackEconomy: null, fallbackDefault: null, fallbackReasoning: null, modelsUrl: '' },
                vibe: { options: [], fallbackEconomy: null, fallbackDefault: null, fallbackReasoning: null, modelsUrl: '' },
                nvidia: {
                  // Filled at runtime from /v1/models (see nvidiaCatalogue).
                  options: [],
                  // No built-in default on purpose: the backend refuses to guess
                  // an id, because a wrong one 404s or hangs.
                  fallbackEconomy: null, fallbackDefault: null, fallbackReasoning: null,
                  modelsUrl: 'https://build.nvidia.com/models',
                },
                copilot_cli: {
                  // Copilot's enabled models depend on the account and its
                  // policy. Keep only current CLI identifiers here; an empty
                  // tier lets Copilot choose an account-compatible model.
                  options: ['auto', 'claude-sonnet-4-5', 'claude-sonnet-4', 'gpt-5'],
                  fallbackEconomy: null, fallbackDefault: null, fallbackReasoning: null,
                  modelsUrl: 'https://docs.github.com/en/copilot',
                },
              };
              const models = knownModels[agentKey];

              const saveTiers = async (field: 'economy' | 'default' | 'reasoning', value: string) => {
                const newEditing = { ...tierEditing, [agentKey]: { ...editing, [field]: value } };
                setTierEditing(newEditing);
                // `default` is included for EVERY agent — pre-fix it was
                // omitted from this payload, so any save here silently wiped
                // the Default-tier override (e.g. the Ollama model picked
                // via OllamaCard, which writes the same field).
                const tierOf = (k: string) => ({
                  economy: newEditing[k]?.economy || null,
                  default: newEditing[k]?.default || null,
                  reasoning: newEditing[k]?.reasoning || null,
                });
                const newTiers: ModelTiersConfig = {
                  claude_code: tierOf('claude_code'),
                  codex: tierOf('codex'),
                  open_code: tierOf('open_code'),
                  gemini_cli: tierOf('gemini_cli'),
                  kiro: tierOf('kiro'),
                  vibe: tierOf('vibe'),
                  copilot_cli: tierOf('copilot_cli'),
                  ollama: tierOf('ollama'),
                  lite_llm: tierOf('lite_llm'),
                  nvidia: tierOf('nvidia'),
                };
                try { await configApi.setModelTiers(newTiers); toast(t('config.saved'), 'success'); } catch { toast(t('config.saveError'), 'error'); }
              };

              // KT-337 — a fetched-catalogue provider needs a free-text field, not a
              // dropdown: the catalogue is long, partly uncallable, and a pasted id
              // must be accepted. The datalist offers the catalogue as suggestions
              // once loaded; the badge shows what a real probe said, because being
              // listed is NOT being usable for this provider.
              const renderCatalogueInput = (field: 'economy' | 'default' | 'reasoning', icon: string, iconColor: string) => {
                const current = editing[field] ?? '';
                const probe = current ? nvidiaProbes[current] : undefined;
                const badge = probe
                  ? (probe.verdict === 'Usable' ? '✅' : probe.verdict === 'NoAnswer' ? '⏳' : '⛔')
                  : '';
                const draftKey = `${agentKey}:${field}`;
                const shown = nvidiaDraft[draftKey] ?? current;
                const commit = (raw: string) => {
                  const value = raw.trim();
                  setNvidiaDraft(d => { const next = { ...d }; delete next[draftKey]; return next; });
                  if (value !== current) saveTiers(field, value);
                };
                return (
                  <div className="flex-row gap-2" key={field}>
                    <span className="text-2xs" style={{ color: iconColor, width: 14 }} title={field}>{icon}</span>
                    <input
                      className="set-tier-input"
                      list="nvidia-model-catalogue"
                      data-model-tier-agent={agent.agent_type}
                      data-model-tier={field}
                      placeholder={t('config.defaultModel')}
                      value={shown}
                      onChange={e => {
                        const next = e.target.value;
                        setNvidiaDraft(d => ({ ...d, [draftKey]: next }));
                        // A datalist pick lands as a complete catalogue id in one
                        // change event: commit it right away instead of waiting for
                        // blur. Free text keeps committing on Enter/blur so a save
                        // does not fire on every keystroke.
                        if ((nvidiaCatalogue ?? []).includes(next)) commit(next);
                      }}
                      onKeyDown={e => { if (e.key === 'Enter') commit((e.target as HTMLInputElement).value); }}
                      onBlur={e => commit(e.target.value)}
                      aria-label={t('disc.modelTier') + ' ' + field}
                    />
                    {current && (
                      <button
                        className="set-icon-btn"
                        style={{ fontSize: 10 }}
                        title={probe ? probe.detail : t('config.nvidiaVerifyModel')}
                        onClick={() => probeNvidiaModel(current)}
                      >{badge || '🔍'}</button>
                    )}
                  </div>
                );
              };

              const renderSelect = (field: 'economy' | 'default' | 'reasoning', options: string[], icon: string, iconColor: string, fallback: string | null) => {
                if (agent.agent_type === 'Nvidia') return renderCatalogueInput(field, icon, iconColor);
                if (options.length === 0) return (
                  <span className="text-2xs text-ghost" style={{ padding: '2px 6px' }}>{icon} N/A</span>
                );
                return (
                  <div className="flex-row gap-2">
                    <span className="text-2xs" style={{ color: iconColor, width: 14 }} title={field}>{icon}</span>
                    <SearchableSelect
                      className="searchable-select--compact set-agent-model-select"
                      value={editing[field]}
                      options={options.map(model => ({
                        value: model,
                        label: model,
                        keywords: model.replaceAll('/', ' '),
                        description: modelCostSuffix(model) || undefined,
                      }))}
                      onChange={value => void saveTiers(field, value)}
                      label={`${t('disc.modelTier')} ${field}`}
                      placeholder={t('config.searchModel')}
                      emptyLabel={t('config.searchModelEmpty')}
                      clearLabel={`${t('config.defaultModel')}${fallback ? ` (${fallback}${modelCostSuffix(fallback)})` : ''}`}
                      dataModelTierAgent={agent.agent_type}
                      dataModelTier={field}
                    />
                  </div>
                );
              };

              return (
                <div className="set-agent-panel set-agent-panel-models">
                  <div className="set-agent-section-title">
                    <span>{t('disc.modelTier')}</span>
                    {usageReport && (
                      <ContextHelp title={t('config.modelCostObservedTitle')}>
                        <p>{t('config.modelCostObservedHelp')}</p>
                      </ContextHelp>
                    )}
                    {models.modelsUrl && (
                      <a href={models.modelsUrl} target="_blank" rel="noopener noreferrer"
                        title={t('config.viewModels')}
                      >
                        <ExternalLink size={8} /> {t('config.viewModels')}
                      </a>
                    )}
                  </div>
                  <div className="flex-row gap-5">
                    {agent.agent_type === 'Nvidia' && (
                      <>
                        {/* One shared datalist feeds the three tier inputs. */}
                        <datalist id="nvidia-model-catalogue">
                          {(nvidiaCatalogue ?? []).map(id => <option key={id} value={id} />)}
                        </datalist>
                        <button
                          className="set-icon-btn text-2xs"
                          disabled={nvidiaLoading}
                          title={t('config.nvidiaLoadCatalogue')}
                          onClick={() => loadNvidiaCatalogue()}
                        >
                          {nvidiaCatalogue ? `${nvidiaCatalogue.length} ⟳` : (nvidiaLoading ? '…' : '⟳')}
                        </button>
                      </>
                    )}
                    {renderSelect('economy', models.options, '\u26A1', 'rgba(var(--kr-success-rgb), 0.6)', models.fallbackEconomy)}
                    {renderSelect('default', models.options, '\uD83C\uDFAF', 'rgba(var(--kr-info-rgb), 0.6)', models.fallbackDefault)}
                    {renderSelect('reasoning', models.options, '\uD83E\uDDE0', 'rgba(var(--kr-warning-amber-rgb), 0.6)', models.fallbackReasoning)}
                  </div>
                </div>
              );
            })()}
            </div>
          </div>
          </React.Fragment>
          );
        })}

        {/* KT-339 — unified External API zone: LiteLLM, NVIDIA and any other
            OpenAI-compatible service live here as named connections. */}
        <ExternalApiSection
          t={t}
          toast={toast}
          modelCostSuffix={modelCostSuffix}
          onModelTiersChanged={refetchAgentAccess}
        />

        {/* Best practices links */}
        <div className="set-best-practices">
          <div className="flex-row gap-3 text-sm font-semibold mb-4" style={{ color: 'rgba(var(--kr-accent-rgb), 0.6)' }}>
            <ExternalLink size={10} /> {t('config.bestPractices')}
          </div>
          <div className="flex-wrap text-xs" style={{ gap: '6px 16px' }}>
            <a href="https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents" target="_blank" rel="noopener noreferrer">Anthropic — Context Engineering</a>
            <a href="https://help.openai.com/en/articles/6654000-best-practices-for-prompt-engineering-with-the-openai-api" target="_blank" rel="noopener noreferrer">OpenAI — Prompt Engineering</a>
            <a href="https://help.mistral.ai/en/articles/347476-how-to-write-good-instructions-for-my-agent" target="_blank" rel="noopener noreferrer">Mistral — Agent Instructions</a>
            <a href="https://ai.google.dev/gemini-api/docs/prompting-strategies" target="_blank" rel="noopener noreferrer">Google — Gemini Prompting</a>
            <a href="https://kiro.dev/docs/cli/custom-agents/configuration-reference/" target="_blank" rel="noopener noreferrer">Kiro — Agent Configuration</a>
          </div>
        </div>

      {/* Per-agent update modal — opened from the freshness pill on a
       *  given agent row. Surfaces the same install_command the backend
       *  uses for fresh installs (npm/curl/uv tool install scripts are
       *  idempotent — re-running upgrades in place). */}
      {updateModalFor && (
        <div className="dash-modal-overlay" onClick={() => setUpdateModalFor(null)}>
          <div
            className="dash-modal set-compression-modal"
            onClick={e => e.stopPropagation()}
            role="dialog"
            aria-modal="true"
            aria-labelledby="agent-update-title"
            onKeyDown={e => { if (e.key === 'Escape') setUpdateModalFor(null); }}
          >
            <div className="dash-modal-header">
              <h3 id="agent-update-title" className="dash-modal-title">
                {t('config.agentUpdateModalTitle', updateModalFor.name)}
              </h3>
              <button
                onClick={() => setUpdateModalFor(null)}
                className="dash-modal-close"
                aria-label="Close"
              >
                <X size={16} />
              </button>
            </div>
            <div className="set-compression-modal-body">
              <p>
                {t('config.agentUpdateModalBody',
                  updateModalFor.version ?? '?',
                  updateModalFor.latest_version ?? '?')}
              </p>
              <div className="set-compression-install-label">{t('config.rtk.installCommand')}</div>
              <pre className="set-compression-install-cmd">{updateModalFor.install_command ?? ''}</pre>
              <button
                type="button"
                className="set-compression-copy-btn"
                onClick={() => navigator.clipboard.writeText(updateModalFor.install_command ?? '').catch(() => {})}
                aria-label={t('common.copy')}
              >
                <Copy size={12} /> {t('common.copy')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
