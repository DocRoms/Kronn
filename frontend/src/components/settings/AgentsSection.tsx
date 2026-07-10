import { useState, useEffect, useRef } from 'react';
import { config as configApi, agents as agentsApi } from '../../lib/api';
import { userError } from '../../lib/userError';
import { OllamaCard } from './OllamaCard';
import { CompressionSection } from './CompressionSection';
import { useApi } from '../../hooks/useApi';
import type { AgentDetection, AgentsConfig, ModelTiersConfig } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';
import { isUpdateAvailable } from '../../lib/version';
import {
  Key, AlertTriangle, Save,
  Plus, Trash2, Download, Check,
  Loader2, RefreshCw, X, Eye, EyeOff, Play, StopCircle,
  ExternalLink, FolderSearch, ArrowUpCircle, Copy,
} from 'lucide-react';
import '../../pages/SettingsPage.css';

interface AgentsSectionProps {
  agents: AgentDetection[];
  agentAccess: AgentsConfig | null;
  configLanguage: string | null;
  refetchAgents: () => void;
  refetchAgentAccess: () => void;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
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
  inDocker = false,
}: AgentsSectionProps) {
  const [installing, setInstalling] = useState<string | null>(null);
  const [newKeyInputs, setNewKeyInputs] = useState<Record<string, { name: string; value: string }>>({});
  const [addingKeyFor, setAddingKeyFor] = useState<string | null>(null);
  const [tokenVisible, setTokenVisible] = useState<Set<string>>(new Set());
  const [tierEditing, setTierEditing] = useState<Record<string, { economy: string; default: string; reasoning: string }>>({});
  // When set to an agent name, the per-agent update modal is shown. The
  // modal is small + global to the section (rather than per-row state) so
  // we never re-render rows on its open/close — keeps the agent grid
  // scroll position stable.
  const [updateModalFor, setUpdateModalFor] = useState<AgentDetection | null>(null);

  const { data: tokenConfig, refetch: refetchTokens } = useApi(() => configApi.getTokens(), []);

  // 0.8.6 phase 4 — global default tier + summary strategy (applied to
  // NEW disc / QP / WF agent steps when the form doesn't explicitly
  // pick one). Strict semantic (cf. backend `ServerConfig.
  // default_model_tier` rustdoc) — never retroactive. Loaded once on
  // mount, written back on every change via setServerConfig PATCH.
  const [defaultTier, setDefaultTier] = useState<'economy' | 'default' | 'reasoning' | null>(null);
  const [defaultSummaryStrategy, setDefaultSummaryStrategy] = useState<'Auto' | 'OnDemand' | 'Off' | null>(null);
  useEffect(() => {
    configApi.getServerConfig().then(cfg => {
      if (cfg) {
        setDefaultTier(cfg.default_model_tier ?? 'default');
        setDefaultSummaryStrategy(cfg.default_summary_strategy ?? 'Off');
      }
    }).catch(() => {
      setDefaultTier('default');
      setDefaultSummaryStrategy('Off');
    });
  }, []);

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

  // Load model tiers once. Pre-fix the loop only seeded 5 of the 7 agents
  // (copilot_cli + ollama were missing), so when the user opened the
  // tier dropdowns for those two agents the inputs showed empty even
  // when the backend already had saved values — and a subsequent save
  // wiped the side that wasn't being edited (saveTiers reads from the
  // editing map and writes both economy + reasoning back to the API).
  useEffect(() => {
    configApi.getModelTiers().then(tiers => {
      if (tiers) {
        const editing: Record<string, { economy: string; default: string; reasoning: string }> = {};
        for (const key of ['claude_code', 'codex', 'gemini_cli', 'kiro', 'vibe', 'copilot_cli', 'ollama'] as const) {
          editing[key] = { economy: tiers[key]?.economy ?? '', default: tiers[key]?.default ?? '', reasoning: tiers[key]?.reasoning ?? '' };
        }
        setTierEditing(editing);
      }
    }).catch(() => {});
  }, []);

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

  return (
    <div>
      <div>
        <div className="flex-row gap-4 mb-4" style={{ justifyContent: 'flex-end' }}>
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

        <CompressionSection agents={agents} onActivated={refetchAgents} toast={toast} t={t} />

        {/* 0.8.6 phase 4 — Default model tier for new disc / QP / WF
            agent steps. Strict semantic — never retroactive. Sibling of
            the RTK CompressionSection above ; both live in the "Mode IA"
            card because they're the two cross-cutting agent-cost knobs. */}
        <div
          className="set-default-tier-section"
          data-testid="default-tier-section"
          style={{
            padding: '14px 16px',
            marginBottom: 12,
            borderRadius: 'var(--kr-radius-md, 8px)',
            background: 'var(--kr-bg-card-subtle, transparent)',
            border: '1px solid var(--kr-border-subtle, transparent)',
          }}
        >
          <div className="flex-row gap-2 mb-2">
            <span style={{ fontSize: 14 }}>🎯</span>
            <span className="font-semibold text-sm">{t('config.defaultTierLabel')}</span>
          </div>
          <p className="text-xs text-muted mb-2" style={{ marginTop: 0 }}>
            {t('config.defaultTierHint')}
          </p>
          <div className="flex-row gap-1" role="radiogroup" aria-label={t('config.defaultTierLabel')}>
            {(['economy', 'default', 'reasoning'] as const).map(tier => {
              const tierIcons: Record<typeof tier, string> = { economy: '⚡', default: '🎯', reasoning: '🧠' };
              const tierLabels: Record<typeof tier, string> = {
                economy: t('disc.tier.economy'),
                default: t('disc.tier.default'),
                reasoning: t('disc.tier.reasoning'),
              };
              const active = defaultTier === tier;
              return (
                <button
                  key={tier}
                  type="button"
                  role="radio"
                  aria-checked={active}
                  className="disc-tier-btn"
                  data-active={active}
                  data-testid={`default-tier-btn-${tier}`}
                  onClick={() => saveDefaultTier(tier)}
                  disabled={defaultTier === null}
                  title={tierLabels[tier]}
                >
                  <span style={{ marginRight: 4 }}>{tierIcons[tier]}</span>
                  {tierLabels[tier]}
                </button>
              );
            })}
          </div>
        </div>

        {/* 0.8.6 phase 4 — Default summary strategy. Auto-summary used
            to fire after every reply ; flipped to OFF by default in
            0.8.6 since modern agents have large context + MCP access
            to fetch older history on demand. Re-enable for small-
            context agents that can't ask Kronn for context themselves. */}
        <div
          className="set-default-summary-section"
          data-testid="default-summary-section"
          style={{
            padding: '14px 16px',
            marginBottom: 12,
            borderRadius: 'var(--kr-radius-md, 8px)',
            background: 'var(--kr-bg-card-subtle, transparent)',
            border: '1px solid var(--kr-border-subtle, transparent)',
          }}
        >
          <div className="flex-row gap-2 mb-2">
            <span style={{ fontSize: 14 }}>📝</span>
            <span className="font-semibold text-sm">{t('config.defaultSummaryLabel')}</span>
          </div>
          <p className="text-xs text-muted mb-2" style={{ marginTop: 0 }}>
            {t('config.defaultSummaryHint')}
          </p>
          <div className="flex-row gap-1" role="radiogroup" aria-label={t('config.defaultSummaryLabel')}>
            {(['Off', 'Auto', 'OnDemand'] as const).map(strategy => {
              const icons: Record<typeof strategy, string> = { Off: '🚫', Auto: '🔄', OnDemand: '👋' };
              const labels: Record<typeof strategy, string> = {
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
                  className="disc-tier-btn"
                  data-active={active}
                  data-testid={`default-summary-btn-${strategy.toLowerCase()}`}
                  onClick={() => saveDefaultSummary(strategy)}
                  disabled={defaultSummaryStrategy === null}
                  title={labels[strategy]}
                >
                  <span style={{ marginRight: 4 }}>{icons[strategy]}</span>
                  {labels[strategy]}
                </button>
              );
            })}
          </div>
        </div>

        {inDocker && (
          <div className="set-agent-runtime-warning" role="note">
            ⚠️ {t('config.dockerInstallNote')}
          </div>
        )}

        {agents.map(agent => {
          // Ollama gets its own dedicated card with health check + model picker
          if (agent.agent_type === 'Ollama') {
            return <OllamaCard key="ollama" t={t} />;
          }

          const permFlag: Record<string, { flag: string; descKey: string }> = {
            ClaudeCode: { flag: '--dangerously-skip-permissions', descKey: 'config.fullAccess' },
            Codex: { flag: '--full-auto', descKey: 'config.autoApply' },
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
          };
          const tf = tokenField[agent.agent_type];
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
          <div key={agent.name} className="set-agent-row">
            <div className="flex-row gap-5">
              <div className="relative">
                <div className="set-dot" data-on={agent.installed && agent.enabled} aria-hidden="true" />
                <span className="set-sr-only">
                  {agent.installed && agent.enabled ? t('config.enabled') : t('config.disabled')}
                </span>
              </div>
              <div className="flex-1">
                <div className="flex-row gap-3">
                  <span className="font-semibold text-base">{agent.name}</span>
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
              </div>
              {/* Only a binary actually installed in the container gets the
                  enable/uninstall controls. An agent reachable only via the
                  npx runtime is NOT considered installed — it keeps the
                  "runtime OK — via npx" hint above and is offered for a real
                  install, so the user isn't told an agent they never installed
                  is "Activé". */}
              {agent.installed ? (
                <div className="flex-row gap-3">
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
                </div>
              ) : (
                <button
                  className="set-install-btn"
                  style={{ padding: '4px 10px', fontSize: 11 }}
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
            {perm && (agent.installed || agent.runtime_available) && (
              <div className="set-perm-box">
                <div
                  role="switch"
                  aria-checked={isFullAccess}
                  tabIndex={0}
                  className="flex-row gap-4 cursor-pointer"
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
                  <code className={`text-xs ${isFullAccess ? 'text-accent' : 'text-muted'}`}>{perm.flag}</code>
                </div>
                <p className="text-xs text-muted" style={{ marginTop: 4, marginLeft: 38 }}>
                  {t(perm.descKey)}
                </p>
              </div>
            )}
            {tf && (agent.installed || agent.runtime_available) && (() => {
              const providerKeys = tokenConfig?.keys?.filter(k => k.provider === tf.key) ?? [];
              const isDisabled = tokenConfig?.disabled_overrides?.includes(tf.key);
              const isAdding = addingKeyFor === tf.key;
              const newInput = newKeyInputs[tf.key] ?? { name: '', value: '' };
              return (
              <div className="set-agent-sub">
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
                    <a
                      href={tf.url} target="_blank" rel="noopener noreferrer"
                      className="flex-row text-ghost flex-shrink-0"
                      style={{ marginLeft: 'auto' }}
                      title={t('config.getKey')}
                    >
                      <ExternalLink size={10} />
                    </a>
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
                    <a href={tf.url} target="_blank" rel="noopener noreferrer"
                      className="flex-row text-ghost flex-shrink-0"
                      title={t('config.getKey')}>
                      <ExternalLink size={10} />
                    </a>
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
              const agentKey = agent.agent_type === 'ClaudeCode' ? 'claude_code'
                : agent.agent_type === 'Codex' ? 'codex'
                : agent.agent_type === 'GeminiCli' ? 'gemini_cli'
                : agent.agent_type === 'Kiro' ? 'kiro'
                : 'vibe';
              const editing = tierEditing[agentKey];
              if (!editing) return null;

              // `fallback*` = the backend's BUILT-IN model when no override is
              // set (runner.rs::resolve_model_flag) — shown in the empty-value
              // option label so "Par défaut (…)" never lies about what actually
              // runs. `null` = the agent's own default (no --model flag).
              const knownModels: Record<string, {
                economy: string[]; default: string[]; reasoning: string[];
                fallbackEconomy: string | null; fallbackDefault: string | null; fallbackReasoning: string | null;
                modelsUrl: string;
              }> = {
                claude_code: {
                  economy: ['haiku', 'sonnet'],
                  default: ['sonnet', 'fable', 'opus', 'haiku'],
                  // Fable 5 (released 2026) is the new top model. Listed
                  // first so it surfaces for new tier picks; the built-in
                  // fallback stays `opus` (label below stays truthful via
                  // fallbackReasoning). Verified: the claude CLI accepts
                  // the `fable` alias for --model (tested 2026-06-10).
                  reasoning: ['fable', 'opus', 'sonnet'],
                  fallbackEconomy: 'haiku', fallbackDefault: 'sonnet', fallbackReasoning: 'opus',
                  modelsUrl: 'https://docs.anthropic.com/en/docs/about-claude/models',
                },
                codex: {
                  economy: ['gpt-5-codex-mini', 'gpt-5.1-codex', 'gpt-5-codex'],
                  default: ['gpt-5.5', 'gpt-5.4', 'gpt-5.1-codex'],
                  reasoning: ['gpt-5.5', 'gpt-5.4', 'gpt-5.3-codex', 'gpt-5.2-codex', 'gpt-5.1-codex-max'],
                  fallbackEconomy: 'gpt-5-codex-mini', fallbackDefault: null, fallbackReasoning: 'gpt-5.4',
                  modelsUrl: 'https://developers.openai.com/codex/models',
                },
                gemini_cli: {
                  economy: ['gemini-2.5-flash', 'gemini-2.5-flash-lite', 'gemini-3-flash-preview'],
                  default: ['gemini-3.1-pro-preview', 'gemini-2.5-flash'],
                  reasoning: ['gemini-3.1-pro-preview', 'gemini-2.5-pro'],
                  fallbackEconomy: 'gemini-2.5-flash', fallbackDefault: null, fallbackReasoning: 'gemini-3.1-pro-preview',
                  modelsUrl: 'https://ai.google.dev/gemini-api/docs/models',
                },
                kiro: { economy: [], default: [], reasoning: [], fallbackEconomy: null, fallbackDefault: null, fallbackReasoning: null, modelsUrl: '' },
                vibe: { economy: [], default: [], reasoning: [], fallbackEconomy: null, fallbackDefault: null, fallbackReasoning: null, modelsUrl: '' },
                copilot_cli: {
                  economy: ['gpt-4o-mini'], default: ['o4-mini', 'gpt-4o-mini'], reasoning: ['o4-mini'],
                  fallbackEconomy: 'gpt-4o-mini', fallbackDefault: null, fallbackReasoning: 'o4-mini',
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
                  gemini_cli: tierOf('gemini_cli'),
                  kiro: tierOf('kiro'),
                  vibe: tierOf('vibe'),
                  copilot_cli: tierOf('copilot_cli'),
                  ollama: tierOf('ollama'),
                };
                try { await configApi.setModelTiers(newTiers); toast(t('config.saved'), 'success'); } catch { toast(t('config.saveError'), 'error'); }
              };

              const renderSelect = (field: 'economy' | 'default' | 'reasoning', options: string[], icon: string, iconColor: string, fallback: string | null) => {
                if (options.length === 0) return (
                  <span className="text-2xs text-ghost" style={{ padding: '2px 6px' }}>{icon} N/A</span>
                );
                return (
                  <div className="flex-row gap-2">
                    <span className="text-2xs" style={{ color: iconColor, width: 14 }} title={field}>{icon}</span>
                    <select
                      className="set-tier-select"
                      value={editing[field]}
                      onChange={e => saveTiers(field, e.target.value)}
                      aria-label={t('disc.modelTier') + ' ' + field}
                    >
                      {/* Empty value = the backend built-in fallback (passed in
                          so the label matches runner.rs, not just options[0]).
                          No fallback = the agent's own default model. */}
                      <option value="">{t('config.defaultModel')}{fallback ? ` (${fallback})` : ''}</option>
                      {options.map(m => (
                        <option key={m} value={m}>{m}</option>
                      ))}
                    </select>
                  </div>
                );
              };

              return (
                <div className="set-agent-sub">
                  <div className="flex-row gap-3 mb-2">
                    <span className="text-xs text-muted font-semibold">{t('disc.modelTier')}</span>
                    {models.modelsUrl && (
                      <a href={models.modelsUrl} target="_blank" rel="noopener noreferrer"
                        className="flex-row gap-1 text-2xs" style={{ color: 'rgba(var(--kr-info-rgb), 0.5)', textDecoration: 'none' }}
                        title={t('config.viewModels')}
                      >
                        <ExternalLink size={8} /> {t('config.viewModels')}
                      </a>
                    )}
                  </div>
                  <div className="flex-row gap-5">
                    {renderSelect('economy', models.economy, '\u26A1', 'rgba(var(--kr-success-rgb), 0.6)', models.fallbackEconomy)}
                    {renderSelect('default', models.default, '\uD83C\uDFAF', 'rgba(var(--kr-info-rgb), 0.6)', models.fallbackDefault)}
                    {renderSelect('reasoning', models.reasoning, '\uD83E\uDDE0', 'rgba(var(--kr-warning-amber-rgb), 0.6)', models.fallbackReasoning)}
                  </div>
                </div>
              );
            })()}

          </div>
          );
        })}
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
