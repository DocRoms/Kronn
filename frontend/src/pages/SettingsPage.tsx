import { useState } from 'react';
import { config as configApi, agents as agentsApi, stats as statsApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import { useT } from '../lib/I18nContext';
import { UI_LOCALES } from '../lib/i18n';
import { AGENT_COLORS } from '../lib/constants';
import type { AgentDetection, AgentsConfig } from '../types/generated';
import {
  MessageSquare, Cpu, Zap, Key, AlertTriangle, Save,
  HardDrive, Plus, Trash2, Download, Upload, Check,
  Loader2, RefreshCw, X, Eye, EyeOff, Play, StopCircle,
  ExternalLink, ChevronRight,
} from 'lucide-react';

/** Output languages for agents (sent to backend, not related to UI i18n) */
const LANGUAGES: { code: string; label: string; flag: string }[] = [
  { code: 'fr', label: 'Français', flag: 'FR' },
  { code: 'en', label: 'English', flag: 'EN' },
  { code: 'es', label: 'Español', flag: 'ES' },
  { code: 'zh', label: '中文', flag: 'ZH' },
  { code: 'br', label: 'Brezhoneg', flag: 'BR' },
];

interface SettingsPageProps {
  agents: AgentDetection[];
  agentAccess: AgentsConfig | null;
  configLanguage: string | null;
  refetchAgents: () => void;
  refetchAgentAccess: () => void;
  refetchLanguage: () => void;
  refetchProjects: () => void;
  refetchDiscussions: () => void;
  onReset: () => void;
}

export function SettingsPage({
  agents,
  agentAccess,
  configLanguage,
  refetchAgents,
  refetchAgentAccess,
  refetchLanguage,
  refetchProjects,
  refetchDiscussions,
  onReset,
}: SettingsPageProps) {
  const { t, locale, setLocale } = useT();

  // Internal state
  const [installing, setInstalling] = useState<string | null>(null);
  const [newKeyInputs, setNewKeyInputs] = useState<Record<string, { name: string; value: string }>>({});
  const [addingKeyFor, setAddingKeyFor] = useState<string | null>(null);
  const [tokenVisible, setTokenVisible] = useState<Set<string>>(new Set());
  const [usageExpanded, setUsageExpanded] = useState<string | null>(null);
  const [usageSearch, setUsageSearch] = useState('');

  // Internal API calls
  const { data: tokenConfig, refetch: refetchTokens } = useApi(() => configApi.getTokens(), []);
  const { data: dbInfo, refetch: refetchDbInfo } = useApi(() => configApi.dbInfo(), []);
  const { data: agentUsageData, refetch: refetchAgentUsage } = useApi(() => statsApi.agentUsage(), []);

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
      <h1 style={ss.h1}>Configuration</h1>
      <p style={{ ...ss.meta, marginBottom: 20 }}>{t('config.subtitle')}</p>

      {/* UI Language */}
      <div style={ss.card(false)}>
        <div style={{ padding: '16px 20px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
            <MessageSquare size={14} style={{ color: '#c8ff00' }} />
            <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.uiLanguage')}</span>
          </div>
          <p style={{ color: 'rgba(255,255,255,0.35)', fontSize: 12, marginBottom: 12 }}>
            {t('config.uiLanguageHint')}
          </p>
          <div style={{ display: 'flex', gap: 8 }}>
            {UI_LOCALES.map(l => (
              <button
                key={l.code}
                style={{
                  padding: '8px 16px', borderRadius: 6, cursor: 'pointer',
                  fontFamily: 'inherit', fontSize: 12, fontWeight: 600,
                  background: locale === l.code ? 'rgba(200,255,0,0.15)' : 'rgba(255,255,255,0.04)',
                  color: locale === l.code ? '#c8ff00' : 'rgba(255,255,255,0.4)',
                  border: locale === l.code ? '1px solid rgba(200,255,0,0.3)' : '1px solid rgba(255,255,255,0.08)',
                }}
                onClick={() => setLocale(l.code)}
              >
                {l.flag} {l.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Output Language */}
      <div style={ss.card(false)}>
        <div style={{ padding: '16px 20px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
            <MessageSquare size={14} style={{ color: '#c8ff00' }} />
            <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.outputLanguage')}</span>
          </div>
          <p style={{ color: 'rgba(255,255,255,0.35)', fontSize: 12, marginBottom: 12 }}>
            {t('config.outputLanguageHint')}
          </p>
          <div style={{ display: 'flex', gap: 8 }}>
            {LANGUAGES.map(l => (
              <button
                key={l.code}
                style={{
                  padding: '8px 16px', borderRadius: 6, cursor: 'pointer',
                  fontFamily: 'inherit', fontSize: 12, fontWeight: 600,
                  background: (configLanguage ?? 'fr') === l.code ? 'rgba(200,255,0,0.15)' : 'rgba(255,255,255,0.04)',
                  color: (configLanguage ?? 'fr') === l.code ? '#c8ff00' : 'rgba(255,255,255,0.4)',
                  border: (configLanguage ?? 'fr') === l.code ? '1px solid rgba(200,255,0,0.3)' : '1px solid rgba(255,255,255,0.08)',
                }}
                onClick={async () => {
                  await configApi.saveLanguage(l.code);
                  refetchLanguage();
                }}
              >
                {l.flag} {l.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Agents */}
      <div style={ss.card(false)}>
        <div style={{ padding: '16px 20px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12 }}>
            <Cpu size={14} style={{ color: '#c8ff00' }} />
            <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.agents')}</span>
            <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.3)', marginLeft: 'auto' }}>
              {agents.filter(a => a.installed || a.runtime_available).length}/{agents.length} {agents.filter(a => a.installed || a.runtime_available).length > 1 ? t('config.installedPlural') : t('config.installed')}
            </span>
            <button style={ss.iconBtn} onClick={() => refetchAgents()} title={t('config.refresh')}>
              <RefreshCw size={12} />
            </button>
          </div>

          {(() => {
            const isWSL = agents.some(a => a.host_label === 'WSL');
            const hasDockerAgent = agents.some(a => a.installed && !a.host_managed);
            return isWSL && hasDockerAgent ? (
              <div style={{ padding: '8px 12px', marginBottom: 8, borderRadius: 6, background: 'rgba(255,180,0,0.06)', border: '1px solid rgba(255,180,0,0.15)', display: 'flex', alignItems: 'flex-start', gap: 8 }}>
                <AlertTriangle size={12} style={{ color: '#ffb400', flexShrink: 0, marginTop: 2 }} />
                <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.5)', lineHeight: 1.4 }}>{t('config.wslWarning')}</span>
              </div>
            ) : null;
          })()}

          {agents.map(agent => {
            const permFlag: Record<string, { flag: string; descKey: string }> = {
              ClaudeCode: { flag: '--dangerously-skip-permissions', descKey: 'config.fullAccess' },
              Codex: { flag: '--full-auto', descKey: 'config.autoApply' },
              GeminiCli: { flag: '--yolo', descKey: 'config.fullAccess' },
            };
            const perm = permFlag[agent.agent_type];
            const tokenField: Record<string, { key: 'anthropic' | 'openai' | 'google'; hint: string; url: string }> = {
              ClaudeCode: { key: 'anthropic', hint: 'ANTHROPIC_API_KEY', url: 'https://console.anthropic.com/settings/keys' },
              Codex: { key: 'openai', hint: 'OPENAI_API_KEY', url: 'https://platform.openai.com/api-keys' },
              GeminiCli: { key: 'google', hint: 'GEMINI_API_KEY', url: 'https://aistudio.google.com/apikey' },
            };
            const tf = tokenField[agent.agent_type];
            const isFullAccess = agent.agent_type === 'ClaudeCode'
              ? agentAccess?.claude_code?.full_access ?? false
              : agent.agent_type === 'Codex'
                ? agentAccess?.codex?.full_access ?? false
                : agent.agent_type === 'GeminiCli'
                  ? agentAccess?.gemini_cli?.full_access ?? false
                  : false;

            return (
            <div key={agent.name} style={{ padding: '10px 0', borderTop: '1px solid rgba(255,255,255,0.05)' }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                <div style={ss.dot((agent.installed || agent.runtime_available) && agent.enabled)} />
                <div style={{ flex: 1 }}>
                  <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                    <span style={{ fontWeight: 600, fontSize: 12 }}>{agent.name}</span>
                    <span style={ss.originBadge}>{agent.origin}</span>
                    {agent.version && <code style={{ ...ss.code, fontSize: 10 }}>v{agent.version}</code>}
                    {agent.latest_version && agent.latest_version !== agent.version && (
                      <span style={ss.updateBadge}>&#x2B06; {agent.latest_version}</span>
                    )}
                  </div>
                  {!agent.installed && !agent.runtime_available && (
                    <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.25)', marginTop: 2 }}>
                      <code style={ss.code}>{agent.install_command}</code>
                    </div>
                  )}
                  {!agent.installed && agent.runtime_available && (
                    <div style={{ fontSize: 10, color: 'rgba(52,211,153,0.5)', marginTop: 2 }}>
                      runtime OK <span style={{ color: 'rgba(255,255,255,0.2)' }}>— via npx</span>
                    </div>
                  )}
                </div>
                {(agent.installed || agent.runtime_available) ? (
                  <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                    <button
                      style={{
                        ...ss.iconBtn,
                        fontSize: 10,
                        padding: '2px 8px',
                        borderRadius: 4,
                        background: agent.enabled ? 'rgba(200,255,0,0.1)' : 'rgba(255,100,0,0.1)',
                        border: agent.enabled ? '1px solid rgba(200,255,0,0.2)' : '1px solid rgba(255,100,0,0.2)',
                        color: agent.enabled ? '#c8ff00' : '#ff8c00',
                      }}
                      title={agent.enabled ? t('config.toggleDisable') : t('config.toggleEnable')}
                      onClick={async () => {
                        try {
                          await agentsApi.toggle(agent.agent_type);
                        } catch { /* ignore */ }
                        refetchAgents();
                      }}
                      disabled={installing !== null}
                    >
                      {agent.enabled ? t('config.enabled') : t('config.disabled')}
                    </button>
                    {agent.host_managed && (
                      <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.25)', marginLeft: 2 }} title={t('config.hostManaged')}>{agent.host_label ?? 'host'}</span>
                    )}
                    <button
                      style={{ ...ss.iconBtn, color: 'rgba(255,255,255,0.2)' }}
                      title={t('config.uninstall')}
                      onClick={async () => {
                        if (!confirm(t('config.uninstallConfirm', agent.name))) return;
                        setInstalling(agent.name);
                        try {
                          await agentsApi.uninstall(agent.agent_type);
                        } catch {
                          alert(t('config.uninstallFailed'));
                          setInstalling(null);
                          return;
                        }
                        // Re-detect: if agent is still installed, uninstall had no effect
                        const updated = await agentsApi.detect();
                        const still = updated?.find((a: AgentDetection) => a.agent_type === agent.agent_type);
                        if (still?.installed) {
                          alert(t('config.uninstallFailed'));
                        }
                        refetchAgents();
                        setInstalling(null);
                      }}
                      disabled={installing !== null}
                    >
                      <Trash2 size={11} />
                    </button>
                  </div>
                ) : (
                  <button
                    style={{ ...ss.installBtn, padding: '4px 10px', fontSize: 11 }}
                    onClick={() => handleInstallAgent(agent)}
                    disabled={installing !== null}
                  >
                    {installing === agent.name ? (
                      <><Loader2 size={10} style={{ animation: 'spin 1s linear infinite' }} /> ...</>
                    ) : (
                      <><Download size={10} /> Installer</>
                    )}
                  </button>
                )}
              </div>
              {perm && (agent.installed || agent.runtime_available) && (
                <div style={{ marginLeft: 22, marginTop: 8, padding: '8px 12px', borderRadius: 6, background: 'rgba(255,255,255,0.02)', border: '1px solid rgba(255,255,255,0.05)' }}>
                  <div
                    style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer' }}
                    onClick={async () => {
                      await configApi.setAgentAccess({ agent: agent.agent_type, full_access: !isFullAccess });
                      refetchAgentAccess();
                    }}
                  >
                    <div style={{
                      width: 30, height: 16, borderRadius: 8, position: 'relative' as const, transition: 'background 0.2s',
                      background: isFullAccess ? 'rgba(200,255,0,0.3)' : 'rgba(255,255,255,0.1)',
                      border: isFullAccess ? '1px solid rgba(200,255,0,0.4)' : '1px solid rgba(255,255,255,0.15)',
                    }}>
                      <div style={{
                        width: 12, height: 12, borderRadius: '50%', position: 'absolute' as const, top: 1, transition: 'left 0.2s',
                        left: isFullAccess ? 16 : 1,
                        background: isFullAccess ? '#c8ff00' : 'rgba(255,255,255,0.3)',
                      }} />
                    </div>
                    <code style={{ fontSize: 10, color: isFullAccess ? '#c8ff00' : 'rgba(255,255,255,0.4)' }}>{perm.flag}</code>
                  </div>
                  <p style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', marginTop: 4, marginLeft: 38 }}>
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
                <div style={{ marginLeft: 22, marginTop: 6 }}>
                  {/* Provider-level override toggle */}
                  {providerKeys.length > 0 && (
                    <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 4 }}>
                      <Key size={10} style={{ color: 'rgba(255,255,255,0.25)', flexShrink: 0 }} />
                      <button
                        style={{ ...ss.iconBtn, padding: 0 }}
                        title={isDisabled ? t('config.enableOverride') : t('config.disableOverride')}
                        onClick={async () => {
                          await configApi.toggleTokenOverride(tf.key);
                          refetchTokens();
                        }}
                      >
                        {isDisabled
                          ? <Play size={10} style={{ color: 'rgba(255,255,255,0.25)' }} />
                          : <StopCircle size={10} style={{ color: 'rgba(52,211,153,0.5)' }} />}
                      </button>
                      <span style={{ fontSize: 10, color: isDisabled ? 'rgba(255,255,255,0.25)' : 'rgba(52,211,153,0.6)' }}>
                        {isDisabled ? t('config.overrideDisabled') : t('config.overrideActive')}
                      </span>
                      <a
                        href={tf.url} target="_blank" rel="noopener noreferrer"
                        style={{ display: 'flex', alignItems: 'center', color: 'rgba(255,255,255,0.25)', flexShrink: 0, marginLeft: 'auto' }}
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
                    <div key={k.id} style={{
                      display: 'flex', alignItems: 'center', gap: 6, padding: '2px 0 2px 16px',
                      opacity: isDisabled ? 0.4 : 1,
                    }}>
                      {/* Active indicator / activate button */}
                      {k.active ? (
                        <Check size={9} style={{ color: 'rgba(52,211,153,0.7)', flexShrink: 0 }} />
                      ) : (
                        <button style={{ ...ss.iconBtn, padding: 0 }} title={t('config.activateKey')}
                          onClick={async () => { await configApi.activateApiKey(k.id); refetchTokens(); }}>
                          <div style={{ width: 9, height: 9, borderRadius: '50%', border: '1px solid rgba(255,255,255,0.2)' }} />
                        </button>
                      )}
                      <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.5)', minWidth: 60, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                        {k.name}
                      </span>
                      <span style={{
                        fontSize: 10, padding: '1px 6px', borderRadius: 4, fontFamily: isVis ? 'monospace' : 'inherit',
                        background: isDisabled ? 'rgba(255,255,255,0.04)' : 'rgba(52,211,153,0.1)',
                        color: isDisabled ? 'rgba(255,255,255,0.25)' : 'rgba(52,211,153,0.7)',
                        textDecoration: isDisabled ? 'line-through' : 'none',
                      }}>
                        {isVis ? k.masked_value : k.masked_value.replace(/[^.]/g, '\u2022')}
                      </span>
                      <button style={{ ...ss.iconBtn, padding: 0 }} title={isVis ? 'Hide' : 'Show'}
                        onClick={() => setTokenVisible(prev => {
                          const next = new Set(prev);
                          if (next.has(k.id)) next.delete(k.id); else next.add(k.id);
                          return next;
                        })}>
                        {isVis ? <EyeOff size={9} style={{ color: '#c8ff00' }} /> : <Eye size={9} style={{ color: 'rgba(255,255,255,0.25)' }} />}
                      </button>
                      <button style={{ ...ss.iconBtn, padding: 0 }} title={t('config.deleteKey')}
                        onClick={async () => {
                          if (confirm(t('config.deleteKeyConfirm').replace('{0}', k.name))) {
                            await configApi.deleteApiKey(k.id);
                            refetchTokens();
                          }
                        }}>
                        <Trash2 size={9} style={{ color: 'rgba(255,107,107,0.5)' }} />
                      </button>
                    </div>
                    );
                  })}

                  {/* No keys yet */}
                  {providerKeys.length === 0 && !isAdding && (
                    <div style={{ display: 'flex', alignItems: 'center', gap: 8, paddingLeft: 16 }}>
                      <Key size={10} style={{ color: 'rgba(255,255,255,0.25)', flexShrink: 0 }} />
                      <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.25)' }}>{t('config.localAuth')}</span>
                      <a href={tf.url} target="_blank" rel="noopener noreferrer"
                        style={{ display: 'flex', alignItems: 'center', color: 'rgba(255,255,255,0.25)', flexShrink: 0 }}
                        title={t('config.getKey')}>
                        <ExternalLink size={10} />
                      </a>
                    </div>
                  )}

                  {/* Add key button / form */}
                  {isAdding ? (
                    <div style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '4px 0 2px 16px' }}>
                      <input
                        type="text"
                        style={{ ...ss.input, fontSize: 10, padding: '3px 6px', width: 100 }}
                        placeholder={t('config.keyName')}
                        value={newInput.name}
                        onChange={e => setNewKeyInputs(prev => ({ ...prev, [tf.key]: { ...newInput, name: e.target.value } }))}
                      />
                      <input
                        type="password"
                        style={{ ...ss.input, flex: 1, fontSize: 10, padding: '3px 6px', maxWidth: 180 }}
                        placeholder={tf.hint}
                        value={newInput.value}
                        onChange={e => setNewKeyInputs(prev => ({ ...prev, [tf.key]: { ...newInput, value: e.target.value } }))}
                      />
                      {newInput.value && (
                        <button style={{ ...ss.iconBtn, fontSize: 10, color: '#c8ff00' }}
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
                                  alert(t('config.syncTokensDone').replace('{0}', synced.join(', ')));
                                } else {
                                  alert(t('config.syncTokensNone'));
                                }
                              }
                            } catch { /* done */ }
                          }}>
                          <Save size={10} />
                        </button>
                      )}
                      <button style={{ ...ss.iconBtn, padding: 0 }} onClick={() => setAddingKeyFor(null)}>
                        <X size={10} style={{ color: 'rgba(255,255,255,0.3)' }} />
                      </button>
                    </div>
                  ) : (
                    <button
                      style={{ ...ss.iconBtn, fontSize: 10, color: 'rgba(255,255,255,0.3)', padding: '2px 0 0 16px', display: 'flex', alignItems: 'center', gap: 4 }}
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
            </div>
            );
          })}
        </div>
      </div>

      {/* Token Usage per Agent */}
      {agentUsageData && agentUsageData.length > 0 && (
      <div style={ss.card(false)}>
        <div style={{ padding: '16px 20px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12 }}>
            <Zap size={14} style={{ color: '#c8ff00' }} />
            <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.tokenUsage')}</span>
            <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.3)', marginLeft: 'auto' }}>
              {agentUsageData.reduce((s, a) => s + a.total_tokens, 0).toLocaleString()} tokens
            </span>
            <button style={ss.iconBtn} onClick={() => refetchAgentUsage()} title={t('config.refresh')}>
              <RefreshCw size={12} />
            </button>
          </div>

          {agentUsageData.map(agent => {
            const isExpanded = usageExpanded === agent.agent_type;
            const color = AGENT_COLORS[agent.agent_type] ?? '#8b5cf6';
            const filteredProjects = isExpanded
              ? agent.by_project.filter(p => !usageSearch || p.project_name.toLowerCase().includes(usageSearch.toLowerCase()))
              : [];

            return (
            <div key={agent.agent_type} style={{ marginBottom: 8 }}>
              <div
                style={{
                  display: 'flex', alignItems: 'center', gap: 8, padding: '8px 12px', cursor: 'pointer',
                  borderRadius: 6, background: isExpanded ? 'rgba(255,255,255,0.04)' : 'transparent',
                }}
                onClick={() => setUsageExpanded(isExpanded ? null : agent.agent_type)}
              >
                <ChevronRight size={12} style={{ color: 'rgba(255,255,255,0.3)', transform: isExpanded ? 'rotate(90deg)' : 'none', transition: 'transform .15s' }} />
                <Cpu size={12} style={{ color }} />
                <span style={{ fontWeight: 600, fontSize: 12, color }}>{agent.agent_type}</span>
                <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginLeft: 'auto' }}>
                  {agent.total_tokens.toLocaleString()} tok
                </span>
                <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.2)' }}>
                  {agent.message_count} msg
                </span>
              </div>

              {isExpanded && (
                <div style={{ paddingLeft: 32, paddingRight: 12, paddingBottom: 8 }}>
                  {agent.by_project.length > 5 && (
                    <input
                      type="text"
                      placeholder={t('projects.search')}
                      value={usageSearch}
                      onChange={e => setUsageSearch(e.target.value)}
                      style={{ ...ss.input, fontSize: 10, padding: '4px 8px', marginBottom: 6, width: '100%' }}
                    />
                  )}
                  {filteredProjects.map(p => (
                    <div key={p.project_id} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '3px 0', fontSize: 11 }}>
                      <span style={{ color: 'rgba(255,255,255,0.6)', flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                        {p.project_name}
                      </span>
                      <span style={{ color: 'rgba(255,255,255,0.3)', fontSize: 10, flexShrink: 0 }}>
                        {p.tokens_used.toLocaleString()} tok
                      </span>
                      <span style={{ color: 'rgba(255,255,255,0.15)', fontSize: 9, flexShrink: 0 }}>
                        {p.message_count} msg
                      </span>
                    </div>
                  ))}
                  {filteredProjects.length === 0 && usageSearch && (
                    <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.2)', padding: '4px 0' }}>
                      {t('projects.noResult')}
                    </div>
                  )}
                </div>
              )}
            </div>
            );
          })}
        </div>
      </div>
      )}

      {/* Database */}
      <div style={ss.card(false)}>
        <div style={{ padding: '16px 20px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12 }}>
            <HardDrive size={14} style={{ color: '#c8ff00' }} />
            <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.database')}</span>
            {dbInfo && (
              <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.3)', marginLeft: 'auto' }}>
                {dbInfo.size_bytes < 1024 * 1024
                  ? `${(dbInfo.size_bytes / 1024).toFixed(1)} Ko`
                  : `${(dbInfo.size_bytes / (1024 * 1024)).toFixed(1)} Mo`}
              </span>
            )}
          </div>

          {dbInfo && (
            <div style={{ display: 'flex', gap: 16, marginBottom: 16 }}>
              {[
                { label: t('config.dbProjects'), value: dbInfo.project_count },
                { label: t('config.dbDiscussions'), value: dbInfo.discussion_count },
                { label: t('config.dbMessages'), value: dbInfo.message_count },
                { label: t('config.dbMcps'), value: dbInfo.mcp_count },
                { label: t('config.dbTasks'), value: dbInfo.task_count },
              ].map(({ label, value }) => (
                <div key={label} style={{ textAlign: 'center' }}>
                  <div style={{ fontSize: 18, fontWeight: 700, color: '#c8ff00' }}>{value}</div>
                  <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)', marginTop: 2 }}>{label}</div>
                </div>
              ))}
            </div>
          )}

          <div style={{ display: 'flex', gap: 8 }}>
            <button
              style={ss.scanBtn}
              onClick={async () => {
                const data = await configApi.exportData();
                if (!data) return;
                const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
                const url = URL.createObjectURL(blob);
                const a = document.createElement('a');
                a.href = url;
                a.download = `kronn-export-${new Date().toISOString().slice(0, 10)}.json`;
                a.click();
                URL.revokeObjectURL(url);
              }}
            >
              <Download size={12} /> {t('config.export')}
            </button>
            <button
              style={ss.scanBtn}
              onClick={() => {
                const input = document.createElement('input');
                input.type = 'file';
                input.accept = '.json';
                input.onchange = async () => {
                  const file = input.files?.[0];
                  if (!file) return;
                  try {
                    const text = await file.text();
                    const data = JSON.parse(text);
                    if (!data.version || !data.projects || !data.discussions) {
                      alert(t('config.importInvalid'));
                      return;
                    }
                    if (!confirm(t('config.importConfirm'))) return;
                    await configApi.importData(data);
                    refetchProjects();
                    refetchDiscussions();
                    refetchDbInfo();
                  } catch {
                    alert(t('config.importError'));
                  }
                };
                input.click();
              }}
            >
              <Upload size={12} /> {t('config.import')}
            </button>
          </div>
        </div>
      </div>

      {/* General */}
      <div style={{ ...ss.card(false), marginTop: 16 }}>
        <div style={{ padding: '16px 20px' }}>
          <p style={{ color: 'rgba(255,255,255,0.4)', fontSize: 13, marginBottom: 16 }}>
            {t('config.configFile')} : <code style={ss.code}>~/.config/kronn/config.toml</code>
          </p>
          <div style={{ borderTop: '1px solid rgba(255,255,255,0.05)', paddingTop: 16 }}>
            <p style={{ color: 'rgba(255,255,255,0.35)', fontSize: 12, marginBottom: 12 }}>
              {t('config.resetHint')}
            </p>
            <button style={ss.dangerBtn} onClick={onReset}>
              <Trash2 size={12} /> {t('config.reset')}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

// ─── Styles (copied from Dashboard, only what SettingsPage needs) ────────────

const ss = {
  h1: { fontSize: 20, fontWeight: 700, letterSpacing: '-0.02em' } as const,
  meta: { color: 'rgba(255,255,255,0.4)', fontSize: 12, marginTop: 4 } as const,
  card: (active: boolean) => ({ background: '#12151c', border: `1px solid ${active ? 'rgba(255,255,255,0.12)' : 'rgba(255,255,255,0.07)'}`, borderRadius: 10, marginBottom: 12, transition: 'border-color 0.2s' } as const),
  iconBtn: { background: 'none', border: '1px solid rgba(255,255,255,0.08)', borderRadius: 4, padding: '4px 8px', color: 'rgba(255,255,255,0.5)', cursor: 'pointer', display: 'flex', alignItems: 'center', fontSize: 11 } as const,
  dot: (on: boolean) => ({ width: 7, height: 7, borderRadius: '50%', background: on ? '#34d399' : 'rgba(255,255,255,0.15)', boxShadow: on ? '0 0 6px rgba(52,211,153,0.4)' : 'none', flexShrink: 0 } as const),
  originBadge: { fontSize: 10, fontWeight: 600, padding: '1px 6px', borderRadius: 4, background: 'rgba(100,180,255,0.1)', color: 'rgba(100,180,255,0.7)', border: '1px solid rgba(100,180,255,0.15)' } as const,
  code: { fontSize: 11, fontFamily: 'JetBrains Mono, monospace', background: 'rgba(255,255,255,0.06)', padding: '2px 6px', borderRadius: 4 } as const,
  updateBadge: { fontSize: 10, fontWeight: 600, padding: '1px 6px', borderRadius: 4, background: 'rgba(255,200,0,0.1)', color: '#ffc800', marginLeft: 6 } as const,
  installBtn: { padding: '6px 14px', background: 'rgba(200,255,0,0.1)', color: '#c8ff00', border: '1px solid rgba(200,255,0,0.2)', borderRadius: 6, fontSize: 12, cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6, fontFamily: 'inherit' } as const,
  input: { width: '100%', padding: '8px 12px', background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.08)', borderRadius: 6, color: '#e8eaed', fontSize: 12, fontFamily: 'inherit', outline: 'none' } as const,
  scanBtn: { padding: '7px 14px', borderRadius: 6, border: '1px solid rgba(200,255,0,0.2)', background: 'rgba(200,255,0,0.05)', color: '#c8ff00', cursor: 'pointer', fontSize: 12, fontFamily: 'inherit', fontWeight: 500, display: 'flex', alignItems: 'center', gap: 6 } as const,
  dangerBtn: { background: 'rgba(255,77,106,0.08)', border: '1px solid rgba(255,77,106,0.2)', borderRadius: 6, padding: '6px 14px', color: '#ff4d6a', cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6, fontSize: 12, fontFamily: 'inherit' } as const,
};
