import { useState } from 'react';
import '../pages/DiscussionsPage.css';
import { discussions as discussionsApi } from '../lib/api';
import type { Project, AgentDetection, Discussion, AgentType, Skill, AgentProfile, Directive, McpConfigDisplay, McpIncompatibility, Contact } from '../types/generated';
import { agentColor, isHiddenPath, isUsable, isValidationDisc } from '../lib/constants';
import type { ToastFn } from '../hooks/useToast';
import {
  Cpu, GitBranch, Server,
  Trash2,
  Pencil, ShieldCheck, Check, Zap, FileText, Settings, Rocket,
  Menu, Lock, Unlock, RefreshCw, Share2, Users2,
} from 'lucide-react';

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
              {discussion.title}
            </span>
          )}
          {!isValidationDisc(discussion.title) && !isBootstrapDisc(discussion.title) && !isBriefingDisc(discussion.title) && (
          <button
            className="disc-icon-btn"
            style={{ padding: '2px 4px', border: 'none', background: 'none', color: 'rgba(255,255,255,0.2)' }}
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
                    {a.agent_type === discussion.agent && <Check size={10} style={{ marginLeft: 'auto', color: '#c8ff00' }} />}
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
          {(discussion.profile_ids?.length ?? 0) > 0 && (
            <>
              <span className="disc-separator">·</span>
              {discussion.profile_ids?.map((pid: string) => {
                const p = availableProfiles.find(p => p.id === pid);
                return p ? (
                  <span key={pid} className="disc-header-profile-badge" style={{ background: `${p.color}15`, color: p.color, border: `1px solid ${p.color}30` }}>
                    {p.avatar} {p.persona_name || p.name}
                  </span>
                ) : null;
              })}
            </>
          )}
          {(discussion.skill_ids ?? []).length > 0 && (
            <>
              <span className="disc-separator">·</span>
              {(discussion.skill_ids ?? []).map(sid => {
                const skill = availableSkills.find(s => s.id === sid);
                return (
                  <span key={sid} className="disc-header-skill-badge">
                    {skill?.name ?? sid}
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
                return (
                  <span key={id} className="disc-header-directive-badge">
                    <FileText size={7} style={{ marginRight: 2 }} />
                    {d ? `${d.icon} ${d.name}` : id}
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
            className="disc-icon-btn" style={{ color: showMcpPopover ? '#00d4ff' : undefined }}
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
                          color: incomp ? '#ff6b6b' : isApiOnly ? 'rgba(255,255,255,0.25)' : '#e8eaed',
                          opacity: incomp ? 0.7 : isApiOnly ? 0.5 : 1,
                        }}
                      >
                        <Server size={9} style={{ color: incomp ? '#ff6b6b' : isApiOnly ? 'rgba(255,255,255,0.2)' : '#00d4ff' }} className="flex-shrink-0" />
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
            className="disc-icon-btn" style={{ color: showProfileEditor ? '#a78bfa' : undefined }}
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
                    const newPid = e.target.value || null;
                    await discussionsApi.update(discussion.id, { project_id: newPid });
                    onDiscussionUpdated();
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
                        <button key={profile.id} title={profile.role}
                          className="disc-toggle-pill"
                          data-active={active}
                          data-color="purple"
                          style={{
                            borderColor: active ? (profile.color || 'rgba(139,92,246,0.4)') : undefined,
                            background: active ? `${profile.color}15` : undefined,
                            color: active ? (profile.color || '#a78bfa') : undefined,
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
            className="disc-icon-btn" style={{ color: showGitPanel ? '#c8ff00' : undefined }}
            onClick={onToggleGitPanel}
            title={t('git.filesBtn')}
            aria-label={t('git.filesBtn')}
          >
            <GitBranch size={13} />
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
          className="disc-icon-btn" style={{ color: '#ff4d6a' }}
          onClick={() => onDelete(discussion.id)}
          aria-label="Delete discussion"
        >
          <Trash2 size={12} />
        </button>
      </div>
    </div>
  );
}
