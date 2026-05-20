import { useState, useEffect, useRef } from 'react';
import '../pages/DiscussionsPage.css';
import { ProfileTooltip } from './ProfileTooltip';
import { skills as skillsApi, profiles as profilesApi, directives as directivesApi } from '../lib/api';
import type { Project, AgentDetection, AgentType, AgentsConfig, Skill, AgentProfile, Directive } from '../types/generated';
import { AGENT_LABELS, isAgentRestricted as isAgentRestrictedUtil, isUsable, isHiddenPath } from '../lib/constants';
import {
  Folder, ChevronRight, GitBranch,
  MessageSquare, X, AlertTriangle,
  Settings, Check, Zap, UserCircle, FileText, Paperclip, Image,
} from 'lucide-react';

// ─── Public types ────────────────────────────────────────────────────────────

export interface NewDiscConfig {
  title: string;
  agent: AgentType;
  projectId: string | null;
  prompt: string;
  skillIds: string[];
  profileIds: string[];
  directiveIds: string[];
  workspaceMode: 'Direct' | 'Isolated';
  tier: 'economy' | 'default' | 'reasoning';
  branchName: string;
  baseBranch: string;
  pendingFiles?: File[];
  /** 0.8.6 phase 2 — when `false`, the parent should create the disc
   *  WITHOUT auto-launching an agent (CLI run skipped). The user is
   *  expected to invite agents later via the `[+ Inviter]` header
   *  button. Default `true` preserves the legacy behaviour. */
  launchAgentNow: boolean;
}

export interface NewDiscussionFormProps {
  projects: Project[];
  agents: AgentDetection[];
  configLanguage: string | null;
  agentAccess: AgentsConfig | null;
  prefill?: { projectId: string; title: string; prompt: string; locked?: boolean } | null;
  onSubmit: (config: NewDiscConfig) => void;
  onClose: () => void;
  onPrefillConsumed?: () => void;
  onNavigate: (page: string) => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

// ─── Component ───────────────────────────────────────────────────────────────

export function NewDiscussionForm({
  projects,
  agents,
  agentAccess,
  prefill,
  onSubmit,
  onClose,
  onPrefillConsumed,
  onNavigate,
  t,
}: NewDiscussionFormProps) {
  // ─── Internal state ──────────────────────────────────────────────────────
  const [newDiscTitle, setNewDiscTitle] = useState('');
  const [newDiscAgent, setNewDiscAgent] = useState<AgentType | ''>('');
  const [newDiscProjectId, setNewDiscProjectId] = useState<string>('');
  const [newDiscPrompt, setNewDiscPrompt] = useState('');
  const [newDiscPrefilled, setNewDiscPrefilled] = useState(false);
  const [showAdvancedOptions, setShowAdvancedOptions] = useState(false);
  const [expandedAdvanced, setExpandedAdvanced] = useState<'skills' | 'profiles' | 'directives' | null>(null);

  const [availableSkills, setAvailableSkills] = useState<Skill[]>([]);
  const [newDiscSkillIds, setNewDiscSkillIds] = useState<string[]>([]);
  const [availableProfiles, setAvailableProfiles] = useState<AgentProfile[]>([]);
  const [newDiscProfileIds, setNewDiscProfileIds] = useState<string[]>([]);
  const [newDiscDirectiveIds, setNewDiscDirectiveIds] = useState<string[]>([]);
  const [availableDirectives, setAvailableDirectives] = useState<Directive[]>([]);
  const [newDiscWorkspaceMode, setNewDiscWorkspaceMode] = useState<'Direct' | 'Isolated'>('Direct');
  const [newDiscTier, setNewDiscTier] = useState<'economy' | 'default' | 'reasoning'>('default');
  // 0.8.6 phase 2 — disc-first refactor. When `false`, the disc is
  // created without launching a CLI ; the user invites agents later
  // via the `[+ Inviter]` header button. Default `true` keeps the
  // legacy "create + run" flow for the 80% common case.
  const [launchAgentNow, setLaunchAgentNow] = useState(true);
  const [newDiscBranchName, setNewDiscBranchName] = useState('');
  const [newDiscBaseBranch, setNewDiscBaseBranch] = useState('main');
  const [pendingFiles, setPendingFiles] = useState<File[]>([]);
  const newDiscFileInputRef = useRef<HTMLInputElement>(null);

  // ─── Derived ─────────────────────────────────────────────────────────────
  const installedAgentsList = agents.filter(isUsable);

  const isAgentRestricted = (agentType: AgentType): boolean =>
    isAgentRestrictedUtil(agentAccess ?? undefined, agentType);

  // ─── Effects ─────────────────────────────────────────────────────────────

  // Fetch available skills, profiles, directives. Also re-fetch
  // profiles on `kronn:profiles-changed` so a secret-code unlock
  // (e.g. Batman) flips the picker without a reload.
  useEffect(() => {
    const refetchProfiles = () => profilesApi.list()
      .then(setAvailableProfiles)
      .catch(e => console.warn('Failed to load profiles:', e));
    skillsApi.list().then(setAvailableSkills).catch(e => console.warn('Failed to load skills:', e));
    refetchProfiles();
    directivesApi.list().then(setAvailableDirectives).catch(e => console.warn('Failed to load directives:', e));
    window.addEventListener('kronn:profiles-changed', refetchProfiles);
    return () => window.removeEventListener('kronn:profiles-changed', refetchProfiles);
  }, []);

  // Auto-select first installed agent if current selection is invalid
  useEffect(() => {
    if (installedAgentsList.length > 0 && !installedAgentsList.some(a => a.agent_type === newDiscAgent)) {
      setNewDiscAgent(installedAgentsList[0].agent_type);
    }
  }, [installedAgentsList.length, newDiscAgent]);

  // Handle prefill from parent (e.g. "validate audit" button on Projects page)
  useEffect(() => {
    if (prefill) {
      // Lock fields only when explicitly requested (validation audit)
      setNewDiscPrefilled(!!prefill.locked);
      setNewDiscProjectId(prefill.projectId);
      setNewDiscTitle(prefill.title);
      setNewDiscPrompt(prefill.prompt);
      // Auto-select mandatory profiles ONLY for validation audits.
      // Pre-fix this fired on every prefill — including the "New discussion"
      // button on a project card and the "Discuss this file" CTA from the AI
      // doc viewer — which silently pre-selected architect/tech-lead/qa-
      // engineer profiles for unrelated chats. Bound to `locked` because
      // only the validation entry-point sets that flag.
      if (prefill.locked) {
        const validationProfileIds = ['architect', 'tech-lead', 'qa-engineer'];
        setNewDiscProfileIds(validationProfileIds);
      }
      onPrefillConsumed?.();
    }
  }, [prefill, onPrefillConsumed]);

  // ─── Callbacks ───────────────────────────────────────────────────────────

  const handleClose = () => {
    setNewDiscPrefilled(false);
    setNewDiscWorkspaceMode('Direct');
    setNewDiscBranchName('');
    setNewDiscBaseBranch('main');
    onClose();
  };

  const [creating, setCreating] = useState(false);
  const creatingRef = useRef(false);

  const handleCreate = async () => {
    // Submit gate :
    //   - launch mode  → prompt + agent both required (legacy contract)
    //   - no-launch    → prompt OR title required so the empty disc has
    //                    SOMETHING to display in the list. We default
    //                    the title to a placeholder when both blank.
    if (creatingRef.current) return;
    if (launchAgentNow) {
      if (!newDiscPrompt.trim() || !newDiscAgent) return;
    } else {
      if (!newDiscPrompt.trim() && !newDiscTitle.trim()) return;
    }
    creatingRef.current = true;
    setCreating(true);
    try {
      // `onSubmit` is typed `=> void` but the parent's implementation may be
      // async — await it through Promise.resolve so failures unblock the
      // button. Without this, if `discussions.create` throws, `creating`
      // stays true forever and the form is wedged until close+reopen.
      const fallbackTitle = launchAgentNow
        ? newDiscPrompt.trim().slice(0, 60)
        : (newDiscPrompt.trim().slice(0, 60) || t('disc.discFirstDefaultTitle'));
      await Promise.resolve(onSubmit({
        title: newDiscTitle.trim() || fallbackTitle,
        // Even when `launchAgentNow=false`, the backend `CreateDiscussionRequest`
        // still requires an agent_type field (legacy `discussions.agent`
        // column NOT NULL). We send the currently-selected agent as a
        // placeholder ; the parent skips `runAgent` so no CLI runs.
        // The new `discussion_sessions` table is the source of truth for
        // actual participants from 0.8.6 onward.
        agent: (newDiscAgent || installedAgentsList[0]?.agent_type || 'ClaudeCode') as AgentType,
        projectId: newDiscProjectId || null,
        prompt: newDiscPrompt.trim(),
        skillIds: newDiscSkillIds,
        profileIds: newDiscProfileIds,
        directiveIds: newDiscDirectiveIds,
        workspaceMode: newDiscWorkspaceMode,
        tier: newDiscTier,
        branchName: newDiscBranchName,
        baseBranch: newDiscBaseBranch,
        pendingFiles: pendingFiles.length > 0 ? pendingFiles : undefined,
        launchAgentNow,
      }));
    } catch (e) {
      // Parent (`handleCreateDiscussion` in DiscussionsPage) already toasts
      // its own errors. We swallow here only to keep the form unwedged —
      // the `finally` reset alone isn't enough because an uncaught throw
      // becomes an unhandled-rejection warning in the dev console.
      console.warn('[NewDiscussionForm] onSubmit rejected:', e);
    } finally {
      creatingRef.current = false;
      setCreating(false);
    }
  };

  // ─── Render ──────────────────────────────────────────────────────────────

  return (
    <div
      className="disc-new-overlay"
      onClick={e => { if (e.target === e.currentTarget) handleClose(); }}
      onKeyDown={e => { if (e.key === 'Escape') handleClose(); }}
      role="dialog"
      aria-modal="true"
      tabIndex={-1}
    >
      <div
        className="disc-new-card"
        onKeyDown={e => {
          if (e.key === 'Escape') { e.stopPropagation(); handleClose(); }
          // Pre-fix: no `preventDefault` here, so Ctrl+Enter inside the
          // prompt textarea inserted a newline AND triggered submit. The
          // submitted prompt ended with a stray "\n" — visible in agent
          // transcripts as a blank line at the bottom of the first
          // message. Suppress the default keypress so only the submit
          // path fires.
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && !e.nativeEvent.isComposing && newDiscPrompt.trim()) {
            e.preventDefault();
            handleCreate();
          }
        }}
      >
        <div className="disc-new-header">
          <span className="disc-new-title">{t('disc.newTitle')}</span>
          <button className="disc-icon-btn" onClick={handleClose} aria-label="Close"><X size={14} /></button>
        </div>

        <div className="disc-new-grid">
          <div>
            <label className="disc-form-label">{t('disc.project')}</label>
            <select className="disc-select-styled" aria-label={t('disc.project')} data-locked={newDiscPrefilled} value={newDiscProjectId} onChange={e => {
              const pid = e.target.value;
              setNewDiscProjectId(pid);
              const proj = projects.find(p => p.id === pid);
              if (proj?.default_skill_ids?.length) setNewDiscSkillIds(proj.default_skill_ids);
              setNewDiscWorkspaceMode('Direct');
              setNewDiscBranchName('');
              setNewDiscBaseBranch('main');
            }} disabled={newDiscPrefilled}>
              <option value="">{t('disc.noProject')}</option>
              {projects.filter(p => !isHiddenPath(p.path)).map(p => (
                <option key={p.id} value={p.id}>{p.name}</option>
              ))}
            </select>
          </div>
          <div>
            <label className="disc-form-label">
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                <input
                  type="checkbox"
                  checked={launchAgentNow}
                  onChange={e => setLaunchAgentNow(e.target.checked)}
                  aria-label={t('disc.launchAgentNow')}
                  style={{ margin: 0 }}
                />
                {t('disc.launchAgentNow')}
                {/* 0.8.6 phase 2 — tooltip via native `title` keeps the
                    info discoverable without inflating the form. Copy is
                    23 words, validated user-side 2026-05-20. */}
                <span
                  className="disc-form-info-icon"
                  title={t('disc.launchAgentNowHint')}
                  aria-label={t('disc.launchAgentNowHint')}
                  style={{ cursor: 'help', opacity: 0.7, fontSize: '0.85em' }}
                >
                  ⓘ
                </span>
              </span>
            </label>
            {launchAgentNow ? (
              <select className="disc-select-styled" aria-label={t('disc.agent')} value={newDiscAgent} onChange={e => setNewDiscAgent(e.target.value as AgentType)}>
                {installedAgentsList.map(a => (
                  <option key={a.name} value={a.agent_type}>{a.name}</option>
                ))}
                {installedAgentsList.length === 0 && (
                  <option value="" disabled>{t('disc.noAgent')}</option>
                )}
              </select>
            ) : (
              <div className="disc-form-hint" style={{ fontSize: '0.85em', opacity: 0.7, padding: '6px 0' }}>
                {t('disc.discFirstHint')}
              </div>
            )}
          </div>
        </div>

        {newDiscAgent && isAgentRestricted(newDiscAgent as AgentType) && (
          <div className="disc-restricted-warn">
            <AlertTriangle size={11} style={{ color: 'var(--kr-warning)', flexShrink: 0 }} />
            <span className="disc-restricted-warn-text">
              {t('config.restrictedAgent', AGENT_LABELS[newDiscAgent] ?? newDiscAgent)}
              {' — '}
              <span style={{ cursor: 'pointer', textDecoration: 'underline' }} onClick={() => { onClose(); onNavigate('settings'); }}>{t('config.restrictedAgentLink')}</span>
            </span>
          </div>
        )}

        {/* Workspace mode toggle — always shown when a project is selected.
            Previously hidden when `repo_url` was null/empty (non-git projects),
            but that made the option silently disappear for users who couldn't
            tell why. Now always visible: for non-git projects, Isolated mode
            is disabled with a hint explaining the requirement. */}
        {(() => {
          const selectedProj = projects.find(p => p.id === newDiscProjectId);
          if (!newDiscProjectId) return null; // no project → no workspace choice
          const hasRepo = Boolean(selectedProj?.repo_url);
          return (
            <div style={{ marginBottom: 12 }}>
              <label className="disc-form-label">{t('disc.workspaceLabel')}</label>
              <div className="disc-workspace-toggle">
                <button
                  type="button"
                  className="disc-workspace-btn"
                  data-active={newDiscWorkspaceMode === 'Direct'}
                  data-mode="direct"
                  onClick={() => { setNewDiscWorkspaceMode('Direct'); setNewDiscBranchName(''); }}
                >
                  <Folder size={12} />
                  <div>
                    <div className="disc-workspace-btn-title">{t('disc.workspaceDirect')}</div>
                    <div className="disc-workspace-btn-desc">{t('disc.workspaceDirectDesc')}</div>
                  </div>
                </button>
                <button
                  type="button"
                  disabled={!hasRepo}
                  onClick={() => {
                    if (!hasRepo) return;
                    setNewDiscWorkspaceMode('Isolated');
                    if (!newDiscBranchName) {
                      const title = newDiscTitle.trim();
                      const slug = title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
                      setNewDiscBranchName(slug || `disc-${Date.now()}`);
                    }
                  }}
                  className="disc-workspace-btn"
                  data-active={newDiscWorkspaceMode === 'Isolated'}
                  data-mode="isolated"
                  title={hasRepo ? undefined : t('disc.workspaceIsolatedNeedsRepo')}
                  style={!hasRepo ? { opacity: 0.5, cursor: 'not-allowed' } : undefined}
                >
                  <GitBranch size={12} />
                  <div>
                    <div className="disc-workspace-btn-title">{t('disc.workspaceIsolated')}</div>
                    <div className="disc-workspace-btn-desc">
                      {hasRepo ? t('disc.workspaceIsolatedDesc') : t('disc.workspaceIsolatedNeedsRepo')}
                    </div>
                  </div>
                </button>
              </div>
              {newDiscWorkspaceMode === 'Isolated' && hasRepo && (
                <div className="disc-workspace-branch-grid">
                  <div>
                    <label className="disc-form-label" data-size="xs">{t('disc.branchName')}</label>
                    <input
                      className="disc-input-styled"
                      value={newDiscBranchName}
                      onChange={e => setNewDiscBranchName(e.target.value)}
                      placeholder="feature/my-branch"
                      aria-label={t('disc.branchName')}
                    />
                  </div>
                  <div>
                    <label className="disc-form-label" data-size="xs">{t('disc.baseBranch')}</label>
                    <input
                      className="disc-input-styled"
                      value={newDiscBaseBranch}
                      onChange={e => setNewDiscBaseBranch(e.target.value)}
                      placeholder="main"
                      aria-label={t('disc.baseBranch')}
                    />
                  </div>
                </div>
              )}
            </div>
          );
        })()}

        {/* Advanced options (collapsible) */}
        {(availableSkills.length > 0 || availableProfiles.length > 0 || availableDirectives.length > 0) && (
          <div style={{ marginBottom: 12 }}>
            <button
              type="button"
              className="disc-advanced-toggle"
              onClick={() => setShowAdvancedOptions(prev => !prev)}
              aria-expanded={showAdvancedOptions}
              aria-label={t('disc.advancedOptions')}
            >
              <ChevronRight size={11} className="disc-chevron" data-expanded={showAdvancedOptions} />
              <Settings size={10} />
              {t('disc.advancedOptions')}
              {(newDiscSkillIds.length > 0 || newDiscProfileIds.length > 0 || newDiscDirectiveIds.length > 0 || newDiscTier !== 'default') && (
                <span className="disc-advanced-count">
                  ({newDiscSkillIds.length + newDiscProfileIds.length + newDiscDirectiveIds.length}{newDiscTier !== 'default' ? ` · ${newDiscTier === 'economy' ? '⚡' : '🧠'}` : ''})
                </span>
              )}
            </button>

            {showAdvancedOptions && (
              <div className="disc-advanced-panel">

                {/* Model tier selector */}
                <div className="disc-advanced-section">
                  <div className="disc-advanced-section-label">{t('disc.modelTier')}</div>
                  <div style={{ display: 'flex', gap: 4 }}>
                    {(['economy', 'default', 'reasoning'] as const).map(tier => (
                      <button key={tier} type="button" className="disc-tier-btn" data-active={newDiscTier === tier} data-tier={tier} onClick={() => setNewDiscTier(tier)}>
                        {tier === 'economy' ? '⚡' : tier === 'reasoning' ? '🧠' : '⚙️'} {t(`disc.tier.${tier}`)}
                      </button>
                    ))}
                  </div>
                </div>

                {/* Skills accordion */}
                {availableSkills.length > 0 && (
                  <div className="disc-advanced-section">
                    <button type="button" className="disc-advanced-section-toggle" onClick={() => setExpandedAdvanced(prev => prev === 'skills' ? null : 'skills')}>
                      <ChevronRight size={9} className="disc-chevron" data-expanded={expandedAdvanced === 'skills'} />
                      <Zap size={10} />
                      <span>{t('skills.selectSkills')}</span>
                      {newDiscSkillIds.length > 0 && <span className="disc-advanced-count">{newDiscSkillIds.length}</span>}
                    </button>
                    {expandedAdvanced === 'skills' && (
                      <div className="disc-advanced-chips">
                        {availableSkills.map(skill => {
                          const selected = newDiscSkillIds.includes(skill.id);
                          return (
                            <button key={skill.id} type="button" className="disc-chip" data-active={selected} data-color="accent"
                              onClick={() => setNewDiscSkillIds(prev => selected ? prev.filter(id => id !== skill.id) : [...prev, skill.id])}
                              title={skill.description || skill.name}
                            >
                              {selected && <Check size={9} />} {skill.name}
                            </button>
                          );
                        })}
                      </div>
                    )}
                  </div>
                )}

                {/* Profiles accordion */}
                {availableProfiles.length > 0 && (
                  <div className="disc-advanced-section" data-tour-id="disc-form-profiles">
                    <button
                      type="button"
                      className="disc-advanced-section-toggle"
                      data-tour-id="disc-form-profiles-toggle"
                      onClick={() => setExpandedAdvanced(prev => prev === 'profiles' ? null : 'profiles')}
                    >
                      <ChevronRight size={9} className="disc-chevron" data-expanded={expandedAdvanced === 'profiles'} />
                      <UserCircle size={10} />
                      <span>{t('profiles.select')}</span>
                      {newDiscProfileIds.length > 0 && <span className="disc-advanced-count">{newDiscProfileIds.length}</span>}
                    </button>
                    {expandedAdvanced === 'profiles' && (
                      <div className="disc-advanced-chips">
                        <button type="button" className="disc-chip" data-active={newDiscProfileIds.length === 0} data-color="purple" onClick={() => setNewDiscProfileIds([])}>
                          {t('profiles.none')}
                        </button>
                        {availableProfiles.map((profile, idx) => {
                          const selected = newDiscProfileIds.includes(profile.id);
                          return (
                            <ProfileTooltip key={profile.id} profile={profile}>
                              <button
                                type="button"
                                className="disc-chip"
                                data-active={selected}
                                data-color="purple"
                                // First chip gets a stable tour id so the
                                // guided tour can anchor on a real element.
                                {...(idx === 0 ? { 'data-tour-id': 'disc-form-profile-chip' } : {})}
                                onClick={() => setNewDiscProfileIds(prev => selected ? prev.filter(id => id !== profile.id) : [...prev, profile.id])}
                                style={selected && profile.color ? { borderColor: profile.color, background: `${profile.color}15`, color: profile.color } : undefined}
                              >
                                {selected && <Check size={9} />} {profile.avatar} {profile.persona_name || profile.name}
                              </button>
                            </ProfileTooltip>
                          );
                        })}
                      </div>
                    )}
                  </div>
                )}

                {/* Directives accordion */}
                {availableDirectives.length > 0 && (
                  <div className="disc-advanced-section">
                    <button type="button" className="disc-advanced-section-toggle" onClick={() => setExpandedAdvanced(prev => prev === 'directives' ? null : 'directives')}>
                      <ChevronRight size={9} className="disc-chevron" data-expanded={expandedAdvanced === 'directives'} />
                      <FileText size={10} />
                      <span>{t('directives.title')}</span>
                      {newDiscDirectiveIds.length > 0 && <span className="disc-advanced-count">{newDiscDirectiveIds.length}</span>}
                    </button>
                    {expandedAdvanced === 'directives' && (
                      <div className="disc-advanced-chips">
                        {availableDirectives.map(directive => {
                          const selected = newDiscDirectiveIds.includes(directive.id);
                          return (
                            <button key={directive.id} type="button" className="disc-chip" data-active={selected} data-color="warning"
                              onClick={() => setNewDiscDirectiveIds(prev => selected ? prev.filter(id => id !== directive.id) : [...prev, directive.id])}
                              title={directive.description || directive.name}
                            >
                              {selected && <Check size={9} />} {directive.icon} {directive.name}
                            </button>
                          );
                        })}
                      </div>
                    )}
                  </div>
                )}
              </div>
            )}
          </div>
        )}

        <label className="disc-form-label">{t('disc.title')}</label>
        <input
          className="disc-input-styled"
          data-locked={newDiscPrefilled}
          placeholder={t('disc.titlePlaceholder')}
          value={newDiscTitle}
          aria-label={t('disc.title')}
          onChange={e => {
            if (newDiscPrefilled) return;
            const val = e.target.value;
            setNewDiscTitle(val);
            if (newDiscWorkspaceMode === 'Isolated') {
              const slug = val.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
              setNewDiscBranchName(slug || `disc-${Date.now()}`);
            }
          }}
          readOnly={newDiscPrefilled}
        />

        <label className="disc-form-label" style={{ marginTop: 12 }}>{t('disc.prompt')}</label>
        <textarea
          className="disc-textarea-styled"
          data-locked={newDiscPrefilled}
          placeholder={t('disc.promptPlaceholder')}
          value={newDiscPrompt}
          aria-label={t('disc.prompt')}
          onChange={e => !newDiscPrefilled && setNewDiscPrompt(e.target.value)}
          readOnly={newDiscPrefilled}
          rows={4}
          autoFocus={!newDiscPrefilled}
        />

        {/* Context files */}
        <div className="disc-new-files-row">
          <input
            type="file"
            multiple
            style={{ display: 'none' }}
            ref={newDiscFileInputRef}
            aria-label={t('disc.attachFiles')}
            onChange={e => {
              const files = Array.from(e.target.files ?? []);
              if (files.length > 0) {
                setPendingFiles(prev => [...prev, ...files]);
              }
              e.target.value = '';
            }}
          />
          <button
            type="button"
            className="disc-new-attach-btn"
            onClick={() => newDiscFileInputRef.current?.click()}
          >
            <Paperclip size={12} /> {pendingFiles.length > 0 ? `${pendingFiles.length} ${t('disc.attachFile')}` : t('disc.attachFile')}
          </button>
          {pendingFiles.length > 0 && (
            <div className="disc-new-files-list">
              {pendingFiles.map((f, i) => (
                <span key={i} className="disc-context-file-badge">
                  {f.type.startsWith('image/') ? <Image size={10} /> : <FileText size={10} />}
                  <span className="disc-context-file-name">{f.name}</span>
                  <button className="disc-context-file-remove" onClick={() => setPendingFiles(prev => prev.filter((_, j) => j !== i))}>
                    <X size={9} />
                  </button>
                </span>
              ))}
            </div>
          )}
        </div>

        {/* Warnings for validation discussion */}
        {newDiscPrefilled && (
          <div className="disc-audit-warn">
            <p className="disc-audit-warn-title">
              <AlertTriangle size={11} /> {t('disc.auditWarn')}
            </p>
            <p className="disc-audit-warn-hint">
              {t('disc.auditHint')}
            </p>
          </div>
        )}

        <button
          className="disc-create-btn"
          data-ready={launchAgentNow ? !!newDiscPrompt.trim() : (!!newDiscPrompt.trim() || !!newDiscTitle.trim())}
          onClick={handleCreate}
          // 0.8.6 phase 2 — disc-first mode allows submitting WITHOUT a
          // prompt (just a title) since the agent will be invited later.
          // Launch mode keeps the legacy gates.
          disabled={
            creating ||
            (launchAgentNow
              ? !newDiscPrompt.trim() || !newDiscAgent
              : !newDiscPrompt.trim() && !newDiscTitle.trim())
          }
        >
          <MessageSquare size={14} /> {launchAgentNow ? t('disc.start') : t('disc.createEmpty')}
          <span className="disc-create-shortcut">Ctrl+Enter</span>
        </button>
      </div>
    </div>
  );
}
