import { useState, useEffect } from 'react';
import '../pages/DiscussionsPage.css';
import { discussions as discussionsApi } from '../lib/api';
import type { Project, AgentDetection, Discussion, AgentType, Skill, AgentProfile, Directive, McpConfigDisplay, McpIncompatibility, Contact } from '../types/generated';
import { agentColor, isHiddenPath, isUsable, isValidationDisc } from '../lib/constants';
import type { ToastFn } from '../hooks/useToast';
import {
  Cpu, GitBranch, Server,
  Trash2,
  Pencil, ShieldCheck, Check, Zap, FileText, Settings, Rocket,
  Menu, Lock, Unlock, RefreshCw, Share2, Users2, Star,
  FlaskConical, Info,
} from 'lucide-react';
import { MatrixText } from './MatrixText';
import { ProfileTooltip } from './ProfileTooltip';

const isBootstrapDisc = (title: string) => title.startsWith('Bootstrap: ');
const isBriefingDisc = (title: string) => title.startsWith('Briefing');

export interface ChatHeaderProps {
  discussion: Discussion;
  projects: Project[];
  agents: AgentDetection[];
  availableSkills: Skill[];
  availableProfiles: AgentProfile[];
  availableDirectives: Directive[];
  mcpConfigs: McpConfigDisplay[];
  mcpIncompatibilities: McpIncompatibility[];
  showGitPanel: boolean;
  isMobile: boolean;
  sending: boolean;
  /// Number of uncommitted files in the discussion worktree (Isolated mode
  /// only — caller passes 0 for Direct mode). Drives the badge on the
  /// git-panel icon; nudges the user to commit when the agent didn't.
  pendingFilesCount: number;
  /// User-friendly "Tester cette version" CTA: parent owns the call so it
  /// can open the preflight modal if the server returns a blocker.
  onRequestTestMode: () => void;
  onToggleGitPanel: () => void;
  onToggleSidebar: () => void;
  onDelete: (discId: string) => void;
  onDiscussionUpdated: () => void;
  onAgentSwitch: (newAgent: AgentType) => void;
  contacts: Contact[];
  onShare: (contactIds: string[]) => void;
  toast: ToastFn;
  t: (key: string, ...args: any[]) => string;
}

export function ChatHeader({
  discussion,
  projects,
  agents,
  availableSkills,
  availableProfiles,
  availableDirectives,
  mcpConfigs,
  mcpIncompatibilities,
  showGitPanel,
  isMobile,
  sending,
  pendingFilesCount,
  onRequestTestMode,
  onToggleGitPanel,
  onToggleSidebar,
  onDelete,
  onDiscussionUpdated,
  onAgentSwitch,
  contacts,
  onShare,
  toast,
  t,
}: ChatHeaderProps) {
  // Header-only state
  const [editingTitleId, setEditingTitleId] = useState<string | null>(null);
  const [showSharePopover, setShowSharePopover] = useState(false);
  const [editingTitleText, setEditingTitleText] = useState('');
  const [showMcpPopover, setShowMcpPopover] = useState(false);
  const [mcpSearchFilter, setMcpSearchFilter] = useState('');
  const [showProfileEditor, setShowProfileEditor] = useState(false);
  const [showAgentSwitch, setShowAgentSwitch] = useState(false);
  // Which inline badge popover is currently open, if any. Encoded as
  // "type:id" (e.g. "profile:default-architect", "skill:bootstrap-architect")
  // so we only need one useState for the whole header sub-row.
  const [openBadgeInfo, setOpenBadgeInfo] = useState<string | null>(null);

  // Close the badge info popover on click-outside and on Escape. The
  // click-outside check walks up from the event target until it finds
  // a `.disc-badge-wrap` parent; if none, we close. This lets clicks
  // *inside* the popover (e.g. on a link) pass through.
  useEffect(() => {
    if (!openBadgeInfo) return;
    const onClick = (e: MouseEvent) => {
      const el = e.target as Element | null;
      if (!el?.closest('.disc-badge-wrap')) setOpenBadgeInfo(null);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpenBadgeInfo(null);
    };
    document.addEventListener('click', onClick);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('click', onClick);
      document.removeEventListener('keydown', onKey);
    };
  }, [openBadgeInfo]);
  const toggleBadgeInfo = (key: string) => (e: React.MouseEvent) => {
    e.stopPropagation();
    setOpenBadgeInfo(prev => (prev === key ? null : key));
  };

  const installedAgentsList = agents.filter(isUsable);

  return (
    <div className="disc-chat-header">
      {isMobile && (
        <button
          className="disc-mobile-sidebar-btn"
          onClick={onToggleSidebar}
          aria-label="Open sidebar"
        >
          <Menu size={18} />
        </button>
      )}
      <div className="disc-chat-header-info">
        <div className="disc-chat-header-title">
          {/* Pin / favorite toggle — always visible in the header so the user
              can pin from inside the conversation. Outline = not pinned,
              filled yellow = pinned. Sidebar shows the result in its
              "Favorites" section at the top. */}
          <button
            type="button"
            className="disc-pin-header-btn"
            onClick={async () => {
              try {
                await discussionsApi.update(discussion.id, { pinned: !discussion.pinned });
                onDiscussionUpdated();
              } catch { /* silent — toast from parent */ }
            }}
            title={discussion.pinned ? t('disc.unpin') : t('disc.pin')}
            aria-label={discussion.pinned ? t('disc.unpin') : t('disc.pin')}
            aria-pressed={discussion.pinned}
          >
            <Star
              size={14}
              style={discussion.pinned
                ? { color: 'var(--kr-warning)', fill: 'var(--kr-warning)' }
                : { color: 'var(--kr-text-ghost)' }}
            />
          </button>
          {isValidationDisc(discussion.title) && <ShieldCheck size={14} className="text-accent flex-shrink-0" />}
          {isBriefingDisc(discussion.title) && <Zap size={14} className="text-info flex-shrink-0" />}
          {isBootstrapDisc(discussion.title) && <Rocket size={14} className="text-accent flex-shrink-0" />}
          {editingTitleId === discussion.id && !isValidationDisc(discussion.title) && !isBootstrapDisc(discussion.title) && !isBriefingDisc(discussion.title) ? (
            <input
              autoFocus
              className="disc-title-input"
              value={editingTitleText}
              onChange={e => setEditingTitleText(e.target.value)}
              onKeyDown={async e => {
                if (e.key === 'Enter' && editingTitleText.trim()) {
                  const newTitle = editingTitleText.trim();
                  await discussionsApi.update(discussion.id, { title: newTitle });
                  setEditingTitleId(null);
                  onDiscussionUpdated();
                }
                if (e.key === 'Escape') setEditingTitleId(null);
              }}
              onBlur={async () => {
                if (editingTitleText.trim() && editingTitleText.trim() !== discussion.title) {
                  const newTitle = editingTitleText.trim();
                  await discussionsApi.update(discussion.id, { title: newTitle });
                  onDiscussionUpdated();
                }
                setEditingTitleId(null);
              }}
            />
          ) : (
            <span
              style={{ cursor: (isValidationDisc(discussion.title) || isBootstrapDisc(discussion.title) || isBriefingDisc(discussion.title)) ? 'default' : 'pointer' }}
              onDoubleClick={() => {
                if (isValidationDisc(discussion.title) || isBootstrapDisc(discussion.title) || isBriefingDisc(discussion.title)) return;
                setEditingTitleId(discussion.id);
                setEditingTitleText(discussion.title);
              }}
              title={(isValidationDisc(discussion.title) || isBootstrapDisc(discussion.title) || isBriefingDisc(discussion.title)) ? undefined : t('disc.editTitle')}
            >
              <MatrixText text={discussion.title} />
            </span>
          )}
          {!isValidationDisc(discussion.title) && !isBootstrapDisc(discussion.title) && !isBriefingDisc(discussion.title) && (
          <button
            className="disc-icon-btn"
            style={{ padding: '2px 4px', border: 'none', background: 'none', color: 'var(--kr-text-ghost)' }}
            onClick={() => {
              if (editingTitleId === discussion.id) {
                setEditingTitleId(null);
              } else {
                setEditingTitleId(discussion.id);
                setEditingTitleText(discussion.title);
              }
            }}
            title={t('disc.editTitle')}
            aria-label={t('disc.editTitle')}
          >
            <Pencil size={10} />
          </button>
          )}
        </div>
        <div className="disc-chat-header-sub">
          <span>{discussion.project_id ? (projects.find(p => p.id === discussion.project_id)?.name ?? '?') : t('disc.general')} · </span>
          <span className="relative flex-row gap-1">
            <button
              className="disc-agent-switch-btn"
              onClick={() => setShowAgentSwitch(prev => !prev)}
              disabled={sending}
              title={t('disc.switchAgent')}
              style={{ color: agentColor(discussion.agent) }}
            >
              {discussion.agent} <RefreshCw size={8} className="opacity-50" />
            </button>
            {showAgentSwitch && (
              <div className="disc-agent-switch-popover">
                {installedAgentsList.map(a => (
                  <button
                    key={a.agent_type}
                    disabled={a.agent_type === discussion.agent}
                    className="disc-agent-switch-item"
                    data-current={a.agent_type === discussion.agent}
                    onClick={async () => {
                      setShowAgentSwitch(false);
                      try {
                        await discussionsApi.update(discussion.id, { agent: a.agent_type });
                        onAgentSwitch(a.agent_type);
                      } catch (err) {
                        toast(String(err), 'error');
                      }
                    }}
                  >
                    <Cpu size={10} style={{ color: agentColor(a.agent_type) }} />
                    {a.name}
                    {a.agent_type === discussion.agent && <Check size={10} style={{ marginLeft: 'auto', color: 'var(--kr-accent-ink)' }} />}
                  </button>
                ))}
              </div>
            )}
          </span>
          {discussion.workspace_mode === 'Isolated' && discussion.worktree_branch && (
            <span className="disc-worktree-badge" data-locked={!!discussion.workspace_path}>
              <GitBranch size={8} /> {discussion.worktree_branch}
              <span className="opacity-50 text-2xs">{discussion.workspace_path ? 'worktree' : t('disc.worktreeUnlocked')}</span>
              <button
                className="disc-worktree-lock-btn"
                title={discussion.workspace_path ? t('disc.worktreeUnlock') : t('disc.worktreeLock')}
                onClick={async (e) => {
                  e.stopPropagation();
                  try {
                    if (discussion.workspace_path) {
                      await discussionsApi.worktreeUnlock(discussion.id);
                    } else {
                      await discussionsApi.worktreeLock(discussion.id);
                    }
                    onDiscussionUpdated();
                  } catch (err) {
                    toast(String(err), 'error');
                  }
                }}
              >
                {discussion.workspace_path ? <Unlock size={9} /> : <Lock size={9} />}
              </button>
            </span>
          )}
          {/* Test-mode CTA — only while the worktree is active and we're
              not already testing. Hidden in Direct mode (no branch to swap)
              and while in test mode (global banner is the exit path). */}
          {discussion.workspace_mode === 'Isolated'
            && discussion.worktree_branch
            && !!discussion.workspace_path
            && !discussion.test_mode_restore_branch && (
            <button
              className="disc-test-mode-btn"
              onClick={onRequestTestMode}
              title={t('testMode.ctaTooltip')}
            >
              <FlaskConical size={11} />
              <span>{t('testMode.cta')}</span>
              <span className="disc-test-mode-btn-hint" aria-hidden="true">
                <Info size={9} />
              </span>
            </button>
          )}
          {(discussion.profile_ids?.length ?? 0) > 0 && (
            <>
              <span className="disc-separator">·</span>
              {discussion.profile_ids?.map((pid: string) => {
                const p = availableProfiles.find(p => p.id === pid);
                if (!p) return null;
                const key = `profile:${pid}`;
                const isOpen = openBadgeInfo === key;
                return (
                  <span key={pid} className="disc-badge-wrap">
                    <button
                      type="button"
                      className="disc-header-profile-badge"
                      style={{ background: `${p.color}15`, color: p.color, border: `1px solid ${p.color}30`, cursor: 'pointer' }}
                      onClick={toggleBadgeInfo(key)}
                      aria-expanded={isOpen}
                      title={t('disc.badgeInfo.showDetails')}
                    >
                      {p.avatar} {p.persona_name || p.name}
                    </button>
                    {isOpen && (
                      <div className="disc-badge-info-popover" role="dialog">
                        <div className="disc-badge-info-header">
                          <span className="disc-badge-info-avatar" style={{ background: `${p.color}20`, color: p.color }}>
                            {p.avatar}
                          </span>
                          <div>
                            <div className="disc-badge-info-title">{p.persona_name || p.name}</div>
                            <div className="disc-badge-info-subtitle">{p.role}</div>
                          </div>
                        </div>
                        <div className="disc-badge-info-kind">{t('disc.badgeInfo.profileKind')}</div>
                        <div className="disc-badge-info-body">
                          {p.persona_prompt.length > 400
                            ? p.persona_prompt.slice(0, 400) + '…'
                            : p.persona_prompt}
                        </div>
                        <div className="disc-badge-info-footer">
                          ~{p.token_estimate} {t('disc.badgeInfo.tokens')}
                          {p.is_builtin && <span className="disc-badge-info-pill">{t('disc.badgeInfo.builtin')}</span>}
                        </div>
                      </div>
                    )}
                  </span>
                );
              })}
            </>
          )}
          {(discussion.skill_ids ?? []).length > 0 && (
            <>
              <span className="disc-separator">·</span>
              {(discussion.skill_ids ?? []).map(sid => {
                const skill = availableSkills.find(s => s.id === sid);
                const key = `skill:${sid}`;
                const isOpen = openBadgeInfo === key;
                return (
                  <span key={sid} className="disc-badge-wrap">
                    <button
                      type="button"
                      className="disc-header-skill-badge"
                      style={{ cursor: 'pointer' }}
                      onClick={toggleBadgeInfo(key)}
                      aria-expanded={isOpen}
                      title={t('disc.badgeInfo.showDetails')}
                    >
                      {skill?.icon ? `${skill.icon} ` : ''}{skill?.name ?? sid}
                    </button>
                    {isOpen && skill && (
                      <div className="disc-badge-info-popover" role="dialog">
                        <div className="disc-badge-info-header">
                          <span className="disc-badge-info-avatar">{skill.icon || '📘'}</span>
                          <div>
                            <div className="disc-badge-info-title">{skill.name}</div>
                            <div className="disc-badge-info-subtitle">{skill.category}</div>
                          </div>
                        </div>
                        <div className="disc-badge-info-kind">{t('disc.badgeInfo.skillKind')}</div>
                        <div className="disc-badge-info-body">
                          {skill.description}
                        </div>
                        <div className="disc-badge-info-footer">
                          ~{skill.token_estimate} {t('disc.badgeInfo.tokens')}
                          {skill.is_builtin && <span className="disc-badge-info-pill">{t('disc.badgeInfo.builtin')}</span>}
                        </div>
                      </div>
                    )}
                    {isOpen && !skill && (
                      <div className="disc-badge-info-popover" role="dialog">
                        <div className="disc-badge-info-title">{sid}</div>
                        <div className="disc-badge-info-body">{t('disc.badgeInfo.notFound')}</div>
                      </div>
                    )}
                  </span>
                );
              })}
            </>
          )}
          {(discussion.directive_ids ?? []).length > 0 && (
            <>
              <span className="disc-separator">·</span>
              {(discussion.directive_ids ?? []).map(id => {
                const d = availableDirectives.find(dd => dd.id === id);
                const key = `directive:${id}`;
                const isOpen = openBadgeInfo === key;
                return (
                  <span key={id} className="disc-badge-wrap">
                    <button
                      type="button"
                      className="disc-header-directive-badge"
                      style={{ cursor: 'pointer' }}
                      onClick={toggleBadgeInfo(key)}
                      aria-expanded={isOpen}
                      title={t('disc.badgeInfo.showDetails')}
                    >
                      <FileText size={7} style={{ marginRight: 2 }} />
                      {d ? `${d.icon} ${d.name}` : id}
                    </button>
                    {isOpen && d && (
                      <div className="disc-badge-info-popover" role="dialog">
                        <div className="disc-badge-info-header">
                          <span className="disc-badge-info-avatar">{d.icon || '📜'}</span>
                          <div>
                            <div className="disc-badge-info-title">{d.name}</div>
                            <div className="disc-badge-info-subtitle">{d.category}</div>
                          </div>
                        </div>
                        <div className="disc-badge-info-kind">{t('disc.badgeInfo.directiveKind')}</div>
                        <div className="disc-badge-info-body">
                          {d.description}
                        </div>
                        <div className="disc-badge-info-footer">
                          ~{d.token_estimate} {t('disc.badgeInfo.tokens')}
                          {d.is_builtin && <span className="disc-badge-info-pill">{t('disc.badgeInfo.builtin')}</span>}
                        </div>
                      </div>
                    )}
                  </span>
                );
              })}
            </>
          )}
        </div>
      </div>
      <div className="disc-chat-header-actions">
        {/* MCP info button */}
        <div className="relative">
          <button
            className="disc-icon-btn" style={{ color: showMcpPopover ? 'var(--kr-cyan)' : undefined }}
            onClick={() => { setShowMcpPopover(prev => { if (prev) setMcpSearchFilter(''); return !prev; }); setShowProfileEditor(false); }}
            title={t('disc.mcps')}
            aria-label={t('disc.mcps')}
          >
            <Server size={13} />
          </button>
          {showMcpPopover && (() => {
            const discMcps = discussion.project_id
              ? mcpConfigs.filter(c => c.is_global || c.project_ids.includes(discussion.project_id!))
              : mcpConfigs.filter(c => c.include_general);
            // Agents running via direct API (no CLI) cannot use MCP tools
            const apiOnlyAgents: AgentType[] = ['Vibe' as AgentType];
            const isApiOnly = apiOnlyAgents.includes(discussion.agent);
            const filterLower = mcpSearchFilter.toLowerCase();
            const filteredMcps = filterLower
              ? discMcps.filter(c => c.label.toLowerCase().includes(filterLower) || c.server_name.toLowerCase().includes(filterLower))
              : discMcps;
            return (
              <div className="disc-mcp-popover">
                <div className="disc-mcp-header">
                  {t('disc.mcps')}
                  <span className="disc-mcp-header-count">{discMcps.length}</span>
                </div>
                {discMcps.length > 6 && (
                  <div className="disc-mcp-search-wrap">
                    <input
                      type="text"
                      value={mcpSearchFilter}
                      onChange={e => setMcpSearchFilter(e.target.value)}
                      placeholder={t('disc.mcpSearch')}
                      className="disc-mcp-search"
                      autoFocus
                    />
                  </div>
                )}
                {isApiOnly && (
                  <div className="disc-mcp-api-notice">
                    <span className="text-xs">⚡</span>
                    Mode API — MCPs indisponibles
                  </div>
                )}
                <div className="disc-mcp-list">
                  {filteredMcps.length === 0 ? (
                    <div className="disc-mcp-empty">{mcpSearchFilter ? t('disc.noMcps') : t('disc.noMcps')}</div>
                  ) : filteredMcps.map(c => {
                    const incomp = mcpIncompatibilities.find(
                      i => i.server_id === c.server_id && i.agent === discussion.agent
                    );
                    return (
                      <div
                        key={c.id}
                        title={incomp ? `\u26a0 ${incomp.reason}` : isApiOnly ? 'Non disponible en mode API' : undefined}
                        className="disc-mcp-item"
                        style={{
                          color: incomp ? 'var(--kr-error)' : isApiOnly ? 'var(--kr-text-ghost)' : 'var(--kr-text-primary)',
                          opacity: incomp ? 0.7 : isApiOnly ? 0.5 : 1,
                        }}
                      >
                        <Server size={9} style={{ color: incomp ? 'var(--kr-error)' : isApiOnly ? 'var(--kr-text-ghost)' : 'var(--kr-cyan)' }} className="flex-shrink-0" />
                        {c.label}
                        {incomp && <span className="disc-mcp-incompatible">incompatible</span>}
                        <span className="disc-mcp-item-name">{c.server_name}</span>
                      </div>
                    );
                  })}
                </div>
              </div>
            );
          })()}
        </div>

        {/* Edit profiles/skills button */}
        <div className="relative">
          <button
            className="disc-icon-btn" style={{ color: showProfileEditor ? 'var(--kr-purple-soft)' : undefined }}
            onClick={() => { setShowProfileEditor(prev => !prev); setShowMcpPopover(false); }}
            title={t('disc.editConfig')}
            aria-label={t('disc.editConfig')}
          >
            <Settings size={13} />
          </button>
          {showProfileEditor && (
            <div className="disc-profile-popover">
              {/* Project */}
              <div className="disc-popover-section">
                <div className="disc-popover-label">{t('disc.project')}</div>
                <select
                  className="disc-popover-select"
                  value={discussion.project_id ?? ''}
                  onChange={async (e) => {
                    // Send "" (not null) for "General" — serde can't distinguish
                    // JSON null from absent field with Option<Option<String>>
                    const newPid = e.target.value;
                    try {
                      await discussionsApi.update(discussion.id, { project_id: newPid || '' });
                      onDiscussionUpdated();
                    } catch (err) {
                      console.error('Failed to update project:', err);
                    }
                  }}
                >
                  <option value="">{t('disc.general')}</option>
                  {projects.filter(p => !isHiddenPath(p.path)).map(p => (
                    <option key={p.id} value={p.id}>{p.name}</option>
                  ))}
                </select>
              </div>

              {/* Profiles */}
              {availableProfiles.length > 0 && (
                <div className="disc-popover-section">
                  <div className="disc-popover-label">{t('profiles.select')}</div>
                  <div className="flex-wrap gap-2">
                    {availableProfiles.map(profile => {
                      const active = (discussion.profile_ids ?? []).includes(profile.id);
                      return (
                        <ProfileTooltip key={profile.id} profile={profile}>
                          <button
                            className="disc-toggle-pill"
                            data-active={active}
                            data-color="purple"
                            style={{
                              borderColor: active ? (profile.color || 'rgba(var(--kr-purple-rgb), 0.4)') : undefined,
                              background: active ? `${profile.color}15` : undefined,
                              color: active ? (profile.color || 'var(--kr-purple-soft)') : undefined,
                            }}
                            onClick={async () => {
                              const current = discussion.profile_ids ?? [];
                              const next = active ? current.filter((id: string) => id !== profile.id) : [...current, profile.id];
                              await discussionsApi.update(discussion.id, { profile_ids: next });
                              onDiscussionUpdated();
                            }}>
                            {active && <Check size={8} />}
                            {profile.avatar} {profile.persona_name || profile.name}
                          </button>
                        </ProfileTooltip>
                      );
                    })}
                  </div>
                </div>
              )}

              {/* Skills */}
              {availableSkills.length > 0 && (
                <div className="disc-popover-section">
                  <div className="disc-popover-label">{t('skills.selectSkills')}</div>
                  <div className="flex-wrap gap-2">
                    {availableSkills.map(skill => {
                      const active = (discussion.skill_ids ?? []).includes(skill.id);
                      return (
                        <button key={skill.id}
                          className="disc-toggle-pill"
                          data-active={active}
                          data-color="accent"
                          onClick={async () => {
                            const current = discussion.skill_ids ?? [];
                            const next = active ? current.filter((id: string) => id !== skill.id) : [...current, skill.id];
                            await discussionsApi.update(discussion.id, { skill_ids: next });
                            onDiscussionUpdated();
                          }}>
                          {active && <Check size={8} />}
                          {skill.name}
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}

              {/* Model Tier */}
              <div className="disc-popover-section">
                <div className="disc-popover-label">{t('disc.modelTier')}</div>
                <div className="flex-row gap-2">
                  {(['economy', 'default', 'reasoning'] as const).map(tier => {
                    const active = (discussion.tier ?? 'default') === tier;
                    return (
                      <button key={tier}
                        className="disc-toggle-pill"
                        data-active={active}
                        data-tier={tier}
                        onClick={async () => {
                          await discussionsApi.update(discussion.id, { tier });
                          onDiscussionUpdated();
                        }}>
                        {tier === 'economy' ? '⚡' : tier === 'reasoning' ? '\ud83e\udde0' : '\u2699\ufe0f'} {t(`disc.tier.${tier}`)}
                      </button>
                    );
                  })}
                </div>
              </div>

              {/* Directives */}
              {availableDirectives.length > 0 && (
                <div>
                  <div className="disc-popover-label">{t('directives.title')}</div>
                  <div className="flex-wrap gap-2">
                    {availableDirectives.map(directive => {
                      const active = (discussion.directive_ids ?? []).includes(directive.id);
                      return (
                        <button key={directive.id}
                          className="disc-toggle-pill"
                          data-active={active}
                          data-color="warning"
                          onClick={async () => {
                            const current = discussion.directive_ids ?? [];
                            const next = active ? current.filter((id: string) => id !== directive.id) : [...current, directive.id];
                            await discussionsApi.update(discussion.id, { directive_ids: next });
                            onDiscussionUpdated();
                          }}>
                          {active && <Check size={8} />}
                          {directive.icon} {directive.name}
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}
            </div>
          )}
        </div>

        {discussion.project_id && (
          <button
            className="disc-icon-btn" style={{ color: showGitPanel ? 'var(--kr-accent-ink)' : undefined }}
            onClick={onToggleGitPanel}
            title={pendingFilesCount > 0
              ? t('git.pendingFilesTooltip', pendingFilesCount)
              : t('git.filesBtn')}
            aria-label={t('git.filesBtn')}
          >
            <GitBranch size={13} />
            {pendingFilesCount > 0 && (
              <span className="disc-icon-btn-badge" aria-label={t('git.pendingFilesTooltip', pendingFilesCount)}>
                {pendingFilesCount > 9 ? '9+' : pendingFilesCount}
              </span>
            )}
          </button>
        )}
        {/* Share button */}
        {contacts.length > 0 && (
          <div style={{ position: 'relative' }}>
            <button
              className="disc-icon-btn"
              onClick={() => setShowSharePopover(!showSharePopover)}
              style={{ color: discussion.shared_id ? 'var(--kr-success)' : undefined }}
              title={discussion.shared_id ? t('contacts.wsConnected') : 'Share'}
            >
              {discussion.shared_id ? <Users2 size={12} /> : <Share2 size={12} />}
            </button>
            {showSharePopover && (
              <div className="disc-popover" style={{ right: 0, top: '100%', minWidth: 200 }}>
                <div className="text-sm font-semibold mb-3">Share with</div>
                {contacts.map(c => {
                  const alreadyShared = discussion.shared_with?.includes(c.id);
                  return (
                    <button
                      key={c.id}
                      className="disc-popover-row"
                      style={{ opacity: alreadyShared ? 0.5 : 1 }}
                      onClick={() => {
                        if (!alreadyShared) {
                          onShare([c.id]);
                          setShowSharePopover(false);
                        }
                      }}
                      disabled={alreadyShared}
                    >
                      <span className="text-sm">{c.pseudo}</span>
                      {alreadyShared && <Check size={10} style={{ color: 'var(--kr-success)' }} />}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        )}
        <button
          className="disc-icon-btn" style={{ color: 'var(--kr-error)' }}
          onClick={() => onDelete(discussion.id)}
          aria-label="Delete discussion"
        >
          <Trash2 size={12} />
        </button>
      </div>
    </div>
  );
}
