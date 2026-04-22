import { useState, useEffect, useCallback } from 'react';
import { useKonamiCode } from '../hooks/useKonamiCode';
import { version as appVersion } from '../../package.json';
import { config as configApi, skills as skillsApi, directives as directivesApi, autoTriggersApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import { useT } from '../lib/I18nContext';
import { UI_LOCALES } from '../lib/i18n';
import type { AgentDetection, AgentsConfig, Project, Skill, Directive } from '../types/generated';
import type { ToastFn } from '../hooks/useToast';
import {
  MessageSquare, Cpu, Zap, AlertTriangle, Save,
  HardDrive, Plus, Trash2, Pencil, Download, Upload, Check,
  RefreshCw, X, Eye, EyeOff,
  Layers, FolderSearch, Filter, FileText,
  Shield, Globe, Copy, Server, Mic, Volume2, HelpCircle, ChevronRight,
  Sun, Moon, Monitor, Terminal, Heart, Key,
} from 'lucide-react';
import { STT_MODELS, getSttModelId, setSttModelId } from '../lib/stt-models';
import { TTS_VOICES, getTtsVoiceId, setTtsVoiceId } from '../lib/tts-models';
import { setAuthToken } from '../lib/api';
import { useTheme, type ThemeMode } from '../lib/ThemeContext';
import { AgentsSection } from '../components/settings/AgentsSection';
import { IdentitySection } from '../components/settings/IdentitySection';
import { ProfilesSection } from '../components/settings/ProfilesSection';
import { UsageSection } from '../components/settings/UsageSection';
import { DebugSection } from '../components/settings/DebugSection';
import { MatrixText } from '../components/MatrixText';
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
  onNavigateDiscussion?: (discussionId: string) => void;
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
  onNavigateDiscussion,
  toast,
}: SettingsPageProps) {
  const { t, locale, setLocale } = useT();
  const { theme, setTheme, unlockedThemes, unlockTheme } = useTheme();

  // Secret-code state — local to the appearance card. The code is never
  // stored (UX: if the user refreshes, they can re-enter — keeps codes
  // from hanging around in memory or localStorage between sessions).
  const [secretCode, setSecretCode] = useState('');
  const [secretSubmitting, setSecretSubmitting] = useState(false);

  // The Secret Code row is hidden by default — it reveals itself when
  // the user enters the Konami code (↑↑↓↓←→←→BA) anywhere on the page.
  // Persisted across sessions so the reveal "sticks" on a given device.
  // Tampering doesn't matter: the input just fronts /api/themes/unlock,
  // which a curious user could hit with curl anyway.
  const [secretRevealed, setSecretRevealed] = useState(() => {
    try { return localStorage.getItem('kronn:secretInputRevealed') === '1'; }
    catch { return false; }
  });
  const handleKonamiReveal = useCallback(() => {
    setSecretRevealed(prev => {
      if (prev) return prev;
      try { localStorage.setItem('kronn:secretInputRevealed', '1'); } catch { /* noop */ }
      toast(t('config.konamiReveal'), 'info');
      return true;
    });
  }, [toast, t]);
  useKonamiCode(handleKonamiReveal);
  const handleUnlockSubmit = async () => {
    const code = secretCode.trim();
    if (!code) return;
    setSecretSubmitting(true);
    try {
      const unlocks = await unlockTheme(code);
      setSecretCode('');

      // Apply last theme in the bundle (if any) so the user sees the
      // result immediately. For bundles (e.g. kronnBatman → gotham +
      // batman profile) this picks gotham which is thematically tied
      // to the profile.
      const themeUnlock = unlocks.find(u => u.kind === 'theme');
      if (themeUnlock) {
        setTheme(themeUnlock.name as ThemeMode);
      }

      // Tell every consumer of the profile list to refetch — Batman is
      // now visible via GET /api/profiles. NewDiscussionForm, ChatHeader,
      // WorkflowWizard, and ProfilesSection all listen to this event.
      const profileUnlocks = unlocks.filter(u => u.kind === 'profile');
      if (profileUnlocks.length > 0) {
        window.dispatchEvent(new CustomEvent('kronn:profiles-changed'));
      }

      // Toast — one per unlocked item so the user knows what's new.
      // Profiles get a playful message, themes the existing one.
      for (const u of unlocks) {
        if (u.kind === 'profile' && u.name === 'batman') {
          toast(t('config.batmanRecruited'), 'success');
        } else if (u.kind === 'theme') {
          toast(t('config.secretUnlocked', u.name), 'success');
        }
      }
    } catch {
      // Generic toast — do not hint whether codes exist or not.
      toast(t('config.secretInvalid'), 'error');
    } finally {
      setSecretSubmitting(false);
    }
  };

  // Internal state
  const [, setForceRender] = useState(0);
  const [scanDepth, setScanDepth] = useState(4);
  const [scanPaths, setScanPaths] = useState<string[]>([]);
  const [scanIgnore, setScanIgnore] = useState<string[]>([]);
  const [newScanPath, setNewScanPath] = useState('');
  const [newIgnorePattern, setNewIgnorePattern] = useState('');
  const [configAccordion, setConfigAccordion] = useState<Set<string>>(() => new Set(['agents']));
  const toggleAccordion = (id: string) => setConfigAccordion(prev => {
    const next = new Set(prev);
    if (next.has(id)) next.delete(id); else next.add(id);
    return next;
  });
  const [availableSkills, setAvailableSkills] = useState<Skill[]>([]);
  // Operator opt-out list for skill auto-activation. Populated at mount
  // and kept in sync after each toggle — ChatInput listens to the same
  // window event so it picks up the change without a page reload.
  const [disabledAutoSkills, setDisabledAutoSkills] = useState<Set<string>>(new Set());
  useEffect(() => {
    autoTriggersApi.listDisabled()
      .then(ids => setDisabledAutoSkills(new Set(ids)))
      .catch(e => console.warn('load disabled auto-skills:', e));
  }, []);
  const toggleAutoTrigger = async (skillId: string) => {
    try {
      const nowDisabled = await autoTriggersApi.toggle(skillId);
      setDisabledAutoSkills(prev => {
        const next = new Set(prev);
        if (nowDisabled) next.add(skillId); else next.delete(skillId);
        return next;
      });
      window.dispatchEvent(new CustomEvent('kronn:auto-trigger-changed'));
    } catch (e) {
      console.warn('toggle auto-trigger failed:', e);
      toast(t('skills.autoToggleFailed'), 'error');
    }
  };
  const [showCreateSkill, setShowCreateSkill] = useState(false);
  // Non-null when editing an existing custom skill — same form, different submit
  // target. Cleared when the form closes.
  const [editingSkillId, setEditingSkillId] = useState<string | null>(null);
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
  const [serverDebugMode, setServerDebugMode] = useState(false);
  // True after the user just toggled debug_mode. Shows a "restart required"
  // callout because `tracing_subscriber`'s `EnvFilter` is frozen at startup
  // and the new level only kicks in on the next backend restart.
  const [debugModeNeedsRestart, setDebugModeNeedsRestart] = useState(false);
  const [authToken, setAuthTokenState] = useState<string | null>(null);
  const [authVisible, setAuthVisible] = useState(false);

  // Internal API calls
  const { data: dbInfo, refetch: refetchDbInfo } = useApi(() => configApi.dbInfo(), []);
  useApi(() => configApi.getServerConfig().then(cfg => {
    if (cfg) {
      setServerDomain(cfg.domain ?? '');
      setServerMaxAgents(cfg.max_concurrent_agents);
      setServerStallTimeout(cfg.agent_stall_timeout_min ?? 5);
      setServerDebugMode(cfg.debug_mode ?? false);
    }
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
      <h1 className="set-h1"><MatrixText text="Configuration" /></h1>
      <p className="set-meta mb-9">{t('config.subtitle')}</p>

      {/* Section navigation */}
      <div className="set-nav">
        {[
          { id: 'settings-appearance', label: t('config.appearance') },
          { id: 'settings-languages', label: 'Languages' },
          { id: 'settings-voice', label: t('settings.voice') },
          { id: 'settings-scan', label: 'Scan' },
          { id: 'settings-agent-config', label: 'Agents & Skills' },
          { id: 'settings-identity', label: t('settings.identity') },
          { id: 'settings-usage', label: t('config.usage') },
          { id: 'settings-server', label: t('config.server') },
          { id: 'settings-debug', label: t('settings.debugSection'), live: serverDebugMode },
          { id: 'settings-database', label: 'Database' },
        ].map(s => (
          <button
            key={s.id}
            className="set-nav-btn"
            data-live={s.live ? 'true' : undefined}
            onClick={() => document.getElementById(s.id)?.scrollIntoView({ behavior: 'smooth' })}
          >
            {s.label}
            {s.live && <span className="set-nav-live-dot" aria-hidden="true" />}
          </button>
        ))}
      </div>

      {/* Appearance / Theme */}
      <div id="settings-appearance" className="set-card">
        <div className="set-section">
          <div className="flex-row gap-4 mb-4">
            <Sun size={14} className="text-accent" />
            <span className="font-semibold text-lg">{t('config.appearance')}</span>
          </div>
          <p className="set-hint">
            {t('config.appearanceHint')}
          </p>
          <div className="flex-row gap-4" style={{ flexWrap: 'wrap' }}>
            {(() => {
              // Always-available themes first; unlocked secret themes append
              // in the order the user unlocked them. The secret block is
              // only rendered when at least one is unlocked — the picker
              // stays identical to before for users who never entered a code.
              const base: { mode: ThemeMode; label: string; icon: React.ReactNode }[] = [
                { mode: 'system', label: t('config.themeSystem'), icon: <Monitor size={14} /> },
                { mode: 'light', label: t('config.themeLight'), icon: <Sun size={14} /> },
                { mode: 'dark', label: t('config.themeDark'), icon: <Moon size={14} /> },
              ];
              const secretMeta: Record<string, { label: string; icon: React.ReactNode }> = {
                matrix: { label: t('config.themeMatrix'), icon: <Terminal size={14} /> },
                sakura: { label: t('config.themeSakura'), icon: <Heart size={14} /> },
                gotham: { label: t('config.themeGotham'), icon: <span style={{ fontSize: 14, lineHeight: 1 }}>🦇</span> },
              };
              const secret = unlockedThemes
                .filter(m => secretMeta[m])
                .map(m => ({ mode: m, ...secretMeta[m] }));
              return [...base, ...secret].map(opt => (
                <button
                  key={opt.mode}
                  className="set-choice-btn"
                  data-active={theme === opt.mode}
                  onClick={() => setTheme(opt.mode)}
                >
                  {opt.icon} {opt.label}
                </button>
              ));
            })()}
          </div>

          {/* Secret Code — hidden by default; revealed by the Konami
              code (see useKonamiCode above). The input itself trades a
              code for a hidden theme via POST /api/themes/unlock.
              The whole row is mounted only when `secretRevealed` is
              true, so pre-Konami users never even see it in the DOM. */}
          {secretRevealed && (
            <div
              className="flex-row gap-3 mt-4"
              style={{ alignItems: 'center', flexWrap: 'wrap' }}
            >
              <Key size={12} className="text-faint flex-shrink-0" />
              <span className="text-xs text-muted">{t('config.secretLabel')}</span>
              <input
                type="password"
                className="input"
                style={{ flex: '0 1 180px', fontSize: 'var(--kr-fs-sm)' }}
                placeholder={t('config.secretPlaceholder')}
                value={secretCode}
                onChange={e => setSecretCode(e.target.value)}
                onKeyDown={e => { if (e.key === 'Enter') handleUnlockSubmit(); }}
                disabled={secretSubmitting}
                autoComplete="off"
                spellCheck={false}
              />
              <button
                type="button"
                className="set-choice-btn"
                onClick={handleUnlockSubmit}
                disabled={secretSubmitting || !secretCode.trim()}
              >
                {t('config.secretSubmit')}
              </button>
            </div>
          )}
        </div>
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

      {/* ── Agent Config Accordion (Agents / Skills / Profiles / Directives) ── */}
      <div id="settings-agent-config" className="set-card">

        {/* Agents accordion */}
        <div className="set-accordion-section" id="settings-agents">
          <button className="set-accordion-header" onClick={() => toggleAccordion('agents')} aria-expanded={configAccordion.has('agents')}>
            <ChevronRight size={12} className="set-accordion-chevron" data-expanded={configAccordion.has('agents')} />
            <Cpu size={14} className="text-accent" />
            <span className="font-semibold text-base">Agents</span>
            <span className="set-accordion-count">{agents.length}</span>
          </button>
          {configAccordion.has('agents') && (
            <div className="set-accordion-body">
              <AgentsSection
                agents={agents}
                agentAccess={agentAccess}
                configLanguage={configLanguage}
                refetchAgents={refetchAgents}
                refetchAgentAccess={refetchAgentAccess}
                toast={toast}
                t={t}
              />
            </div>
          )}
        </div>

        {/* Skills accordion */}
        <div className="set-accordion-section" id="settings-skills">
          <button className="set-accordion-header" onClick={() => toggleAccordion('skills')} aria-expanded={configAccordion.has('skills')}>
            <ChevronRight size={12} className="set-accordion-chevron" data-expanded={configAccordion.has('skills')} />
            <Zap size={14} className="text-accent" />
            <span className="font-semibold text-base">{t('skills.title')}</span>
            <span className="set-accordion-count">{availableSkills.length}</span>
          </button>
          {configAccordion.has('skills') && (
          <div className="set-accordion-body">
          <div className="set-section">

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
                {/* Auto-activation toggle — only relevant when the skill
                    declares `auto_triggers` in its YAML frontmatter.
                    Displayed as a compact switch-row so it doesn't
                    crowd the card. */}
                {skill.auto_triggers && (
                  <div className="flex-row gap-2 mb-3" style={{ alignItems: 'center' }}>
                    <button
                      type="button"
                      className="set-choice-btn"
                      style={{ padding: '2px 8px', fontSize: 'var(--kr-fs-xs)' }}
                      data-active={!disabledAutoSkills.has(skill.id)}
                      onClick={() => toggleAutoTrigger(skill.id)}
                      title={t('skills.autoTriggerHint')}
                    >
                      <Zap size={9} />
                      {disabledAutoSkills.has(skill.id)
                        ? t('skills.autoTriggerOff')
                        : t('skills.autoTriggerOn')}
                    </button>
                  </div>
                )}
                {!skill.is_builtin && (
                  <div className="flex-row gap-2">
                    <button
                      className="set-icon-btn"
                      style={{ padding: '2px 6px' }}
                      title={t('skills.editCustom')}
                      onClick={() => {
                        // Strip the frontmatter before populating the textarea —
                        // the backend re-generates it from the form fields on save.
                        // Without stripping, each edit round would nest a new
                        // frontmatter block inside the content body.
                        const body = skill.content.replace(/^---\s*\n[\s\S]*?\n---\s*\n?/, '');
                        setEditingSkillId(skill.id);
                        setNewSkillName(skill.name);
                        setNewSkillDesc(skill.description);
                        setNewSkillIcon(skill.icon);
                        setNewSkillCategory(skill.category);
                        setNewSkillContent(body);
                        setShowCreateSkill(true);
                      }}
                    >
                      <Pencil size={10} />
                    </button>
                    <button
                      className="set-icon-btn text-error"
                      style={{ padding: '2px 6px', borderColor: 'rgba(var(--kr-error-rgb), 0.2)' }}
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
                  </div>
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
                      const payload = {
                        name: newSkillName,
                        description: newSkillDesc,
                        icon: newSkillIcon,
                        category: newSkillCategory,
                        content: newSkillContent,
                      };
                      if (editingSkillId) {
                        // Backend update = delete + recreate. ID may shift if
                        // name slug changed — replace in-place by filtering
                        // out the old ID before appending the new record.
                        const updated = await skillsApi.update(editingSkillId, payload);
                        setAvailableSkills(prev => [
                          ...prev.filter(s => s.id !== editingSkillId),
                          updated,
                        ]);
                        toast(t('skills.saveChanges'), 'success');
                      } else {
                        const created = await skillsApi.create(payload);
                        setAvailableSkills(prev => [...prev, created]);
                        toast(t('skills.add'), 'success');
                      }
                      setShowCreateSkill(false);
                      setEditingSkillId(null);
                      setNewSkillName(''); setNewSkillDesc(''); setNewSkillIcon('Star'); setNewSkillContent('');
                    } catch (err) { console.warn('Settings action failed:', err); }
                  }}
                >
                  <Check size={12} /> {editingSkillId ? t('skills.saveChanges') : t('skills.add')}
                </button>
                <button
                  className="set-icon-btn"
                  onClick={() => {
                    setShowCreateSkill(false);
                    setEditingSkillId(null);
                    setNewSkillName(''); setNewSkillDesc(''); setNewSkillIcon('Star'); setNewSkillContent('');
                  }}
                >
                  <X size={12} />
                </button>
              </div>
            </div>
          )}

          </div>
          </div>
          )}
        </div>

        {/* Profiles accordion */}
        <div className="set-accordion-section" id="settings-profiles">
          <button className="set-accordion-header" onClick={() => toggleAccordion('profiles')} aria-expanded={configAccordion.has('profiles')}>
            <ChevronRight size={12} className="set-accordion-chevron" data-expanded={configAccordion.has('profiles')} />
            <Layers size={14} style={{ color: 'var(--kr-purple)' }} />
            <span className="font-semibold text-base">{t('profiles.title')}</span>
          </button>
          {configAccordion.has('profiles') && (
            <div className="set-accordion-body">
              <ProfilesSection toast={toast} t={t} />
            </div>
          )}
        </div>

        {/* Directives accordion */}
        <div className="set-accordion-section" id="settings-directives">
          <button className="set-accordion-header" onClick={() => toggleAccordion('directives')} aria-expanded={configAccordion.has('directives')}>
            <ChevronRight size={12} className="set-accordion-chevron" data-expanded={configAccordion.has('directives')} />
            <FileText size={14} style={{ color: 'var(--kr-warning-amber)' }} />
            <span className="font-semibold text-base">{t('directives.title')}</span>
            <span className="set-accordion-count">{availableDirectives.length}</span>
          </button>
          {configAccordion.has('directives') && (
          <div className="set-accordion-body">
          <div className="set-section">
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
                  <div className="text-2xs mb-2" style={{ color: 'rgba(var(--kr-error-rgb), 0.6)' }}>
                    ⚠ {t('directives.conflicts')}: {(directive.conflicts ?? []).join(', ')}
                  </div>
                )}
                {!directive.is_builtin && (
                  <button
                    className="set-icon-btn text-error"
                    style={{ padding: '2px 6px', borderColor: 'rgba(var(--kr-error-rgb), 0.2)' }}
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
          )}
        </div>

      </div>{/* end settings-agent-config card */}

      {/* Identity */}
      <IdentitySection toast={toast} t={t} />

      {/* Usage — moved before Server so non-dev users see their consumption first */}
      <UsageSection onNavigateDiscussion={onNavigateDiscussion} />

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
                <span className="text-xs" style={{ color: 'rgba(var(--kr-warning-amber-rgb), 0.8)', lineHeight: 1.4 }}>
                  {t('settings.stallTimeoutWarning')}
                </span>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Debug — its own card so it's easy to find when diagnosing cross-platform
          issues. Toggling the switch persists `debug_mode` in config.toml
          (takes effect at next backend restart). The live viewer reads the
          in-memory ringbuffer fed by every `tracing` event — no file on disk. */}
      <DebugSection
        serverDebugMode={serverDebugMode}
        setServerDebugMode={setServerDebugMode}
        debugModeNeedsRestart={debugModeNeedsRestart}
        setDebugModeNeedsRestart={setDebugModeNeedsRestart}
        t={t}
      />

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

          <div className="set-export-warning">
            <AlertTriangle size={14} />
            <span>
              {t('config.exportApiWarning')}{' '}
              <a
                href="#settings-server"
                onClick={(e) => {
                  e.preventDefault();
                  document.getElementById('settings-server')?.scrollIntoView({ behavior: 'smooth' });
                }}
              >{t('config.exportApiWarningLink')}</a>
            </span>
          </div>
          <div className="flex-row gap-4">
            <button
              className="set-action-btn"
              onClick={async () => {
                try {
                  const blob = await configApi.exportData();
                  const url = URL.createObjectURL(blob);
                  const a = document.createElement('a');
                  a.href = url;
                  a.download = `kronn-export-${new Date().toISOString().slice(0, 10)}.zip`;
                  a.click();
                  URL.revokeObjectURL(url);
                } catch (err) { console.warn('Export failed:', err); }
              }}
            >
              <Download size={12} /> {t('config.export')}
            </button>
            <button
              className="set-action-btn"
              onClick={() => {
                const input = document.createElement('input');
                input.type = 'file';
                input.accept = '.zip,.json';
                input.onchange = async () => {
                  const file = input.files?.[0];
                  if (!file) return;
                  // Validate JSON files (legacy) — ZIP files are validated server-side
                  if (file.name.endsWith('.json')) {
                    try {
                      const text = await file.text();
                      const data = JSON.parse(text);
                      if (!data.version || !data.projects || !data.discussions) {
                        toast(t('config.importInvalid'), 'error');
                        return;
                      }
                    } catch {
                      toast(t('config.importInvalid'), 'error');
                      return;
                    }
                  }
                  if (!confirm(t('config.importConfirm'))) return;
                  try {
                    const result = await configApi.importData(file);
                    refetchProjects();
                    refetchDiscussions();
                    refetchDbInfo();
                    // Show post-import warnings
                    if (result.warnings.length > 0) {
                      for (const w of result.warnings) {
                        toast(w, 'info');
                      }
                    }
                    if (result.invalid_paths.length > 0) {
                      toast(t('config.importPathWarning', result.invalid_paths.length), 'info');
                    }
                    toast(t('config.importSuccess'), 'success');
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
            <button className="set-action-btn" onClick={() => {
              localStorage.removeItem('kronn:tour-completed');
              window.location.reload();
            }}>
              <HelpCircle size={12} /> {t('tour.replay')}
            </button>
          </div>
          <div className="set-inner-divider" style={{ paddingTop: 16 }}>
            <p className="set-hint">
              {t('config.resetHint')}
            </p>
            <button className="set-danger-btn" onClick={() => {
              if (!confirm(t('config.resetConfirm'))) return;
              onReset();
            }}>
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
