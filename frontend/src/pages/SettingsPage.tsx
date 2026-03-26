import { useState, useEffect } from 'react';
import { version as appVersion } from '../../package.json';
import { config as configApi, skills as skillsApi, directives as directivesApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import { useT } from '../lib/I18nContext';
import { UI_LOCALES } from '../lib/i18n';
import type { AgentDetection, AgentsConfig, Project, Skill, Directive } from '../types/generated';
import type { ToastFn } from '../hooks/useToast';
import {
  MessageSquare, Cpu, Zap, AlertTriangle, Save,
  HardDrive, Plus, Trash2, Download, Upload, Check,
  RefreshCw, X, Eye, EyeOff,
  Layers, FolderSearch, Filter, FileText,
  Shield, Globe, Copy, Server, Mic, Volume2,
} from 'lucide-react';
import { STT_MODELS, getSttModelId, setSttModelId } from '../lib/stt-models';
import { TTS_VOICES, getTtsVoiceId, setTtsVoiceId } from '../lib/tts-models';
import { setAuthToken } from '../lib/api';
import { AgentsSection } from '../components/settings/AgentsSection';
import { IdentitySection } from '../components/settings/IdentitySection';
import { ProfilesSection } from '../components/settings/ProfilesSection';
import './SettingsPage.css';

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
  const [, setForceRender] = useState(0);
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
  const [serverStallTimeout, setServerStallTimeout] = useState(5);
  const [authToken, setAuthTokenState] = useState<string | null>(null);
  const [authVisible, setAuthVisible] = useState(false);

  // Internal API calls
  const { data: dbInfo, refetch: refetchDbInfo } = useApi(() => configApi.dbInfo(), []);
  useApi(() => configApi.getServerConfig().then(cfg => {
    if (cfg) { setServerDomain(cfg.domain ?? ''); setServerMaxAgents(cfg.max_concurrent_agents); setServerStallTimeout(cfg.agent_stall_timeout_min ?? 5); }
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

  useEffect(() => {
    skillsApi.list().then(setAvailableSkills).catch(() => {});
    directivesApi.list().then(setAvailableDirectives).catch(() => {});
  }, []);

  return (
    <div>
      <h1 className="set-h1">Configuration</h1>
      <p className="set-meta mb-9">{t('config.subtitle')}</p>

      {/* Section navigation */}
      <div className="set-nav">
        {[
          { id: 'settings-languages', label: 'Languages' },
          { id: 'settings-voice', label: t('settings.voice') },
          { id: 'settings-scan', label: 'Scan' },
          { id: 'settings-agents', label: 'Agents' },
          { id: 'settings-skills', label: 'Skills' },
          { id: 'settings-profiles', label: 'Profiles' },
          { id: 'settings-directives', label: 'Directives' },
          { id: 'settings-identity', label: t('settings.identity') },
          { id: 'settings-server', label: 'Server' },
          { id: 'settings-database', label: 'Database' },
        ].map(s => (
          <button
            key={s.id}
            className="set-nav-btn"
            onClick={() => document.getElementById(s.id)?.scrollIntoView({ behavior: 'smooth' })}
          >
            {s.label}
          </button>
        ))}
      </div>

      {/* UI Language */}
      <div id="settings-languages" className="set-card">
        <div className="set-section">
          <div className="flex-row gap-4 mb-4">
            <MessageSquare size={14} className="text-accent" />
            <span className="font-semibold text-lg">{t('config.uiLanguage')}</span>
          </div>
          <p className="set-hint">
            {t('config.uiLanguageHint')}
          </p>
          <div className="flex-row gap-4">
            {UI_LOCALES.map(l => (
              <button
                key={l.code}
                className="set-choice-btn"
                data-active={locale === l.code}
                onClick={() => setLocale(l.code)}
              >
                {l.flag} {l.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Output Language */}
      <div className="set-card">
        <div className="set-section">
          <div className="flex-row gap-4 mb-4">
            <MessageSquare size={14} className="text-accent" />
            <span className="font-semibold text-lg">{t('config.outputLanguage')}</span>
          </div>
          <p className="set-hint">
            {t('config.outputLanguageHint')}
          </p>
          <div className="flex-row gap-4">
            {LANGUAGES.map(l => (
              <button
                key={l.code}
                className="set-choice-btn"
                data-active={(configLanguage ?? 'fr') === l.code}
                onClick={async () => {
                  try { await configApi.saveLanguage(l.code); refetchLanguage(); }
                  catch { console.warn('Failed to save language'); }
                }}
              >
                {l.flag} {l.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Voice (STT model selection) */}
      <div id="settings-voice" className="set-card">
        <div className="set-section">
          <div className="flex-row gap-6 set-section-header-lg">
            <Mic size={14} className="text-accent" />
            <span className="font-semibold text-lg">{t('settings.voice')}</span>
          </div>

          <div className="mb-8">
            <div className="flex-row gap-4 mb-4">
              <Mic size={12} className="text-muted" />
              <span className="label">{t('settings.sttLabel')}</span>
            </div>
            <p className="set-hint-sm">
              {t('settings.sttDesc')}
            </p>
            <div className="flex-col gap-3">
              {STT_MODELS.map(m => {
                const active = getSttModelId() === m.id;
                return (
                  <button
                    key={m.id}
                    onClick={() => { setSttModelId(m.id); /* force re-render */ setForceRender(x => x + 1); }}
                    className="set-radio-option"
                    data-active={active}
                  >
                    <div className="set-radio-dot" data-active={active} />
                    <div className="flex-1">
                      <div className={`text-md font-semibold ${active ? 'text-primary' : 'text-secondary'}`}>
                        {m.label}
                        <span className="text-xs font-normal text-muted" style={{ marginLeft: 8 }}>{m.size}</span>
                      </div>
                      <div className="text-sm text-faint mt-2">{m.description}</div>
                    </div>
                    {active && <Check size={14} className="text-accent flex-shrink-0" />}
                  </button>
                );
              })}
            </div>
          </div>

          <div>
            <div className="flex-row gap-4 mb-4">
              <Volume2 size={12} className="text-muted" />
              <span className="label">{t('settings.ttsLabel')}</span>
            </div>
            <p className="set-hint">
              {t('settings.ttsDesc')}
            </p>
            {Object.entries(TTS_VOICES).map(([lang, lv]) => (
              <div key={lang} className="mb-6">
                <div className="set-tts-lang-title">
                  {lv.label}
                </div>
                <div className="flex-wrap gap-3">
                  {lv.voices.map(v => {
                    const active = getTtsVoiceId(lang) === v.id;
                    return (
                      <button
                        key={v.id}
                        onClick={() => { setTtsVoiceId(lang, v.id); setForceRender(x => x + 1); }}
                        className="set-voice-btn"
                        data-active={active}
                      >
                        <span className="set-gender-badge" data-gender={v.gender}>
                          {v.gender === 'F' ? 'F' : 'M'}
                        </span>
                        {v.label}
                        {active && <Check size={12} className="text-accent" />}
                      </button>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>

      {/* Scan (depth + paths + ignore) */}
      <div id="settings-scan" className="set-card">
        <div className="set-section">
          <div className="flex-row gap-4 set-section-header-lg">
            <FolderSearch size={14} className="text-accent" />
            <span className="font-semibold text-lg">{t('config.scanPaths')}</span>
            <span className="text-sm text-dim" style={{ marginLeft: 'auto' }}>
              {scanPaths.length} {scanPaths.length > 1 ? t('config.pathsPlural') : t('config.path')}
            </span>
          </div>

          {/* Scan Depth */}
          <div className="mb-8">
            <div className="flex-row gap-4 mb-3">
              <Layers size={12} className="text-muted" />
              <span className="label">{t('config.scanDepth')}</span>
              <span className="text-base font-semibold text-accent" style={{ minWidth: 24, textAlign: 'center', marginLeft: 'auto' }}>
                {scanDepth}
              </span>
            </div>
            <p className="text-sm text-faint mb-4">
              {t('config.scanDepthHint')}
            </p>
            <div className="flex-row gap-6">
              <span className="text-sm text-dim">2</span>
              <input
                type="range"
                min={2}
                max={10}
                value={scanDepth}
                onChange={async (e) => {
                  const v = Number(e.target.value);
                  setScanDepth(v);
                  try { await configApi.setScanDepth(v); }
                  catch { console.warn('Failed to save scan depth'); }
                }}
                className="set-range"
              />
              <span className="text-sm text-dim">10</span>
            </div>
          </div>

          {/* Scan Paths */}
          <div className="mb-8 set-inner-divider">
            <div className="flex-row gap-4 mb-3">
              <FolderSearch size={12} className="text-muted" />
              <span className="label">{t('config.scanPaths')}</span>
            </div>
            <p className="text-sm text-faint mb-4">
              {t('config.scanPathsHint')}
            </p>
            {scanPaths.map((p, i) => (
              <div key={i} className="flex-row gap-4 py-2">
                <code className="set-code text-sm flex-1 truncate">{p}</code>
                <button
                  className="set-icon-btn"
                  style={{ padding: '2px 4px' }}
                  onClick={async () => {
                    const updated = scanPaths.filter((_, j) => j !== i);
                    setScanPaths(updated);
                    try { await configApi.setScanPaths(updated); } catch (err) { console.warn('Settings action failed:', err); }
                  }}
                >
                  <Trash2 size={10} className="text-error" style={{ opacity: 0.5 }} />
                </button>
              </div>
            ))}
            <div className="flex-row gap-3 mt-4">
              <input
                type="text"
                className="set-input set-input-sm flex-1"
                placeholder={t('config.scanPathPlaceholder')}
                value={newScanPath}
                onChange={e => setNewScanPath(e.target.value)}
                onKeyDown={async (e) => {
                  if (e.key === 'Enter' && newScanPath.trim()) {
                    if (scanPaths.includes(newScanPath.trim())) return;
                    const updated = [...scanPaths, newScanPath.trim()];
                    setScanPaths(updated);
                    setNewScanPath('');
                    try { await configApi.setScanPaths(updated); } catch (err) { console.warn('Settings action failed:', err); }
                  }
                }}
              />
              <button
                className="set-icon-btn text-accent"
                style={{ padding: '4px 8px' }}
                onClick={async () => {
                  if (!newScanPath.trim()) return;
                  if (scanPaths.includes(newScanPath.trim())) return;
                  const updated = [...scanPaths, newScanPath.trim()];
                  setScanPaths(updated);
                  setNewScanPath('');
                  try { await configApi.setScanPaths(updated); } catch (err) { console.warn('Settings action failed:', err); }
                }}
              >
                <Plus size={12} />
              </button>
            </div>
          </div>

          {/* Scan Ignore */}
          <div className="set-inner-divider">
            <div className="flex-row gap-4 mb-3">
              <Filter size={12} className="text-muted" />
              <span className="label">{t('config.scanIgnore')}</span>
              <span className="text-sm text-dim" style={{ marginLeft: 'auto' }}>
                {scanIgnore.length} {scanIgnore.length > 1 ? t('config.patternsPlural') : t('config.pattern')}
              </span>
            </div>
            <p className="text-sm text-faint mb-4">
              {t('config.scanIgnoreHint')}
            </p>
            <div className="flex-wrap gap-3 mb-4">
              {scanIgnore.map((p, i) => (
                <div key={i} className="set-ignore-chip">
                  <span className="text-secondary">{p}</span>
                  <button
                    className="set-ignore-chip-x"
                    onClick={async () => {
                      const updated = scanIgnore.filter((_, j) => j !== i);
                      setScanIgnore(updated);
                      try { await configApi.setScanIgnore(updated); } catch (err) { console.warn('Settings action failed:', err); }
                    }}
                  >
                    <X size={9} className="text-error" style={{ opacity: 0.5 }} />
                  </button>
                </div>
              ))}
            </div>
            <div className="flex-row gap-3">
              <input
                type="text"
                className="set-input set-input-sm flex-1"
                placeholder={t('config.scanIgnorePlaceholder')}
                value={newIgnorePattern}
                onChange={e => setNewIgnorePattern(e.target.value)}
                onKeyDown={async (e) => {
                  if (e.key === 'Enter' && newIgnorePattern.trim()) {
                    const updated = [...scanIgnore, newIgnorePattern.trim()];
                    setScanIgnore(updated);
                    setNewIgnorePattern('');
                    try { await configApi.setScanIgnore(updated); } catch (err) { console.warn('Settings action failed:', err); }
                  }
                }}
              />
              <button
                className="set-icon-btn text-accent"
                style={{ padding: '4px 8px' }}
                onClick={async () => {
                  if (!newIgnorePattern.trim()) return;
                  const updated = [...scanIgnore, newIgnorePattern.trim()];
                  setScanIgnore(updated);
                  setNewIgnorePattern('');
                  try { await configApi.setScanIgnore(updated); } catch (err) { console.warn('Settings action failed:', err); }
                }}
              >
                <Plus size={12} />
              </button>
            </div>
          </div>
        </div>
      </div>

      {/* Agents */}
      <AgentsSection
        agents={agents}
        agentAccess={agentAccess}
        configLanguage={configLanguage}
        refetchAgents={refetchAgents}
        refetchAgentAccess={refetchAgentAccess}
        toast={toast}
        t={t}
      />

      {/* Skills */}
      <div id="settings-skills" className="set-card">
        <div className="set-section">
          <h2 className="flex-row gap-6 text-lg font-bold text-primary mb-8">
            <Zap size={16} className="text-accent" /> {t('skills.title')}
          </h2>

          <div className="flex-wrap mb-8" style={{ gap: 10, maxHeight: 400, overflowY: 'auto', overflowX: 'hidden' }}>
            {availableSkills.map(skill => (
              <div key={skill.id} className="set-item-card">
                <div className="flex-between mb-2">
                  <span className="font-semibold text-base text-primary">{skill.name}</span>
                  <div className="flex-row gap-2">
                    <span className="set-cat-badge" data-cat={skill.category}>
                      {t(`skills.${skill.category.toLowerCase()}`)}
                    </span>
                    {skill.is_builtin ? (
                      <span className="set-builtin-badge">{t('skills.builtin')}</span>
                    ) : (
                      <span className="set-custom-badge">{t('skills.custom')}</span>
                    )}
                    {skill.token_estimate > 0 && (
                      <span className="set-token-cost-badge" title={t('config.tokenCostHint')}>
                        ~{skill.token_estimate} tok
                      </span>
                    )}
                  </div>
                </div>
                {skill.description && (
                  <div className="text-sm text-muted mb-3">{skill.description}</div>
                )}
                {!skill.is_builtin && (
                  <button
                    className="set-icon-btn text-error"
                    style={{ padding: '2px 6px', borderColor: 'rgba(255,77,106,0.2)' }}
                    onClick={async () => {
                      if (!confirm(t('skills.deleteConfirm'))) return;
                      try {
                        await skillsApi.delete(skill.id);
                        setAvailableSkills(prev => prev.filter(s => s.id !== skill.id));
                        toast(t('skills.remove'), 'success');
                      } catch (err) { console.warn('Settings action failed:', err); }
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
              className="set-action-btn"
              onClick={() => setShowCreateSkill(true)}
            >
              <Plus size={12} /> {t('skills.createCustom')}
            </button>
          ) : (
            <div className="set-create-form">
              <div className="set-grid-2">
                <div>
                  <label className="set-form-label">{t('skills.name')}</label>
                  <input className="set-input" value={newSkillName} onChange={e => setNewSkillName(e.target.value)} placeholder="My Skill" />
                </div>
                <div>
                  <label className="set-form-label">{t('skills.category')}</label>
                  <select
                    className="set-input cursor-pointer"
                    value={newSkillCategory}
                    onChange={e => setNewSkillCategory(e.target.value as 'Language' | 'Domain' | 'Business')}
                  >
                    <option value="Language">{t('skills.language')}</option>
                    <option value="Business">{t('skills.business')}</option>
                    <option value="Domain">{t('skills.domain')}</option>
                  </select>
                </div>
              </div>
              <div className="mb-5">
                <label className="set-form-label">{t('skills.description')}</label>
                <input className="set-input" value={newSkillDesc} onChange={e => setNewSkillDesc(e.target.value)} placeholder={t('skills.descriptionPlaceholder')} />
              </div>
              <div className="mb-5">
                <label className="set-form-label">{t('skills.icon')}</label>
                <input className="set-input" value={newSkillIcon} onChange={e => setNewSkillIcon(e.target.value)} placeholder="Star, Code, Shield..." />
              </div>
              <div className="mb-5">
                <label className="set-form-label">{t('skills.content')}</label>
                <textarea
                  className="set-textarea"
                  value={newSkillContent}
                  onChange={e => setNewSkillContent(e.target.value)}
                  placeholder="System prompt instructions for this skill..."
                />
              </div>
              <div className="flex-row gap-4">
                <button
                  className="set-action-btn"
                  style={{ opacity: newSkillName && newSkillContent ? 1 : 0.4 }}
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
                    } catch (err) { console.warn('Settings action failed:', err); }
                  }}
                >
                  <Check size={12} /> {t('skills.add')}
                </button>
                <button
                  className="set-icon-btn"
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
      <ProfilesSection toast={toast} t={t} />


      {/* ── Directives (HOW) ── */}
      <div id="settings-directives" className="set-card">
        <div className="set-section">
          <h2 className="flex-row gap-6 text-lg font-bold text-primary mb-8">
            <FileText size={16} style={{ color: '#f59e0b' }} /> {t('directives.title')}
          </h2>
          <div className="flex-wrap mb-8" style={{ gap: 10, maxHeight: 400, overflowY: 'auto', overflowX: 'hidden' }}>
            {availableDirectives.map(directive => (
              <div key={directive.id} className="set-item-card">
                <div className="flex-between mb-2">
                  <span className="font-semibold text-base text-primary">
                    {directive.icon} {directive.name}
                  </span>
                  <div className="flex-row gap-2">
                    <span className="set-cat-badge" data-cat={directive.category}>
                      {t(`directives.${directive.category.toLowerCase()}`)}
                    </span>
                    {directive.is_builtin ? (
                      <span className="set-builtin-badge">{t('directives.builtin')}</span>
                    ) : (
                      <span className="set-custom-badge" data-variant="directive">{t('directives.custom')}</span>
                    )}
                    {directive.token_estimate > 0 && (
                      <span className="set-token-cost-badge" title={t('config.tokenCostHint')}>
                        ~{directive.token_estimate} tok
                      </span>
                    )}
                  </div>
                </div>
                {directive.description && (
                  <div className="text-sm text-muted mb-2">{directive.description}</div>
                )}
                {(directive.conflicts ?? []).length > 0 && (
                  <div className="text-2xs mb-2" style={{ color: 'rgba(255,77,106,0.6)' }}>
                    ⚠ {t('directives.conflicts')}: {(directive.conflicts ?? []).join(', ')}
                  </div>
                )}
                {!directive.is_builtin && (
                  <button
                    className="set-icon-btn text-error"
                    style={{ padding: '2px 6px', borderColor: 'rgba(255,77,106,0.2)' }}
                    onClick={async () => {
                      if (!confirm(t('directives.deleteConfirm'))) return;
                      try {
                        await directivesApi.delete(directive.id);
                        setAvailableDirectives(prev => prev.filter(d => d.id !== directive.id));
                        toast(t('directives.remove'), 'success');
                      } catch (err) { console.warn('Settings action failed:', err); }
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
              className="set-action-btn"
              onClick={() => setShowCreateDirective(true)}
            >
              <Plus size={12} /> {t('directives.createCustom')}
            </button>
          ) : (
            <div className="set-create-form">
              <div className="set-grid-2">
                <div>
                  <label className="set-form-label">{t('directives.name')}</label>
                  <input className="set-input" value={newDirectiveName} onChange={e => setNewDirectiveName(e.target.value)} placeholder="My Directive" />
                </div>
                <div>
                  <label className="set-form-label">{t('directives.category')}</label>
                  <select
                    className="set-input cursor-pointer"
                    value={newDirectiveCategory}
                    onChange={e => setNewDirectiveCategory(e.target.value as 'Output' | 'Language')}
                  >
                    <option value="Output">{t('directives.output')}</option>
                    <option value="Language">{t('directives.language')}</option>
                  </select>
                </div>
              </div>
              <div className="mb-5">
                <label className="set-form-label">{t('directives.description')}</label>
                <input className="set-input" value={newDirectiveDesc} onChange={e => setNewDirectiveDesc(e.target.value)} placeholder={t('directives.descriptionPlaceholder')} />
              </div>
              <div className="set-grid-2">
                <div>
                  <label className="set-form-label">{t('directives.icon')}</label>
                  <input className="set-input" value={newDirectiveIcon} onChange={e => setNewDirectiveIcon(e.target.value)} placeholder="📋, 🔇, 📊..." />
                </div>
                <div>
                  <label className="set-form-label">{t('directives.conflicts')}</label>
                  <input className="set-input" value={newDirectiveConflicts} onChange={e => setNewDirectiveConflicts(e.target.value)} placeholder="token-saver, verbose..." />
                </div>
              </div>
              <div className="mb-5">
                <label className="set-form-label">{t('directives.content')}</label>
                <textarea
                  className="set-textarea"
                  value={newDirectiveContent}
                  onChange={e => setNewDirectiveContent(e.target.value)}
                  placeholder="Instructions for agent output behavior..."
                />
              </div>
              <div className="flex-row gap-4">
                <button
                  className="set-action-btn"
                  style={{ opacity: newDirectiveName && newDirectiveContent ? 1 : 0.4 }}
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
                    } catch (err) { console.warn('Settings action failed:', err); }
                  }}
                >
                  <Check size={12} /> {t('directives.createCustom')}
                </button>
                <button
                  className="set-icon-btn"
                  onClick={() => { setShowCreateDirective(false); setNewDirectiveName(''); setNewDirectiveIcon('📋'); setNewDirectiveContent(''); setNewDirectiveConflicts(''); }}
                >
                  <X size={12} />
                </button>
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Identity */}
      <IdentitySection toast={toast} t={t} />


      <div id="settings-server" className="set-card">
        <div className="set-section">
          <div className="flex-row gap-4 mb-8">
            <Server size={14} className="text-accent" />
            <span className="font-semibold text-lg">{t('config.server')}</span>
          </div>

          {/* Auth Token */}
          <div className="mb-8">
            <div className="flex-row gap-3 mb-3">
              <Shield size={12} className="text-tertiary" />
              <span className="label" style={{ marginBottom: 0 }}>{t('config.authToken')}</span>
              <span className="set-auth-badge" data-on={!!authToken}>
                {authToken ? t('config.authEnabled') : t('config.authDisabled')}
              </span>
            </div>
            {authToken ? (
              <>
                <div className="flex-row gap-3">
                  <code className="set-code flex-1 text-xs truncate">
                    {authVisible ? authToken : '••••••••••••••••••••'}
                  </code>
                  <button className="set-icon-btn" onClick={() => setAuthVisible(!authVisible)}>
                    {authVisible ? <EyeOff size={11} /> : <Eye size={11} />}
                  </button>
                  <button className="set-icon-btn" onClick={() => { navigator.clipboard.writeText(authToken); toast(t('config.authCopied'), 'success'); }}>
                    <Copy size={11} />
                  </button>
                  <button className="set-icon-btn" onClick={async () => {
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
                <div className="set-hint-xs">
                  {t('config.authHint')}
                </div>
              </>
            ) : (
              <div>
                <button className="set-install-btn" onClick={async () => {
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
                <div className="set-hint-xs">
                  {t('config.authDisabledHint')}
                </div>
              </div>
            )}
          </div>

          {/* Domain */}
          <div className="mb-8">
            <div className="flex-row gap-3 mb-3">
              <Globe size={12} className="text-tertiary" />
              <span className="label" style={{ marginBottom: 0 }}>{t('config.domain')}</span>
            </div>
            <div className="flex-row gap-3">
              <input
                className="set-domain-input"
                value={serverDomain}
                onChange={e => setServerDomain(e.target.value)}
                placeholder="kronn.local"
              />
              <button className="set-icon-btn" onClick={async () => {
                try {
                  await configApi.setServerConfig({ domain: serverDomain });
                  toast(t('config.domainSaved'), 'success');
                } catch { toast(t('config.domainError'), 'error'); }
              }}>
                <Save size={11} />
              </button>
            </div>
            <div className="set-hint-xs">
              {t('config.domainHint')}
            </div>
          </div>

          {/* Max concurrent agents */}
          <div>
            <div className="flex-row gap-3 mb-3">
              <Cpu size={12} className="text-tertiary" />
              <span className="label" style={{ marginBottom: 0 }}>{t('config.maxAgents')}</span>
            </div>
            <div className="flex-row gap-6">
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
                className="set-range"
              />
              <span className="text-base font-semibold text-accent" style={{ minWidth: 24, textAlign: 'center' }}>{serverMaxAgents}</span>
            </div>
            <div className="set-hint-xs">
              {t('config.maxAgentsHint')}
            </div>
          </div>

          {/* Stall timeout */}
          <div className="mt-8">
            <div className="flex-row gap-4 mb-4">
              <span className="label" style={{ marginBottom: 0 }}>{t('settings.stallTimeout')}</span>
            </div>
            <div className="flex-row gap-6">
              <input
                type="range" min={1} max={60} step={1}
                value={serverStallTimeout}
                onChange={async (e) => {
                  const v = Number(e.target.value);
                  setServerStallTimeout(v);
                  try { await configApi.setServerConfig({ agent_stall_timeout_min: v }); } catch {}
                }}
                className="set-range"
              />
              <span className="text-base font-semibold text-accent" style={{ minWidth: 36, textAlign: 'center' }}>{serverStallTimeout} min</span>
            </div>
            <div className="set-hint-xs">
              {t('settings.stallTimeoutHint')}
            </div>
            {serverStallTimeout > 10 && (
              <div className="set-warning-callout">
                <AlertTriangle size={12} className="text-warning flex-shrink-0" />
                <span className="text-xs" style={{ color: 'rgba(245,158,11,0.8)', lineHeight: 1.4 }}>
                  {t('settings.stallTimeoutWarning')}
                </span>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Database */}
      <div id="settings-database" className="set-card">
        <div className="set-section">
          <div className="flex-row gap-4 set-section-header-lg">
            <HardDrive size={14} className="text-accent" />
            <span className="font-semibold text-lg">{t('config.database')}</span>
            {dbInfo && (
              <span className="text-sm text-dim" style={{ marginLeft: 'auto' }}>
                {dbInfo.size_bytes < 1024 * 1024
                  ? `${(dbInfo.size_bytes / 1024).toFixed(1)} Ko`
                  : `${(dbInfo.size_bytes / (1024 * 1024)).toFixed(1)} Mo`}
              </span>
            )}
          </div>

          {dbInfo && (
            <div className="flex-wrap gap-8 mb-8">
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
                <div key={label} className="set-db-stat">
                  <div className="set-db-stat-value">{value}</div>
                  <div className="set-db-stat-label">{label}</div>
                </div>
              ))}
            </div>
          )}

          <div className="flex-row gap-4">
            <button
              className="set-action-btn"
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
                } catch (err) { console.warn('Settings action failed:', err); }
              }}
            >
              <Download size={12} /> {t('config.export')}
            </button>
            <button
              className="set-action-btn"
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
      <div className="set-card mt-8">
        <div className="set-section">
          <p className="text-muted text-md mb-8">
            {t('config.configFile')} : <code className="set-code">~/.config/kronn/config.toml</code>
          </p>
          <div className="set-inner-divider" style={{ paddingTop: 16 }}>
            <p className="set-hint">
              {t('config.resetHint')}
            </p>
            <button className="set-danger-btn" onClick={onReset}>
              <Trash2 size={12} /> {t('config.reset')}
            </button>
          </div>
        </div>
      </div>
      <div className="set-footer">
        Kronn v{appVersion} — <a href="https://github.com/DocRoms/Kronn" target="_blank" rel="noopener noreferrer">Source code (AGPL-3.0)</a>
      </div>
    </div>
  );
}
