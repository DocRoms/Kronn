import { useState, useEffect } from 'react';
import { config as configApi, agents as agentsApi } from '../../lib/api';
import { useApi } from '../../hooks/useApi';
import type { AgentDetection, AgentsConfig, ModelTiersConfig } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';
import {
  Key, AlertTriangle, Save,
  Plus, Trash2, Download, Check,
  Loader2, RefreshCw, X, Eye, EyeOff, Play, StopCircle,
  ExternalLink, FolderSearch,
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
}

export function AgentsSection({
  agents,
  agentAccess,
  refetchAgents,
  refetchAgentAccess,
  toast,
  t,
}: AgentsSectionProps) {
  const [installing, setInstalling] = useState<string | null>(null);
  const [newKeyInputs, setNewKeyInputs] = useState<Record<string, { name: string; value: string }>>({});
  const [addingKeyFor, setAddingKeyFor] = useState<string | null>(null);
  const [tokenVisible, setTokenVisible] = useState<Set<string>>(new Set());
  const [tierEditing, setTierEditing] = useState<Record<string, { economy: string; reasoning: string }>>({});

  const { data: tokenConfig, refetch: refetchTokens } = useApi(() => configApi.getTokens(), []);

  // Load model tiers once
  useEffect(() => {
    configApi.getModelTiers().then(tiers => {
      if (tiers) {
        const editing: Record<string, { economy: string; reasoning: string }> = {};
        for (const key of ['claude_code', 'codex', 'gemini_cli', 'kiro', 'vibe'] as const) {
          editing[key] = { economy: tiers[key]?.economy ?? '', reasoning: tiers[key]?.reasoning ?? '' };
        }
        setTierEditing(editing);
      }
    }).catch(() => {});
  }, []);

  const handleInstallAgent = async (agent: AgentDetection) => {
    setInstalling(agent.name);
    try {
      await agentsApi.install(agent.agent_type);
      refetchAgents();
    } catch {
      // silently fail
    } finally {
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

        {agents.map(agent => {
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
                <div className="set-dot" data-on={(agent.installed || agent.runtime_available) && agent.enabled} aria-hidden="true" />
                <span className="set-sr-only">
                  {(agent.installed || agent.runtime_available) && agent.enabled ? t('config.enabled') : t('config.disabled')}
                </span>
              </div>
              <div className="flex-1">
                <div className="flex-row gap-3">
                  <span className="font-semibold text-base">{agent.name}</span>
                  <span className="set-origin-badge">{agent.origin}</span>
                  {agent.version && <code className="set-code text-xs">v{agent.version}</code>}
                  {agent.latest_version && agent.latest_version !== agent.version && (
                    <span className="set-update-badge">&#x2B06; {agent.latest_version}</span>
                  )}
                </div>
                {!agent.installed && !agent.runtime_available && (
                  <div className="text-xs text-faint mt-2">
                    <code className="set-code">{agent.install_command}</code>
                  </div>
                )}
                {!agent.installed && agent.runtime_available && (
                  <div className="text-xs mt-2" style={{ color: 'rgba(52,211,153,0.5)' }}>
                    runtime OK <span className="text-ghost">— via npx</span>
                  </div>
                )}
              </div>
              {(agent.installed || agent.runtime_available) ? (
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
                  disabled={installing !== null}
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
                    try { await configApi.setAgentAccess({ agent: agent.agent_type, full_access: !isFullAccess }); } catch (err) { console.warn('Settings action failed:', err); }
                    refetchAgentAccess();
                  }}
                  onKeyDown={async (e) => {
                    if (e.key === ' ' || e.key === 'Enter') {
                      e.preventDefault();
                      try { await configApi.setAgentAccess({ agent: agent.agent_type, full_access: !isFullAccess }); } catch (err) { console.warn('Settings action failed:', err); }
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
                      onClick={async () => {
                        try { await configApi.toggleTokenOverride(tf.key); } catch (err) { console.warn('Settings action failed:', err); }
                        refetchTokens();
                      }}
                    >
                      {isDisabled
                        ? <Play size={10} className="text-ghost" />
                        : <StopCircle size={10} style={{ color: 'rgba(52,211,153,0.5)' }} />}
                    </button>
                    <span className="text-xs" style={{ color: isDisabled ? 'rgba(255,255,255,0.25)' : 'rgba(52,211,153,0.6)' }}>
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
                      <Check size={9} style={{ color: 'rgba(52,211,153,0.7)' }} className="flex-shrink-0" />
                    ) : (
                      <button className="set-icon-btn set-icon-btn-bare" title={t('config.activateKey')} aria-label={t('config.activateKey')}
                        onClick={async () => { try { await configApi.activateApiKey(k.id); } catch (err) { console.warn('Settings action failed:', err); } refetchTokens(); }}>
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
                          try { await configApi.deleteApiKey(k.id); } catch (err) { console.warn('Settings action failed:', err); }
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

              const knownModels: Record<string, { economy: string[]; reasoning: string[]; modelsUrl: string }> = {
                claude_code: {
                  economy: ['haiku', 'sonnet'],
                  reasoning: ['opus', 'sonnet'],
                  modelsUrl: 'https://docs.anthropic.com/en/docs/about-claude/models',
                },
                codex: {
                  economy: ['gpt-5-codex-mini', 'gpt-5.1-codex', 'gpt-5-codex'],
                  reasoning: ['gpt-5.4', 'gpt-5.3-codex', 'gpt-5.2-codex', 'gpt-5.1-codex-max'],
                  modelsUrl: 'https://developers.openai.com/codex/models',
                },
                gemini_cli: {
                  economy: ['gemini-2.5-flash', 'gemini-2.5-flash-lite', 'gemini-3-flash-preview'],
                  reasoning: ['gemini-3.1-pro-preview', 'gemini-2.5-pro'],
                  modelsUrl: 'https://ai.google.dev/gemini-api/docs/models',
                },
                kiro: { economy: [], reasoning: [], modelsUrl: '' },
                vibe: { economy: [], reasoning: [], modelsUrl: '' },
                copilot_cli: { economy: ['gpt-4o-mini'], reasoning: ['o4-mini'], modelsUrl: 'https://docs.github.com/en/copilot' },
              };
              const models = knownModels[agentKey];

              const saveTiers = async (field: 'economy' | 'reasoning', value: string) => {
                const newEditing = { ...tierEditing, [agentKey]: { ...editing, [field]: value } };
                setTierEditing(newEditing);
                const newTiers: ModelTiersConfig = {
                  claude_code: { economy: newEditing.claude_code?.economy || null, reasoning: newEditing.claude_code?.reasoning || null },
                  codex: { economy: newEditing.codex?.economy || null, reasoning: newEditing.codex?.reasoning || null },
                  gemini_cli: { economy: newEditing.gemini_cli?.economy || null, reasoning: newEditing.gemini_cli?.reasoning || null },
                  kiro: { economy: newEditing.kiro?.economy || null, reasoning: newEditing.kiro?.reasoning || null },
                  vibe: { economy: newEditing.vibe?.economy || null, reasoning: newEditing.vibe?.reasoning || null },
                  copilot_cli: { economy: newEditing.copilot_cli?.economy || null, reasoning: newEditing.copilot_cli?.reasoning || null },
                };
                try { await configApi.setModelTiers(newTiers); toast(t('config.saved'), 'success'); } catch { toast(t('config.saveError'), 'error'); }
              };

              const renderSelect = (field: 'economy' | 'reasoning', options: string[], icon: string, iconColor: string) => {
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
                    >
                      <option value="">{t('config.defaultModel')} ({options[0]})</option>
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
                        className="flex-row gap-1 text-2xs" style={{ color: 'rgba(100,180,255,0.5)', textDecoration: 'none' }}
                        title={t('config.viewModels')}
                      >
                        <ExternalLink size={8} /> {t('config.viewModels')}
                      </a>
                    )}
                  </div>
                  <div className="flex-row gap-5">
                    {renderSelect('economy', models.economy, '\u26A1', 'rgba(52,211,153,0.6)')}
                    {renderSelect('reasoning', models.reasoning, '\uD83E\uDDE0', 'rgba(245,158,11,0.6)')}
                  </div>
                </div>
              );
            })()}

          </div>
          );
        })}
        {/* Best practices links */}
        <div className="set-best-practices">
          <div className="flex-row gap-3 text-sm font-semibold mb-4" style={{ color: 'rgba(200,255,0,0.6)' }}>
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
    </div>
  );
}
