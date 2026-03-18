import { useState, useEffect } from 'react';
import { config as configApi, agents as agentsApi, stats as statsApi, skills as skillsApi, profiles as profilesApi, directives as directivesApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import { useT } from '../lib/I18nContext';
import { UI_LOCALES } from '../lib/i18n';
import { AGENT_COLORS } from '../lib/constants';
import type { AgentDetection, AgentsConfig, ModelTiersConfig, Skill, AgentProfile, Project, Directive } from '../types/generated';
import type { ToastFn } from '../hooks/useToast';
import {
  MessageSquare, Cpu, Zap, Key, AlertTriangle, Save,
  HardDrive, Plus, Trash2, Download, Upload, Check,
  Loader2, RefreshCw, X, Eye, EyeOff, Play, StopCircle,
  ExternalLink, ChevronRight, Layers, FolderSearch, Filter, UserCircle, FileText,
  Shield, Globe, Copy, Server,
} from 'lucide-react';
import { setAuthToken } from '../lib/api';

/** Output languages for agents (sent to backend, not related to UI i18n) */
const LANGUAGES: { code: string; label: string; flag: string }[] = [
  { code: 'fr', label: 'Français', flag: 'FR' },
  { code: 'en', label: 'English', flag: 'EN' },
  { code: 'es', label: 'Español', flag: 'ES' },
  { code: 'zh', label: '中文', flag: 'ZH' },
  { code: 'br', label: 'Brezhoneg', flag: 'BR' },
];

/** Provider usage dashboard URLs per agent type */
const AGENT_USAGE_URLS: Record<string, string> = {
  ClaudeCode: 'https://claude.ai/settings/usage',
  Codex: 'https://platform.openai.com/usage',
  GeminiCli: 'https://aistudio.google.com/usage',
  Vibe: 'https://console.mistral.ai/usage',
  Kiro: '',
};

interface SettingsPageProps {
  agents: AgentDetection[];
  agentAccess: AgentsConfig | null;
  configLanguage: string | null;
  projects?: Project[];
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
  const [tierEditing, setTierEditing] = useState<Record<string, { economy: string; reasoning: string }>>({});
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
  const [newSkillCategory, setNewSkillCategory] = useState<'Language' | 'Domain' | 'Business'>('Language');
  const [newSkillContent, setNewSkillContent] = useState('');
  const [availableProfiles, setAvailableProfiles] = useState<AgentProfile[]>([]);
  const [showCreateProfile, setShowCreateProfile] = useState(false);
  const [newProfileName, setNewProfileName] = useState('');
  const [newProfilePersonaName, setNewProfilePersonaName] = useState('');
  const [newProfileRole, setNewProfileRole] = useState('');
  const [newProfileAvatar, setNewProfileAvatar] = useState('🤖');
  const [newProfileColor, setNewProfileColor] = useState('#a78bfa');
  const [newProfileCategory, setNewProfileCategory] = useState<'Technical' | 'Business' | 'Meta'>('Technical');
  const [newProfilePersona, setNewProfilePersona] = useState('');
  const [expandedProfileDesc, setExpandedProfileDesc] = useState<string | null>(null);
  const [editingPersonaId, setEditingPersonaId] = useState<string | null>(null);
  const [editingPersonaValue, setEditingPersonaValue] = useState('');
  const [availableDirectives, setAvailableDirectives] = useState<Directive[]>([]);
  const [showCreateDirective, setShowCreateDirective] = useState(false);
  const [newDirectiveName, setNewDirectiveName] = useState('');
  const [newDirectiveDesc, setNewDirectiveDesc] = useState('');
  const [newDirectiveIcon, setNewDirectiveIcon] = useState('📋');
  const [newDirectiveCategory, setNewDirectiveCategory] = useState<'Output' | 'Language'>('Output');
  const [newDirectiveContent, setNewDirectiveContent] = useState('');
  const [newDirectiveConflicts, setNewDirectiveConflicts] = useState('');

  const [serverDomain, setServerDomain] = useState('');
  const [serverMaxAgents, setServerMaxAgents] = useState(5);
  const [authToken, setAuthTokenState] = useState<string | null>(null);
  const [authVisible, setAuthVisible] = useState(false);

  // Internal API calls
  const { data: tokenConfig, refetch: refetchTokens } = useApi(() => configApi.getTokens(), []);
  const { data: dbInfo, refetch: refetchDbInfo } = useApi(() => configApi.dbInfo(), []);
  useApi(() => configApi.getServerConfig().then(cfg => {
    if (cfg) { setServerDomain(cfg.domain ?? ''); setServerMaxAgents(cfg.max_concurrent_agents); }
    return cfg;
  }), []);

  // Auth token is loaded from localStorage (set when user activates auth).
  // No need to fetch from backend — it's only returned once at activation.
  useEffect(() => {
    const stored = localStorage.getItem('kronn_auth_token');
    if (stored) setAuthTokenState(stored);
  }, []);
  useApi(() => configApi.getScanDepth().then(d => { if (d != null) setScanDepth(d); return d; }), []);
  useApi(() => configApi.getScanPaths().then(p => { if (p) setScanPaths(p); return p; }), []);
  useApi(() => configApi.getScanIgnore().then(p => { if (p) setScanIgnore(p); return p; }), []);
  const { data: agentUsageData } = useApi(() => statsApi.agentUsage(), []);

  useEffect(() => {
    skillsApi.list().then(setAvailableSkills).catch(() => {});
    profilesApi.list().then(setAvailableProfiles).catch(() => {});
    directivesApi.list().then(setAvailableDirectives).catch(console.error);
    configApi.getModelTiers().then(tiers => {
      if (tiers) {
        // Initialize editing state with current values
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
            <button style={ss.iconBtn} onClick={() => refetchAgents()} title={t('config.refresh')} aria-label={t('config.refresh')}>
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
            const tokenField: Record<string, { key: string; hint: string; url: string }> = {
              ClaudeCode: { key: 'anthropic', hint: 'ANTHROPIC_API_KEY', url: 'https://console.anthropic.com/settings/keys' },
              Codex: { key: 'openai', hint: 'OPENAI_API_KEY', url: 'https://platform.openai.com/api-keys' },
              GeminiCli: { key: 'google', hint: 'GEMINI_API_KEY', url: 'https://aistudio.google.com/apikey' },
              Vibe: { key: 'mistral', hint: 'MISTRAL_API_KEY', url: 'https://console.mistral.ai/api-keys' },
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
                <div style={{ position: 'relative' as const }}>
                  <div aria-hidden="true" style={ss.dot((agent.installed || agent.runtime_available) && agent.enabled)} />
                  <span style={{ position: 'absolute', width: 1, height: 1, padding: 0, margin: -1, overflow: 'hidden', clip: 'rect(0,0,0,0)', whiteSpace: 'nowrap', border: 0 }}>
                    {(agent.installed || agent.runtime_available) && agent.enabled ? t('config.enabled') : t('config.disabled')}
                  </span>
                </div>
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
                    role="switch"
                    aria-checked={isFullAccess}
                    tabIndex={0}
                    style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer' }}
                    onClick={async () => {
                      try { await configApi.setAgentAccess({ agent: agent.agent_type, full_access: !isFullAccess }); } catch (err) { console.error(err); }
                      refetchAgentAccess();
                    }}
                    onKeyDown={async (e) => {
                      if (e.key === ' ' || e.key === 'Enter') {
                        e.preventDefault();
                        try { await configApi.setAgentAccess({ agent: agent.agent_type, full_access: !isFullAccess }); } catch (err) { console.error(err); }
                        refetchAgentAccess();
                      }
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
                        <button style={{ ...ss.iconBtn, padding: 0 }} title={t('config.activateKey')} aria-label={t('config.activateKey')}
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
                      <button style={{ ...ss.iconBtn, padding: 0 }} title={isVis ? 'Hide' : 'Show'} aria-label={isVis ? 'Hide API key' : 'Show API key'}
                        onClick={() => setTokenVisible(prev => {
                          const next = new Set(prev);
                          if (next.has(k.id)) next.delete(k.id); else next.add(k.id);
                          return next;
                        })}>
                        {isVis ? <EyeOff size={9} style={{ color: '#c8ff00' }} /> : <Eye size={9} style={{ color: 'rgba(255,255,255,0.25)' }} />}
                      </button>
                      <button style={{ ...ss.iconBtn, padding: 0 }} title={t('config.deleteKey')} aria-label={t('config.deleteKey')}
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
                        <button style={{ ...ss.iconBtn, fontSize: 10, color: '#c8ff00' }} aria-label="Save API key"
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
                      <button style={{ ...ss.iconBtn, padding: 0 }} onClick={() => setAddingKeyFor(null)} aria-label="Cancel">
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
              {/* Model tier configuration */}
              {(agent.installed || agent.runtime_available) && (() => {
                const agentKey = agent.agent_type === 'ClaudeCode' ? 'claude_code'
                  : agent.agent_type === 'Codex' ? 'codex'
                  : agent.agent_type === 'GeminiCli' ? 'gemini_cli'
                  : agent.agent_type === 'Kiro' ? 'kiro'
                  : 'vibe';
                const editing = tierEditing[agentKey];
                if (!editing) return null;

                // Known models per agent — first item is the default for that tier
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
                  };
                  try { await configApi.setModelTiers(newTiers); toast(t('config.saved'), 'success'); } catch { toast(t('config.saveError'), 'error'); }
                };

                const selectStyle = {
                  background: '#1a1d24', border: '1px solid rgba(255,255,255,0.12)',
                  borderRadius: 4, padding: '3px 6px', fontSize: 10, color: '#e8eaed',
                  fontFamily: 'inherit', width: 150, cursor: 'pointer' as const,
                  WebkitAppearance: 'none' as const, MozAppearance: 'none' as const,
                  appearance: 'none' as const,
                  backgroundImage: `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6'%3E%3Cpath d='M0 0l5 6 5-6z' fill='%23666'/%3E%3C/svg%3E")`,
                  backgroundRepeat: 'no-repeat', backgroundPosition: 'right 6px center',
                  paddingRight: 20,
                };

                const renderSelect = (field: 'economy' | 'reasoning', options: string[], icon: string, iconColor: string) => {
                  if (options.length === 0) return (
                    <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.2)', padding: '2px 6px' }}>{icon} N/A</span>
                  );
                  return (
                    <div style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
                      <span style={{ fontSize: 9, color: iconColor, width: 14 }} title={field}>{icon}</span>
                      <select
                        style={selectStyle}
                        value={editing[field]}
                        onChange={e => saveTiers(field, e.target.value)}
                      >
                        <option value="" style={{ background: '#1a1d24', color: '#e8eaed' }}>{t('config.defaultModel')} ({options[0]})</option>
                        {options.map(m => (
                          <option key={m} value={m} style={{ background: '#1a1d24', color: '#e8eaed' }}>{m}</option>
                        ))}
                      </select>
                    </div>
                  );
                };

                return (
                  <div style={{ marginLeft: 22, marginTop: 6 }}>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 4 }}>
                      <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', fontWeight: 600 }}>{t('disc.modelTier')}</span>
                      {models.modelsUrl && (
                        <a href={models.modelsUrl} target="_blank" rel="noopener noreferrer"
                          style={{ fontSize: 9, color: 'rgba(100,180,255,0.5)', display: 'flex', alignItems: 'center', gap: 2, textDecoration: 'none' }}
                          title={t('config.viewModels')}
                        >
                          <ExternalLink size={8} /> {t('config.viewModels')}
                        </a>
                      )}
                    </div>
                    <div style={{ display: 'flex', gap: 10, alignItems: 'center' }}>
                      {renderSelect('economy', models.economy, '⚡', 'rgba(52,211,153,0.6)')}
                      {renderSelect('reasoning', models.reasoning, '🧠', 'rgba(245,158,11,0.6)')}
                    </div>
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
                  <button
                    style={{ display: 'flex', alignItems: 'center', gap: 6, cursor: 'pointer', padding: '4px 0', background: 'none', border: 'none', width: '100%', font: 'inherit', color: 'inherit', textAlign: 'left' as const }}
                    onClick={() => setUsageExpanded(isExpanded ? null : agent.agent_type)}
                    aria-expanded={isExpanded}
                  >
                    <ChevronRight size={10} style={{ color: 'rgba(255,255,255,0.55)', transform: isExpanded ? 'rotate(90deg)' : 'none', transition: 'transform .15s' }} />
                    <Zap size={10} style={{ color: 'rgba(255,255,255,0.55)' }} />
                    <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.6)' }}>{t('config.estimateTokenUsage')}</span>
                    <span style={{ fontSize: 10, color, marginLeft: 'auto' }}>
                      ~{agentUsage.total_tokens.toLocaleString()} tok
                    </span>
                    <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.55)' }}>
                      {agentUsage.message_count} msg
                    </span>
                  </button>
                  {AGENT_USAGE_URLS[agent.agent_type] && (
                    <a
                      href={AGENT_USAGE_URLS[agent.agent_type]}
                      target="_blank"
                      rel="noopener noreferrer"
                      style={{ display: 'inline-flex', alignItems: 'center', marginLeft: 28, marginTop: 2, gap: 4, fontSize: 9, color: 'rgba(255,255,255,0.35)', textDecoration: 'none' }}
                      title="Provider usage dashboard"
                    >
                      <ExternalLink size={9} />
                      <span>Usage dashboard</span>
                    </a>
                  )}
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

          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 10, marginBottom: 16, maxHeight: 400, overflowY: 'auto', overflowX: 'hidden' }}>
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
                      background: skill.category === 'Language' ? 'rgba(59,130,246,0.15)' : skill.category === 'Business' ? 'rgba(16,185,129,0.15)' : 'rgba(200,255,0,0.1)',
                      color: skill.category === 'Language' ? '#60a5fa' : skill.category === 'Business' ? '#34d399' : '#c8ff00',
                      border: `1px solid ${skill.category === 'Language' ? 'rgba(59,130,246,0.3)' : skill.category === 'Business' ? 'rgba(16,185,129,0.3)' : 'rgba(200,255,0,0.2)'}`,
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
                    {skill.token_estimate > 0 && (
                      <span style={{ fontSize: 9, padding: '1px 6px', borderRadius: 6, background: 'rgba(255,165,0,0.1)', color: 'rgba(255,165,0,0.7)', border: '1px solid rgba(255,165,0,0.2)' }} title={t('config.tokenCostHint')}>
                        ~{skill.token_estimate} tok
                      </span>
                    )}
                  </div>
                </div>
                {skill.description && (
                  <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginBottom: 6 }}>{skill.description}</div>
                )}
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
                    <option value="Language">{t('skills.language')}</option>
                    <option value="Business">{t('skills.business')}</option>
                    <option value="Domain">{t('skills.domain')}</option>
                  </select>
                </div>
              </div>
              <div style={{ marginBottom: 10 }}>
                <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('skills.description')}</label>
                <input style={ss.input} value={newSkillDesc} onChange={e => setNewSkillDesc(e.target.value)} placeholder={t('skills.descriptionPlaceholder')} />
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
                      setNewSkillName(''); setNewSkillIcon('Star'); setNewSkillContent('');
                      toast(t('skills.add'), 'success');
                    } catch (err) { console.error(err); }
                  }}
                >
                  <Check size={12} /> {t('skills.add')}
                </button>
                <button
                  style={ss.iconBtn}
                  onClick={() => { setShowCreateSkill(false); setNewSkillName(''); setNewSkillIcon('Star'); setNewSkillContent(''); }}
                >
                  <X size={12} />
                </button>
              </div>
            </div>
          )}

        </div>
      </div>

      {/* Agent Profiles */}
      <div style={ss.card(false)}>
        <div style={{ padding: '16px 20px' }}>
          <h2 style={{ fontSize: 14, fontWeight: 700, color: '#e8eaed', marginBottom: 16, display: 'flex', alignItems: 'center', gap: 8 }}>
            <UserCircle size={16} style={{ color: '#a78bfa' }} /> {t('profiles.title')}
          </h2>

          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 12, marginBottom: 16, maxHeight: 400, overflowY: 'auto', overflowX: 'hidden' }}>
            {availableProfiles.map(profile => (
              <div key={profile.id} style={{
                padding: '14px 16px', borderRadius: 10, width: 280,
                background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.08)',
                borderLeft: `3px solid ${profile.color}`,
                position: 'relative' as const,
              }}>
                {/* Header: avatar + identity */}
                <div style={{ display: 'flex', gap: 12, marginBottom: 10 }}>
                  <div style={{
                    width: 44, height: 44, borderRadius: 10, display: 'flex', alignItems: 'center', justifyContent: 'center',
                    fontSize: 24, background: `${profile.color}18`, border: `1px solid ${profile.color}30`,
                    flexShrink: 0,
                  }}>
                    {profile.avatar}
                  </div>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontWeight: 700, fontSize: 13, color: '#e8eaed', lineHeight: 1.2, display: 'flex', alignItems: 'center', gap: 4 }}>
                      {editingPersonaId === profile.id ? (
                        <input
                          autoFocus
                          style={{
                            background: 'rgba(255,255,255,0.08)', border: `1px solid ${profile.color}60`, borderRadius: 4,
                            color: profile.color, fontWeight: 700, fontSize: 13, fontFamily: 'inherit',
                            padding: '1px 6px', width: 70,
                          }}
                          value={editingPersonaValue}
                          onChange={e => setEditingPersonaValue(e.target.value)}
                          onBlur={async () => {
                            if (editingPersonaValue !== profile.persona_name) {
                              try {
                                const updated = await profilesApi.updatePersonaName(profile.id, editingPersonaValue);
                                setAvailableProfiles(prev => prev.map(p => p.id === profile.id ? updated : p));
                              } catch (err) { console.error(err); }
                            }
                            setEditingPersonaId(null);
                          }}
                          onKeyDown={e => { if (e.key === 'Enter') (e.target as HTMLInputElement).blur(); if (e.key === 'Escape') setEditingPersonaId(null); }}
                        />
                      ) : (
                        <span
                          style={{ color: profile.color, cursor: 'pointer' }}
                          title={t('profiles.clickToEditName')}
                          onClick={() => { setEditingPersonaId(profile.id); setEditingPersonaValue(profile.persona_name); }}
                        >
                          {profile.persona_name || '—'}
                        </span>
                      )}
                      <span style={{ color: 'rgba(255,255,255,0.25)' }}>·</span>
                      {profile.name}
                    </div>
                    <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.45)', marginTop: 2, display: 'flex', alignItems: 'center', gap: 6 }}>
                      {profile.role}
                      {profile.token_estimate > 0 && (
                        <span style={{ fontSize: 9, padding: '0px 5px', borderRadius: 6, background: 'rgba(255,165,0,0.1)', color: 'rgba(255,165,0,0.7)', border: '1px solid rgba(255,165,0,0.2)' }} title={t('config.tokenCostHint')}>
                          ~{profile.token_estimate} tok
                        </span>
                      )}
                    </div>
                  </div>
                </div>
                {/* Description: expandable persona_prompt */}
                {profile.persona_prompt && (
                  <div style={{ marginBottom: 10 }}>
                    <div style={{
                      fontSize: 10, color: 'rgba(255,255,255,0.3)', lineHeight: 1.4,
                      ...(expandedProfileDesc !== profile.id ? {
                        overflow: 'hidden', display: '-webkit-box',
                        WebkitLineClamp: 2, WebkitBoxOrient: 'vertical' as const,
                      } : {}),
                    }}>
                      {expandedProfileDesc === profile.id ? profile.persona_prompt : profile.persona_prompt.slice(0, 150)}
                    </div>
                    {profile.persona_prompt.length > 100 && (
                      <button
                        style={{
                          fontSize: 9, color: profile.color, background: 'none', border: 'none',
                          cursor: 'pointer', padding: '2px 0', fontFamily: 'inherit', opacity: 0.8,
                        }}
                        onClick={() => setExpandedProfileDesc(expandedProfileDesc === profile.id ? null : profile.id)}
                      >
                        {expandedProfileDesc === profile.id ? t('common.seeLess') : t('common.seeMore')}
                      </button>
                    )}
                  </div>
                )}
                {/* Badges + actions */}
                <div style={{ display: 'flex', alignItems: 'center', gap: 4, flexWrap: 'wrap' }}>
                  <span style={{
                    fontSize: 9, padding: '1px 6px', borderRadius: 6, fontWeight: 600,
                    background: profile.category === 'Technical' ? 'rgba(59,130,246,0.15)' : profile.category === 'Business' ? 'rgba(16,185,129,0.15)' : 'rgba(139,92,246,0.1)',
                    color: profile.category === 'Technical' ? '#60a5fa' : profile.category === 'Business' ? '#34d399' : '#a78bfa',
                    border: `1px solid ${profile.category === 'Technical' ? 'rgba(59,130,246,0.3)' : profile.category === 'Business' ? 'rgba(16,185,129,0.3)' : 'rgba(139,92,246,0.2)'}`,
                  }}>
                    {t(`profiles.${profile.category.toLowerCase()}`)}
                  </span>
                  {profile.is_builtin ? (
                    <span style={{ fontSize: 9, padding: '1px 6px', borderRadius: 6, background: 'rgba(255,255,255,0.05)', color: 'rgba(255,255,255,0.3)', border: '1px solid rgba(255,255,255,0.08)' }}>
                      {t('profiles.builtin')}
                    </span>
                  ) : (
                    <span style={{ fontSize: 9, padding: '1px 6px', borderRadius: 6, background: 'rgba(139,92,246,0.1)', color: 'rgba(139,92,246,0.7)', border: '1px solid rgba(139,92,246,0.2)' }}>
                      {t('profiles.custom')}
                    </span>
                  )}
                  {profile.default_engine && (
                    <span style={{ fontSize: 9, padding: '1px 6px', borderRadius: 6, background: 'rgba(255,255,255,0.04)', color: 'rgba(255,255,255,0.25)', border: '1px solid rgba(255,255,255,0.06)' }}>
                      {profile.default_engine}
                    </span>
                  )}
                  <div style={{ flex: 1 }} />
                  {!profile.is_builtin && (
                    <button
                      style={{ ...ss.iconBtn, padding: '2px 6px', color: '#ff4d6a', borderColor: 'rgba(255,77,106,0.2)' }}
                      onClick={async () => {
                        if (!confirm(t('profiles.deleteConfirm'))) return;
                        try {
                          await profilesApi.delete(profile.id);
                          setAvailableProfiles(prev => prev.filter(p => p.id !== profile.id));
                          toast(t('common.delete'), 'success');
                        } catch (err) { console.error(err); }
                      }}
                    >
                      <Trash2 size={10} />
                    </button>
                  )}
                </div>
              </div>
            ))}
          </div>

          {!showCreateProfile ? (
            <button
              style={{ ...ss.scanBtn, gap: 6 }}
              onClick={() => setShowCreateProfile(true)}
            >
              <Plus size={12} /> {t('profiles.createCustom')}
            </button>
          ) : (
            <div style={{ padding: 16, borderRadius: 8, background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.1)' }}>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10, marginBottom: 10 }}>
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('profiles.name')}</label>
                  <input style={ss.input} value={newProfileName} onChange={e => setNewProfileName(e.target.value)} placeholder="Architect, QA Lead..." />
                </div>
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('profiles.personaName')}</label>
                  <input style={ss.input} value={newProfilePersonaName} onChange={e => setNewProfilePersonaName(e.target.value)} placeholder="Leo, Mia, Sam..." />
                </div>
              </div>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10, marginBottom: 10 }}>
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('profiles.role')}</label>
                  <input style={ss.input} value={newProfileRole} onChange={e => setNewProfileRole(e.target.value)} placeholder="Software Architect, QA Engineer..." />
                </div>
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('profiles.category')}</label>
                  <select
                    style={{ ...ss.input, cursor: 'pointer' }}
                    value={newProfileCategory}
                    onChange={e => setNewProfileCategory(e.target.value as any)}
                  >
                    <option value="Technical">{t('profiles.technical')}</option>
                    <option value="Business">{t('profiles.business')}</option>
                    <option value="Meta">{t('profiles.meta')}</option>
                  </select>
                </div>
              </div>
              <div style={{ display: 'grid', gridTemplateColumns: '60px 1fr 80px', gap: 10, marginBottom: 10 }}>
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('profiles.avatar')}</label>
                  <input style={{ ...ss.input, textAlign: 'center' as const, fontSize: 20, padding: '4px' }} value={newProfileAvatar} onChange={e => setNewProfileAvatar(e.target.value)} placeholder="🤖" />
                </div>
                <div />
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('profiles.color')}</label>
                  <input style={{ ...ss.input, width: '100%', height: 34, padding: 2, cursor: 'pointer' }} type="color" value={newProfileColor} onChange={e => setNewProfileColor(e.target.value)} />
                </div>
              </div>
              <div style={{ marginBottom: 10 }}>
                <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('profiles.persona')}</label>
                <textarea
                  style={{ ...ss.input, minHeight: 120, resize: 'vertical' as const, fontFamily: 'monospace', fontSize: 12, lineHeight: 1.5 }}
                  value={newProfilePersona}
                  onChange={e => setNewProfilePersona(e.target.value)}
                  placeholder="You are an expert in... Always prioritize..."
                />
              </div>
              <div style={{ display: 'flex', gap: 8 }}>
                <button
                  style={{ ...ss.scanBtn, opacity: newProfileName && newProfilePersona ? 1 : 0.4 }}
                  disabled={!newProfileName || !newProfilePersona}
                  onClick={async () => {
                    try {
                      const created = await profilesApi.create({
                        name: newProfileName,
                        persona_name: newProfilePersonaName,
                        role: newProfileRole,
                        avatar: newProfileAvatar,
                        color: newProfileColor,
                        category: newProfileCategory,
                        persona_prompt: newProfilePersona,
                      });
                      setAvailableProfiles(prev => [...prev, created]);
                      setShowCreateProfile(false);
                      setNewProfileName(''); setNewProfilePersonaName(''); setNewProfileRole(''); setNewProfileAvatar('🤖'); setNewProfileColor('#a78bfa'); setNewProfilePersona('');
                      toast(t('profiles.createCustom'), 'success');
                    } catch (err) { console.error(err); }
                  }}
                >
                  <Check size={12} /> {t('profiles.createCustom')}
                </button>
                <button
                  style={ss.iconBtn}
                  onClick={() => { setShowCreateProfile(false); setNewProfileName(''); setNewProfilePersonaName(''); setNewProfileRole(''); setNewProfileAvatar('🤖'); setNewProfileColor('#a78bfa'); setNewProfilePersona(''); }}
                >
                  <X size={12} />
                </button>
              </div>
            </div>
          )}

        </div>
      </div>

      {/* ── Directives (HOW) ── */}
      <div style={ss.card(false)}>
        <div style={{ padding: '16px 20px' }}>
          <h2 style={{ fontSize: 14, fontWeight: 700, color: '#e8eaed', marginBottom: 16, display: 'flex', alignItems: 'center', gap: 8 }}>
            <FileText size={16} style={{ color: '#f59e0b' }} /> {t('directives.title')}
          </h2>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 10, marginBottom: 16, maxHeight: 400, overflowY: 'auto', overflowX: 'hidden' }}>
            {availableDirectives.map(directive => (
              <div key={directive.id} style={{
                padding: '10px 14px', borderRadius: 8, width: 220,
                background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.08)',
              }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 4 }}>
                  <span style={{ fontWeight: 600, fontSize: 12, color: '#e8eaed' }}>
                    {directive.icon} {directive.name}
                  </span>
                  <div style={{ display: 'flex', gap: 4 }}>
                    <span style={{
                      fontSize: 9, padding: '1px 6px', borderRadius: 6, fontWeight: 600,
                      background: directive.category === 'Output' ? 'rgba(245,158,11,0.15)' : 'rgba(59,130,246,0.15)',
                      color: directive.category === 'Output' ? '#fbbf24' : '#60a5fa',
                      border: `1px solid ${directive.category === 'Output' ? 'rgba(245,158,11,0.3)' : 'rgba(59,130,246,0.3)'}`,
                    }}>
                      {t(`directives.${directive.category.toLowerCase()}`)}
                    </span>
                    {directive.is_builtin ? (
                      <span style={{ fontSize: 9, padding: '1px 6px', borderRadius: 6, background: 'rgba(255,255,255,0.05)', color: 'rgba(255,255,255,0.3)', border: '1px solid rgba(255,255,255,0.08)' }}>
                        {t('directives.builtin')}
                      </span>
                    ) : (
                      <span style={{ fontSize: 9, padding: '1px 6px', borderRadius: 6, background: 'rgba(245,158,11,0.1)', color: 'rgba(245,158,11,0.7)', border: '1px solid rgba(245,158,11,0.2)' }}>
                        {t('directives.custom')}
                      </span>
                    )}
                    {directive.token_estimate > 0 && (
                      <span style={{ fontSize: 9, padding: '1px 6px', borderRadius: 6, background: 'rgba(255,165,0,0.1)', color: 'rgba(255,165,0,0.7)', border: '1px solid rgba(255,165,0,0.2)' }} title={t('config.tokenCostHint')}>
                        ~{directive.token_estimate} tok
                      </span>
                    )}
                  </div>
                </div>
                {directive.description && (
                  <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginBottom: 4 }}>{directive.description}</div>
                )}
                {(directive.conflicts ?? []).length > 0 && (
                  <div style={{ fontSize: 9, color: 'rgba(255,77,106,0.6)', marginBottom: 4 }}>
                    ⚠ {t('directives.conflicts')}: {(directive.conflicts ?? []).join(', ')}
                  </div>
                )}
                {!directive.is_builtin && (
                  <button
                    style={{ ...ss.iconBtn, padding: '2px 6px', color: '#ff4d6a', borderColor: 'rgba(255,77,106,0.2)' }}
                    onClick={async () => {
                      if (!confirm(t('directives.deleteConfirm'))) return;
                      try {
                        await directivesApi.delete(directive.id);
                        setAvailableDirectives(prev => prev.filter(d => d.id !== directive.id));
                        toast(t('directives.remove'), 'success');
                      } catch (err) { console.error(err); }
                    }}
                  >
                    <Trash2 size={10} />
                  </button>
                )}
              </div>
            ))}
          </div>

          {/* Create custom directive form */}
          {!showCreateDirective ? (
            <button
              style={{ ...ss.scanBtn, gap: 6 }}
              onClick={() => setShowCreateDirective(true)}
            >
              <Plus size={12} /> {t('directives.createCustom')}
            </button>
          ) : (
            <div style={{ padding: 16, borderRadius: 8, background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.1)' }}>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10, marginBottom: 10 }}>
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('directives.name')}</label>
                  <input style={ss.input} value={newDirectiveName} onChange={e => setNewDirectiveName(e.target.value)} placeholder="My Directive" />
                </div>
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('directives.category')}</label>
                  <select
                    style={{ ...ss.input, cursor: 'pointer' }}
                    value={newDirectiveCategory}
                    onChange={e => setNewDirectiveCategory(e.target.value as any)}
                  >
                    <option value="Output">{t('directives.output')}</option>
                    <option value="Language">{t('directives.language')}</option>
                  </select>
                </div>
              </div>
              <div style={{ marginBottom: 10 }}>
                <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('directives.description')}</label>
                <input style={ss.input} value={newDirectiveDesc} onChange={e => setNewDirectiveDesc(e.target.value)} placeholder={t('directives.descriptionPlaceholder')} />
              </div>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10, marginBottom: 10 }}>
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('directives.icon')}</label>
                  <input style={ss.input} value={newDirectiveIcon} onChange={e => setNewDirectiveIcon(e.target.value)} placeholder="📋, 🔇, 📊..." />
                </div>
                <div>
                  <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('directives.conflicts')}</label>
                  <input style={ss.input} value={newDirectiveConflicts} onChange={e => setNewDirectiveConflicts(e.target.value)} placeholder="token-saver, verbose..." />
                </div>
              </div>
              <div style={{ marginBottom: 10 }}>
                <label style={{ fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', display: 'block', marginBottom: 4 }}>{t('directives.content')}</label>
                <textarea
                  style={{ ...ss.input, minHeight: 120, resize: 'vertical' as const, fontFamily: 'monospace', fontSize: 12, lineHeight: 1.5 }}
                  value={newDirectiveContent}
                  onChange={e => setNewDirectiveContent(e.target.value)}
                  placeholder="Instructions for agent output behavior..."
                />
              </div>
              <div style={{ display: 'flex', gap: 8 }}>
                <button
                  style={{ ...ss.scanBtn, opacity: newDirectiveName && newDirectiveContent ? 1 : 0.4 }}
                  disabled={!newDirectiveName || !newDirectiveContent}
                  onClick={async () => {
                    try {
                      const conflicts = newDirectiveConflicts.split(',').map(s => s.trim()).filter(Boolean);
                      const created = await directivesApi.create({
                        name: newDirectiveName,
                        description: newDirectiveDesc,
                        icon: newDirectiveIcon,
                        category: newDirectiveCategory,
                        content: newDirectiveContent,
                        conflicts: conflicts.length > 0 ? conflicts : undefined,
                      });
                      setAvailableDirectives(prev => [...prev, created]);
                      setShowCreateDirective(false);
                      setNewDirectiveName(''); setNewDirectiveIcon('📋'); setNewDirectiveContent(''); setNewDirectiveConflicts('');
                      toast(t('directives.add'), 'success');
                    } catch (err) { console.error(err); }
                  }}
                >
                  <Check size={12} /> {t('directives.createCustom')}
                </button>
                <button
                  style={ss.iconBtn}
                  onClick={() => { setShowCreateDirective(false); setNewDirectiveName(''); setNewDirectiveIcon('📋'); setNewDirectiveContent(''); setNewDirectiveConflicts(''); }}
                >
                  <X size={12} />
                </button>
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Server & Security */}
      <div style={ss.card(false)}>
        <div style={{ padding: '16px 20px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 16 }}>
            <Server size={14} style={{ color: '#c8ff00' }} />
            <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.server')}</span>
          </div>

          {/* Auth Token */}
          <div style={{ marginBottom: 16 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 6 }}>
              <Shield size={12} style={{ color: 'rgba(255,255,255,0.5)' }} />
              <span style={{ fontSize: 12, fontWeight: 600, color: 'rgba(255,255,255,0.6)' }}>{t('config.authToken')}</span>
              <span style={{ fontSize: 10, fontWeight: 600, padding: '1px 6px', borderRadius: 4, marginLeft: 4,
                background: authToken ? 'rgba(52,211,153,0.1)' : 'rgba(255,100,100,0.1)',
                color: authToken ? '#34d399' : '#ff6464',
                border: `1px solid ${authToken ? 'rgba(52,211,153,0.2)' : 'rgba(255,100,100,0.2)'}`,
              }}>
                {authToken ? t('config.authEnabled') : t('config.authDisabled')}
              </span>
            </div>
            {authToken ? (
              <>
                <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
                  <code style={{ ...ss.code, flex: 1, fontSize: 10, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {authVisible ? authToken : '••••••••••••••••••••'}
                  </code>
                  <button style={ss.iconBtn} onClick={() => setAuthVisible(!authVisible)}>
                    {authVisible ? <EyeOff size={11} /> : <Eye size={11} />}
                  </button>
                  <button style={ss.iconBtn} onClick={() => { navigator.clipboard.writeText(authToken); toast(t('config.authCopied'), 'success'); }}>
                    <Copy size={11} />
                  </button>
                  <button style={ss.iconBtn} onClick={async () => {
                    if (!confirm(t('config.authRegenConfirm'))) return;
                    try {
                      const newToken = await configApi.regenerateAuthToken();
                      setAuthTokenState(newToken);
                      setAuthToken(newToken);
                      toast(t('config.authRegenerated'), 'success');
                    } catch { toast(t('config.authRegenError'), 'error'); }
                  }}>
                    <RefreshCw size={11} />
                  </button>
                </div>
                <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', marginTop: 4 }}>
                  {t('config.authHint')}
                </div>
              </>
            ) : (
              <div>
                <button style={ss.installBtn} onClick={async () => {
                  try {
                    const newToken = await configApi.regenerateAuthToken();
                    setAuthTokenState(newToken);
                    setAuthToken(newToken);
                    setAuthVisible(true);
                    toast(t('config.authActivated'), 'success');
                  } catch { toast(t('config.authRegenError'), 'error'); }
                }}>
                  <Shield size={12} /> {t('config.authActivate')}
                </button>
                <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', marginTop: 4 }}>
                  {t('config.authDisabledHint')}
                </div>
              </div>
            )}
          </div>

          {/* Domain */}
          <div style={{ marginBottom: 16 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 6 }}>
              <Globe size={12} style={{ color: 'rgba(255,255,255,0.5)' }} />
              <span style={{ fontSize: 12, fontWeight: 600, color: 'rgba(255,255,255,0.6)' }}>{t('config.domain')}</span>
            </div>
            <div style={{ display: 'flex', gap: 6 }}>
              <input
                style={{ ...ss.code, flex: 1, background: 'rgba(255,255,255,0.04)', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 4, padding: '4px 8px', color: '#fff', fontSize: 12, fontFamily: 'inherit' }}
                value={serverDomain}
                onChange={e => setServerDomain(e.target.value)}
                placeholder="kronn.local"
              />
              <button style={ss.iconBtn} onClick={async () => {
                try {
                  await configApi.setServerConfig({ domain: serverDomain });
                  toast(t('config.domainSaved'), 'success');
                } catch { toast(t('config.domainError'), 'error'); }
              }}>
                <Save size={11} />
              </button>
            </div>
            <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', marginTop: 4 }}>
              {t('config.domainHint')}
            </div>
          </div>

          {/* Max concurrent agents */}
          <div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 6 }}>
              <Cpu size={12} style={{ color: 'rgba(255,255,255,0.5)' }} />
              <span style={{ fontSize: 12, fontWeight: 600, color: 'rgba(255,255,255,0.6)' }}>{t('config.maxAgents')}</span>
              <span style={{ fontSize: 11, fontWeight: 700, color: '#c8ff00', marginLeft: 4 }}>{serverMaxAgents}</span>
            </div>
            <input
              type="range"
              min={1}
              max={20}
              value={serverMaxAgents}
              onChange={async e => {
                const v = Number(e.target.value);
                setServerMaxAgents(v);
                try { await configApi.setServerConfig({ max_concurrent_agents: v }); } catch {}
              }}
              style={{ width: '100%', accentColor: '#c8ff00' }}
            />
            <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', marginTop: 4 }}>
              {t('config.maxAgentsHint')}
            </div>
          </div>
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
                { label: t('config.dbWorkflows'), value: dbInfo.workflow_count },
                { label: t('config.dbSkills'), value: dbInfo.custom_skill_count },
                { label: t('config.dbProfiles'), value: dbInfo.custom_profile_count },
                { label: t('config.dbDirectives'), value: dbInfo.custom_directive_count },
              ].filter(({ value }) => value > 0).map(({ label, value }) => (
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
      <div style={{ textAlign: 'center', padding: '20px 0 10px', color: 'rgba(255,255,255,0.3)', fontSize: 11 }}>
        Kronn v0.1.0 — <a href="https://github.com/DocRoms/Kronn" target="_blank" rel="noopener noreferrer" style={{ color: 'rgba(200,255,0,0.5)', textDecoration: 'none' }}>Source code (AGPL-3.0)</a>
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
  input: { width: '100%', padding: '8px 12px', background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.08)', borderRadius: 6, color: '#e8eaed', fontSize: 12, fontFamily: 'inherit' } as const,
  scanBtn: { padding: '7px 14px', borderRadius: 6, border: '1px solid rgba(200,255,0,0.2)', background: 'rgba(200,255,0,0.05)', color: '#c8ff00', cursor: 'pointer', fontSize: 12, fontFamily: 'inherit', fontWeight: 500, display: 'flex', alignItems: 'center', gap: 6 } as const,
  dangerBtn: { background: 'rgba(255,77,106,0.08)', border: '1px solid rgba(255,77,106,0.2)', borderRadius: 6, padding: '6px 14px', color: '#ff4d6a', cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6, fontSize: 12, fontFamily: 'inherit' } as const,
};
