import { useState, useEffect } from 'react';
import { config as configApi, agents as agentsApi, stats as statsApi, skills as skillsApi, projects as projectsApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import { useT } from '../lib/I18nContext';
import { UI_LOCALES } from '../lib/i18n';
import { AGENT_COLORS } from '../lib/constants';
import type { AgentDetection, AgentsConfig, Skill, Project } from '../types/generated';
import type { ToastFn } from '../hooks/useToast';
import {
  MessageSquare, Cpu, Zap, Key, AlertTriangle, Save,
  HardDrive, Plus, Trash2, Download, Upload, Check,
  Loader2, RefreshCw, X, Eye, EyeOff, Play, StopCircle,
  ExternalLink, ChevronRight, Layers, FolderSearch, Filter,
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
  projects: Project[];
  refetchAgents: () => void;
  refetchAgentAccess: () => void;
  refetchLanguage: () => void;
  refetchProjects: () => void;
  refetchDiscussions: () => void;
  onReset: () => void;
  toast: ToastFn;
}

export function SettingsPage({
  agents,
  agentAccess,
  configLanguage,
  projects,
  refetchAgents,
  refetchAgentAccess,
  refetchLanguage,
  refetchProjects,
  refetchDiscussions,
  onReset,
  toast,
}: SettingsPageProps) {
  const { t, locale, setLocale } = useT();

  // Internal state
  const [installing, setInstalling] = useState<string | null>(null);
  const [newKeyInputs, setNewKeyInputs] = useState<Record<string, { name: string; value: string }>>({});
  const [addingKeyFor, setAddingKeyFor] = useState<string | null>(null);
  const [tokenVisible, setTokenVisible] = useState<Set<string>>(new Set());
  const [usageExpanded, setUsageExpanded] = useState<string | null>(null);
  const [usageSearch, setUsageSearch] = useState('');
  const [scanDepth, setScanDepth] = useState(4);
  const [scanPaths, setScanPaths] = useState<string[]>([]);
  const [scanIgnore, setScanIgnore] = useState<string[]>([]);
  const [newScanPath, setNewScanPath] = useState('');
  const [newIgnorePattern, setNewIgnorePattern] = useState('');
  const [availableSkills, setAvailableSkills] = useState<Skill[]>([]);
  const [showCreateSkill, setShowCreateSkill] = useState(false);
  const [newSkillName, setNewSkillName] = useState('');
  const [newSkillDesc, setNewSkillDesc] = useState('');
  const [newSkillIcon, setNewSkillIcon] = useState('Star');
  const [newSkillCategory, setNewSkillCategory] = useState<'Technical' | 'Business' | 'Meta'>('Technical');
  const [newSkillContent, setNewSkillContent] = useState('');
  const [projectSkillsExpanded, setProjectSkillsExpanded] = useState<string | null>(null);

  // Internal API calls
  const { data: tokenConfig, refetch: refetchTokens } = useApi(() => configApi.getTokens(), []);
  const { data: dbInfo, refetch: refetchDbInfo } = useApi(() => configApi.dbInfo(), []);
  useApi(() => configApi.getScanDepth().then(d => { if (d != null) setScanDepth(d); return d; }), []);
  useApi(() => configApi.getScanPaths().then(p => { if (p) setScanPaths(p); return p; }), []);
  useApi(() => configApi.getScanIgnore().then(p => { if (p) setScanIgnore(p); return p; }), []);
  const { data: agentUsageData } = useApi(() => statsApi.agentUsage(), []);

  useEffect(() => {
    skillsApi.list().then(setAvailableSkills).catch(() => {});
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
                  try { await configApi.saveLanguage(l.code); refetchLanguage(); }
                  catch { console.error('Failed to save language'); }
                }}
              >
                {l.flag} {l.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Scan (depth + paths + ignore) */}
      <div style={ss.card(false)}>
        <div style={{ padding: '16px 20px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12 }}>
            <FolderSearch size={14} style={{ color: '#c8ff00' }} />
            <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.scanPaths')}</span>
            <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.3)', marginLeft: 'auto' }}>
              {scanPaths.length} {scanPaths.length > 1 ? t('config.pathsPlural') : t('config.path')}
            </span>
          </div>

          {/* Scan Depth */}
          <div style={{ marginBottom: 16 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6 }}>
              <Layers size={12} style={{ color: 'rgba(255,255,255,0.4)' }} />
              <span style={{ fontWeight: 600, fontSize: 12, color: 'rgba(255,255,255,0.6)' }}>{t('config.scanDepth')}</span>
              <span style={{ fontSize: 12, color: '#c8ff00', marginLeft: 'auto', fontWeight: 600, fontVariantNumeric: 'tabular-nums' }}>
                {scanDepth}
              </span>
            </div>
            <p style={{ color: 'rgba(255,255,255,0.35)', fontSize: 11, marginBottom: 8 }}>
              {t('config.scanDepthHint')}
            </p>
            <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
              <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.3)' }}>2</span>
              <input
                type="range"
                min={2}
                max={10}
                value={scanDepth}
                onChange={async (e) => {
                  const v = Number(e.target.value);
                  setScanDepth(v);
                  try { await configApi.setScanDepth(v); }
                  catch { console.error('Failed to save scan depth'); }
                }}
                style={{ flex: 1, accentColor: '#c8ff00', cursor: 'pointer' }}
              />
              <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.3)' }}>10</span>
            </div>
          </div>

          {/* Scan Paths */}
          <div style={{ marginBottom: 16, paddingTop: 12, borderTop: '1px solid rgba(255,255,255,0.05)' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6 }}>
              <FolderSearch size={12} style={{ color: 'rgba(255,255,255,0.4)' }} />
              <span style={{ fontWeight: 600, fontSize: 12, color: 'rgba(255,255,255,0.6)' }}>{t('config.scanPaths')}</span>
            </div>
            <p style={{ color: 'rgba(255,255,255,0.35)', fontSize: 11, marginBottom: 8 }}>
              {t('config.scanPathsHint')}
            </p>
            {scanPaths.map((p, i) => (
              <div key={i} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '4px 0' }}>
                <code style={{ ...ss.code, fontSize: 11, flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{p}</code>
                <button
                  style={{ ...ss.iconBtn, padding: '2px 4px' }}
                  onClick={async () => {
                    const updated = scanPaths.filter((_, j) => j !== i);
                    setScanPaths(updated);
                    try { await configApi.setScanPaths(updated); } catch (err) { console.error(err); }
                  }}
                >
                  <Trash2 size={10} style={{ color: 'rgba(255,107,107,0.5)' }} />
                </button>
              </div>
            ))}
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginTop: 8 }}>
              <input
                type="text"
                style={{ ...ss.input, fontSize: 11, padding: '5px 8px', flex: 1 }}
                placeholder={t('config.scanPathPlaceholder')}
                value={newScanPath}
                onChange={e => setNewScanPath(e.target.value)}
                onKeyDown={async (e) => {
                  if (e.key === 'Enter' && newScanPath.trim()) {
                    if (scanPaths.includes(newScanPath.trim())) return;
                    const updated = [...scanPaths, newScanPath.trim()];
                    setScanPaths(updated);
                    setNewScanPath('');
                    try { await configApi.setScanPaths(updated); } catch (err) { console.error(err); }
                  }
                }}
              />
              <button
                style={{ ...ss.iconBtn, padding: '4px 8px', color: '#c8ff00' }}
                onClick={async () => {
                  if (!newScanPath.trim()) return;
                  if (scanPaths.includes(newScanPath.trim())) return;
                  const updated = [...scanPaths, newScanPath.trim()];
                  setScanPaths(updated);
                  setNewScanPath('');
                  try { await configApi.setScanPaths(updated); } catch (err) { console.error(err); }
                }}
              >
                <Plus size={12} />
              </button>
            </div>
          </div>

          {/* Scan Ignore */}
          <div style={{ paddingTop: 12, borderTop: '1px solid rgba(255,255,255,0.05)' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6 }}>
              <Filter size={12} style={{ color: 'rgba(255,255,255,0.4)' }} />
              <span style={{ fontWeight: 600, fontSize: 12, color: 'rgba(255,255,255,0.6)' }}>{t('config.scanIgnore')}</span>
              <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.3)', marginLeft: 'auto' }}>
                {scanIgnore.length} {scanIgnore.length > 1 ? t('config.patternsPlural') : t('config.pattern')}
              </span>
            </div>
            <p style={{ color: 'rgba(255,255,255,0.35)', fontSize: 11, marginBottom: 8 }}>
              {t('config.scanIgnoreHint')}
            </p>
            <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginBottom: 8 }}>
              {scanIgnore.map((p, i) => (
                <div key={i} style={{
                  display: 'flex', alignItems: 'center', gap: 4, padding: '3px 8px',
                  background: 'rgba(255,255,255,0.04)', border: '1px solid rgba(255,255,255,0.08)',
                  borderRadius: 4, fontSize: 11, fontFamily: 'JetBrains Mono, monospace',
                }}>
                  <span style={{ color: 'rgba(255,255,255,0.6)' }}>{p}</span>
                  <button
                    style={{ background: 'none', border: 'none', padding: 0, cursor: 'pointer', display: 'flex' }}
                    onClick={async () => {
                      const updated = scanIgnore.filter((_, j) => j !== i);
                      setScanIgnore(updated);
                      try { await configApi.setScanIgnore(updated); } catch (err) { console.error(err); }
                    }}
                  >
                    <X size={9} style={{ color: 'rgba(255,107,107,0.5)' }} />
                  </button>
                </div>
              ))}
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
              <input
                type="text"
                style={{ ...ss.input, fontSize: 11, padding: '5px 8px', flex: 1 }}
                placeholder={t('config.scanIgnorePlaceholder')}
                value={newIgnorePattern}
                onChange={e => setNewIgnorePattern(e.target.value)}
                onKeyDown={async (e) => {
                  if (e.key === 'Enter' && newIgnorePattern.trim()) {
                    const updated = [...scanIgnore, newIgnorePattern.trim()];
                    setScanIgnore(updated);
                    setNewIgnorePattern('');
                    try { await configApi.setScanIgnore(updated); } catch (err) { console.error(err); }
                  }
                }}
              />
              <button
                style={{ ...ss.iconBtn, padding: '4px 8px', color: '#c8ff00' }}
                onClick={async () => {
                  if (!newIgnorePattern.trim()) return;
                  const updated = [...scanIgnore, newIgnorePattern.trim()];
                  setScanIgnore(updated);
                  setNewIgnorePattern('');
                  try { await configApi.setScanIgnore(updated); } catch (err) { console.error(err); }
                }}
              >
                <Plus size={12} />
              </button>
            </div>
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
            <button
              style={{ ...ss.iconBtn, fontSize: 10, padding: '2px 8px', borderRadius: 4, background: 'rgba(200,255,0,0.06)', border: '1px solid rgba(200,255,0,0.15)', color: '#c8ff00', display: 'flex', alignItems: 'center', gap: 4 }}
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
                  : agent.agent_type === 'Vibe'
                    ? agentAccess?.vibe?.full_access ?? false
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
                          // Re-detect: if agent is still installed, uninstall had no effect
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
                      try { await configApi.setAgentAccess({ agent: agent.agent_type, full_access: !isFullAccess }); } catch (err) { console.error(err); }
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
                          try { await configApi.toggleTokenOverride(tf.key); } catch (err) { console.error(err); }
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
                          onClick={async () => { try { await configApi.activateApiKey(k.id); } catch (err) { console.error(err); } refetchTokens(); }}>
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
                            try { await configApi.deleteApiKey(k.id); } catch (err) { console.error(err); }
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
                                  toast(t('config.syncTokensDone').replace('{0}', synced.join(', ')), 'success');
                                } else {
                                  toast(t('config.syncTokensNone'), 'info');
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
              {/* Estimated token usage per agent */}
              {(agent.installed || agent.runtime_available) && (() => {
                const agentUsage = agentUsageData?.find(a => a.agent_type === agent.agent_type);
                if (!agentUsage || agentUsage.total_tokens === 0) return null;
                const color = AGENT_COLORS[agent.agent_type] ?? '#8b5cf6';
                const isExpanded = usageExpanded === agent.agent_type;
                const filteredProjects = isExpanded
                  ? agentUsage.by_project.filter(p => !usageSearch || p.project_name.toLowerCase().includes(usageSearch.toLowerCase()))
                  : [];
                return (
                <div style={{ marginLeft: 22, marginTop: 6 }}>
                  <div
                    style={{ display: 'flex', alignItems: 'center', gap: 6, cursor: 'pointer', padding: '4px 0' }}
                    onClick={() => setUsageExpanded(isExpanded ? null : agent.agent_type)}
                  >
                    <ChevronRight size={10} style={{ color: 'rgba(255,255,255,0.25)', transform: isExpanded ? 'rotate(90deg)' : 'none', transition: 'transform .15s' }} />
                    <Zap size={10} style={{ color: 'rgba(255,255,255,0.25)' }} />
                    <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)' }}>{t('config.estimateTokenUsage')}</span>
                    <span style={{ fontSize: 10, color, marginLeft: 'auto' }}>
                      ~{agentUsage.total_tokens.toLocaleString()} tok
                    </span>
                    <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.2)' }}>
                      {agentUsage.message_count} msg
                    </span>
                  </div>
                  {isExpanded && (
                    <div style={{ paddingLeft: 22, paddingBottom: 4 }}>
                      {agentUsage.by_project.length > 5 && (
                        <input
                          type="text"
                          placeholder={t('projects.search')}
                          value={usageSearch}
                          onChange={e => setUsageSearch(e.target.value)}
                          style={{ ...ss.input, fontSize: 10, padding: '3px 6px', marginBottom: 4, width: '100%' }}
                        />
                      )}
                      {filteredProjects.map(p => (
                        <div key={p.project_id} style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '2px 0', fontSize: 10 }}>
                          <span style={{ color: 'rgba(255,255,255,0.5)', flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                            {p.project_name}
                          </span>
                          <span style={{ color: 'rgba(255,255,255,0.25)', flexShrink: 0 }}>
                            ~{p.tokens_used.toLocaleString()} tok
                          </span>
                          <span style={{ color: 'rgba(255,255,255,0.15)', fontSize: 9, flexShrink: 0 }}>
                            {p.message_count} msg
                          </span>
                        </div>
                      ))}
                      {filteredProjects.length === 0 && usageSearch && (
                        <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.2)', padding: '2px 0' }}>
                          {t('projects.noResult')}
                        </div>
                      )}
                    </div>
                  )}
                </div>
                );
              })()}
            </div>
            );
          })}
        </div>
      </div>

      {/* Skills */}
      <div style={ss.card(false)}>
        <div style={{ padding: '16px 20px' }}>
          <h2 style={{ fontSize: 14, fontWeight: 700, color: '#e8eaed', marginBottom: 16, display: 'flex', alignItems: 'center', gap: 8 }}>
            <Zap size={16} style={{ color: '#c8ff00' }} /> {t('skills.title')}
          </h2>

          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 10, marginBottom: 16 }}>
            {availableSkills.map(skill => (
              <div key={skill.id} style={{
                padding: '10px 14px', borderRadius: 8, width: 220,
                background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.08)',
              }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 4 }}>
                  <span style={{ fontWeight: 600, fontSize: 12, color: '#e8eaed' }}>{skill.name}</span>
                  <div style={{ display: 'flex', gap: 4 }}>
                    <span style={{
                      fontSize: 9, padding: '1px 6px', borderRadius: 6, fontWeight: 600,
                      background: skill.category === 'Technical' ? 'rgba(59,130,246,0.15)' : skill.category === 'Business' ? 'rgba(16,185,129,0.15)' : 'rgba(200,255,0,0.1)',
                      color: skill.category === 'Technical' ? '#60a5fa' : skill.category === 'Business' ? '#34d399' : '#c8ff00',
                      border: `1px solid ${skill.category === 'Technical' ? 'rgba(59,130,246,0.3)' : skill.category === 'Business' ? 'rgba(16,185,129,0.3)' : 'rgba(200,255,0,0.2)'}`,
                    }}>
                      {t(`skills.${skill.category.toLowerCase()}`)}
                    </span>
                    {skill.is_builtin ? (
                      <span style={{ fontSize: 9, padding: '1px 6px', borderRadius: 6, background: 'rgba(255,255,255,0.05)', color: 'rgba(255,255,255,0.3)', border: '1px solid rgba(255,255,255,0.08)' }}>
                        {t('skills.builtin')}
                      </span>
                    ) : (
                      <span style={{ fontSize: 9, padding: '1px 6px', borderRadius: 6, background: 'rgba(139,92,246,0.1)', color: 'rgba(139,92,246,0.7)', border: '1px solid rgba(139,92,246,0.2)' }}>
                        {t('skills.custom')}
                      </span>
                    )}
                  </div>
                </div>
                <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginBottom: 6 }}>{skill.description}</div>
                {!skill.is_builtin && (
                  <button
                    style={{ ...ss.iconBtn, padding: '2px 6px', color: '#ff4d6a', borderColor: 'rgba(255,77,106,0.2)' }}
                    onClick={async () => {
                      if (!confirm(t('skills.deleteConfirm'))) return;
                      try {
                        await skillsApi.delete(skill.id);
                        setAvailableSkills(prev => prev.filter(s => s.id !== skill.id));
                        toast(t('skills.remove'), 'success');
                      } catch (err) { console.error(err); }
                    }}
                  >
                    <Trash2 size={10} />
                  </button>
                )}
              </div>
            ))}
          </div>

          {!showCreateSkill ? (
            <button
              style={{ ...ss.scanBtn, gap: 6 }}
              onClick={() => setShowCreateSkill(true)}
            >
              <Plus size={12} /> {t('skills.createCustom')}
            </button>
          ) : (
            <div style={{ padding: 16, borderRadius: 8, background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.1)' }}>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10, marginBottom: 10 }}>
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('skills.name')}</label>
                  <input style={ss.input} value={newSkillName} onChange={e => setNewSkillName(e.target.value)} placeholder="My Skill" />
                </div>
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('skills.category')}</label>
                  <select
                    style={{ ...ss.input, cursor: 'pointer' }}
                    value={newSkillCategory}
                    onChange={e => setNewSkillCategory(e.target.value as any)}
                  >
                    <option value="Technical">{t('skills.technical')}</option>
                    <option value="Business">{t('skills.business')}</option>
                    <option value="Meta">{t('skills.meta')}</option>
                  </select>
                </div>
              </div>
              <div style={{ marginBottom: 10 }}>
                <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('skills.description')}</label>
                <input style={ss.input} value={newSkillDesc} onChange={e => setNewSkillDesc(e.target.value)} placeholder="What this skill does..." />
              </div>
              <div style={{ marginBottom: 10 }}>
                <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('skills.icon')}</label>
                <input style={ss.input} value={newSkillIcon} onChange={e => setNewSkillIcon(e.target.value)} placeholder="Star, Code, Shield..." />
              </div>
              <div style={{ marginBottom: 10 }}>
                <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('skills.content')}</label>
                <textarea
                  style={{ ...ss.input, minHeight: 120, resize: 'vertical' as const, fontFamily: 'monospace', fontSize: 12, lineHeight: 1.5 }}
                  value={newSkillContent}
                  onChange={e => setNewSkillContent(e.target.value)}
                  placeholder="System prompt instructions for this skill..."
                />
              </div>
              <div style={{ display: 'flex', gap: 8 }}>
                <button
                  style={{ ...ss.scanBtn, opacity: newSkillName && newSkillContent ? 1 : 0.4 }}
                  disabled={!newSkillName || !newSkillContent}
                  onClick={async () => {
                    try {
                      const created = await skillsApi.create({
                        name: newSkillName,
                        description: newSkillDesc,
                        icon: newSkillIcon,
                        category: newSkillCategory,
                        content: newSkillContent,
                      });
                      setAvailableSkills(prev => [...prev, created]);
                      setShowCreateSkill(false);
                      setNewSkillName(''); setNewSkillDesc(''); setNewSkillIcon('Star'); setNewSkillContent('');
                      toast(t('skills.add'), 'success');
                    } catch (err) { console.error(err); }
                  }}
                >
                  <Check size={12} /> {t('skills.add')}
                </button>
                <button
                  style={ss.iconBtn}
                  onClick={() => { setShowCreateSkill(false); setNewSkillName(''); setNewSkillDesc(''); setNewSkillIcon('Star'); setNewSkillContent(''); }}
                >
                  <X size={12} />
                </button>
              </div>
            </div>
          )}

          {/* Per-project default skills */}
          {projects.length > 0 && availableSkills.length > 0 && (
            <div style={{ marginTop: 20, paddingTop: 16, borderTop: '1px solid rgba(255,255,255,0.06)' }}>
              <h3 style={{ fontSize: 12, fontWeight: 700, color: 'rgba(255,255,255,0.6)', marginBottom: 10, display: 'flex', alignItems: 'center', gap: 6 }}>
                <Layers size={12} /> {t('skills.projectDefaults')}
              </h3>
              <p style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', marginBottom: 12 }}>
                {t('skills.projectDefaultsHint')}
              </p>
              {projects.filter(p => !p.path.split('/').some(s => s.startsWith('.'))).map(project => {
                const isExpanded = projectSkillsExpanded === project.id;
                const currentSkills = project.default_skill_ids ?? [];
                return (
                  <div key={project.id} style={{ marginBottom: 4 }}>
                    <button
                      style={{
                        width: '100%', display: 'flex', alignItems: 'center', gap: 8,
                        padding: '8px 10px', borderRadius: 6, border: 'none',
                        background: isExpanded ? 'rgba(200,255,0,0.04)' : 'transparent',
                        color: '#e8eaed', cursor: 'pointer', fontFamily: 'inherit', textAlign: 'left' as const,
                      }}
                      onClick={() => setProjectSkillsExpanded(isExpanded ? null : project.id)}
                    >
                      <ChevronRight size={10} style={{ transform: isExpanded ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s' }} />
                      <span style={{ fontSize: 12, fontWeight: 500, flex: 1 }}>{project.name}</span>
                      {currentSkills.length > 0 && (
                        <span style={{ fontSize: 9, color: 'rgba(200,255,0,0.6)', fontWeight: 600 }}>
                          {currentSkills.length} skill{currentSkills.length > 1 ? 's' : ''}
                        </span>
                      )}
                    </button>
                    {isExpanded && (
                      <div style={{ padding: '8px 10px 8px 28px', display: 'flex', flexWrap: 'wrap', gap: 5 }}>
                        {availableSkills.map(skill => {
                          const selected = currentSkills.includes(skill.id);
                          return (
                            <button
                              key={skill.id}
                              type="button"
                              onClick={async () => {
                                const newIds = selected
                                  ? currentSkills.filter(id => id !== skill.id)
                                  : [...currentSkills, skill.id];
                                try { await projectsApi.setDefaultSkills(project.id, newIds); } catch (err) { console.error(err); }
                                refetchProjects();
                              }}
                              style={{
                                padding: '3px 9px', borderRadius: 10, fontSize: 10, fontFamily: 'inherit',
                                fontWeight: selected ? 600 : 400, cursor: 'pointer',
                                border: selected ? '1px solid rgba(200,255,0,0.4)' : '1px solid rgba(255,255,255,0.08)',
                                background: selected ? 'rgba(200,255,0,0.1)' : 'rgba(255,255,255,0.03)',
                                color: selected ? '#c8ff00' : 'rgba(255,255,255,0.4)',
                                display: 'flex', alignItems: 'center', gap: 3,
                                transition: 'all 0.15s',
                              }}
                              title={skill.description}
                            >
                              {selected && <Check size={8} />}
                              {skill.name}
                            </button>
                          );
                        })}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>

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
                try {
                  const data = await configApi.exportData();
                  if (!data) return;
                  const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
                  const url = URL.createObjectURL(blob);
                  const a = document.createElement('a');
                  a.href = url;
                  a.download = `kronn-export-${new Date().toISOString().slice(0, 10)}.json`;
                  a.click();
                  URL.revokeObjectURL(url);
                } catch (err) { console.error(err); }
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
                      toast(t('config.importInvalid'), 'error');
                      return;
                    }
                    if (!confirm(t('config.importConfirm'))) return;
                    await configApi.importData(data);
                    refetchProjects();
                    refetchDiscussions();
                    refetchDbInfo();
                  } catch {
                    toast(t('config.importError'), 'error');
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
