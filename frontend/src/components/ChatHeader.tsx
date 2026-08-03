import { useState, useEffect, useRef } from 'react';
import '../pages/DiscussionsPage.css';
import { discussions as discussionsApi } from '../lib/api';
import type {
  Project,
  AgentDetection,
  Discussion,
  AgentType,
  DiscussionWorkspace,
} from '../types/generated';
import { AGENT_MENTIONS, isUsable, isValidationDisc, isBriefingDisc, isBootstrapDisc } from '../lib/constants';
import type { ToastFn } from '../hooks/useToast';
import {
  GitBranch,
  Trash2,
  Pencil, ShieldCheck, Check, Zap, FileText, Settings, Rocket,
  Menu, Lock, Unlock, Star,
  FlaskConical, Info, UserCircle,
  ListTodo,
  Download,
  Loader2,
  Power,
  PowerOff,
  Search,
} from 'lucide-react';
import { MatrixText } from './MatrixText';
import { LearningsBadge } from './LearningsBadge';
import { DiscParticipantsHeader } from './DiscParticipantsHeader';
import { AgentSwitchPicker } from './AgentSwitchPicker';
import { DiscussionSessionBinding } from './DiscussionSessionBinding';
import { triggerDownload } from '../lib/downloadBlob';
import { ContextHelp } from './ContextHelp';

export interface ChatHeaderProps {
  discussion: Discussion;
  projects: Project[];
  agents: AgentDetection[];
  showGitPanel: boolean;
  showPlanPanel?: boolean;
  showSettingsPanel?: boolean;
  showMessageSearch?: boolean;
  planCompleted?: number;
  planTotal?: number;
  planLater?: number;
  pendingProposalCount?: number;
  pendingProposalItemCount?: number;
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
  onTogglePlanPanel?: () => void;
  onToggleSettingsPanel?: () => void;
  onToggleMessageSearch?: () => void;
  onToggleSidebar: () => void;
  onDelete: (discId: string) => void;
  onDiscussionUpdated: () => void;
  onAgentSwitch: (newAgent: AgentType) => void;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ChatHeader({
  discussion,
  projects,
  agents,
  showGitPanel,
  showPlanPanel = false,
  showSettingsPanel = false,
  showMessageSearch = false,
  planCompleted = 0,
  planTotal = 0,
  planLater = 0,
  pendingProposalCount = 0,
  pendingProposalItemCount = 0,
  isMobile,
  sending,
  pendingFilesCount,
  onRequestTestMode,
  onToggleGitPanel,
  onTogglePlanPanel,
  onToggleSettingsPanel,
  onToggleMessageSearch,
  onToggleSidebar,
  onDelete,
  onDiscussionUpdated,
  onAgentSwitch,
  toast,
  t,
}: ChatHeaderProps) {
  // Header-only state
  const [editingTitleId, setEditingTitleId] = useState<string | null>(null);
  const [editingTitleText, setEditingTitleText] = useState('');
  const [isDiscIdCopied, setIsDiscIdCopied] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [nativeAgentMode, setNativeAgentMode] = useState<{
    discussionId: string;
    disabled: boolean;
  } | null>(null);
  const [nativeAgentModeSaving, setNativeAgentModeSaving] = useState(false);
  const [sessionWorkspaces, setSessionWorkspaces] = useState<DiscussionWorkspace[]>([]);
  const exportInFlight = useRef(false);
  const nativeAgentModeInFlight = useRef(false);
  const discIdResetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => {
    if (discIdResetTimer.current) clearTimeout(discIdResetTimer.current);
  }, []);

  useEffect(() => {
    let current = true;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    const loadNativeAgentMode = () => {
      void discussionsApi.nativeAgentMode(discussion.id)
        .then(mode => {
          if (current) {
            setNativeAgentMode({ discussionId: discussion.id, disabled: mode.disabled });
          }
        })
        .catch(() => {
          // Never guess "enabled" after a transient backend failure: that
          // would make the header contradict the persisted no-agent routing
          // contract. Keep both actions disabled and retry the authoritative
          // backend state after reconnect.
          if (current) retryTimer = setTimeout(loadNativeAgentMode, 5_000);
        });
    };
    loadNativeAgentMode();
    return () => {
      current = false;
      if (retryTimer) clearTimeout(retryTimer);
    };
  }, [discussion.id]);

  useEffect(() => {
    let current = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const load = () => {
      // Partial API mocks are used by focused header tests, and a stale
      // hot-reloaded frontend can briefly run against the previous API object.
      // In both cases the declaration chips are optional decoration.
      if (typeof discussionsApi.workspaces !== 'function') return;
      void discussionsApi.workspaces(discussion.id)
        .then(workspaces => {
          if (!current) return;
          setSessionWorkspaces(
            workspaces.filter(workspace => workspace.session_pk !== null),
          );
        })
        .catch(() => {
          // A transient backend restart must not erase a previously rendered
          // declaration. The next poll refreshes the authoritative state.
        })
        .finally(() => {
          if (current) timer = setTimeout(load, 8_000);
        });
    };
    load();
    return () => {
      current = false;
      if (timer) clearTimeout(timer);
    };
  }, [discussion.id]);

  const copyDiscussionId = async () => {
    try {
      await navigator.clipboard.writeText(discussion.id);
      setIsDiscIdCopied(true);
      if (discIdResetTimer.current) clearTimeout(discIdResetTimer.current);
      discIdResetTimer.current = setTimeout(() => setIsDiscIdCopied(false), 1500);
      toast(t('disc.idCopied'), 'success');
    } catch {
      setIsDiscIdCopied(false);
      toast(t('disc.idCopyFailed'), 'error');
    }
  };

  const installedAgentsList = agents.filter(isUsable);
  const profileCount = discussion.profile_ids?.length ?? 0;
  const skillCount = discussion.skill_ids?.length ?? 0;
  const directiveCount = discussion.directive_ids?.length ?? 0;
  const hasConfiguredContext = profileCount + skillCount + directiveCount > 0;
  const nativeAgentDisabled = nativeAgentMode?.discussionId === discussion.id
    ? nativeAgentMode.disabled
    : null;

  const updateNativeAgentMode = async (disabled: boolean) => {
    if (nativeAgentModeInFlight.current || sending) return;
    nativeAgentModeInFlight.current = true;
    setNativeAgentModeSaving(true);
    try {
      await discussionsApi.update(discussion.id, { no_agent: disabled });
      setNativeAgentMode({ discussionId: discussion.id, disabled });
      onDiscussionUpdated();
      toast(
        t(disabled ? 'disc.nativeAgentDisabledToast' : 'disc.nativeAgentEnabledToast'),
        'success',
      );
    } catch (error) {
      toast(String(error), 'error');
    } finally {
      nativeAgentModeInFlight.current = false;
      setNativeAgentModeSaving(false);
    }
  };

  return (
    <div className="disc-chat-header" data-tour-id="disc-header-controls">
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
        <div className="disc-chat-header-top">
          <div className="disc-chat-header-title" data-tour-id="disc-identity-controls">
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
              className="disc-chat-header-title-text"
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
          {/* 0.8.5 — short disc-id pill. Surfaces the id so a user
              reading an agent summary like "Disc 3 — 04a9c927" can
              click → copy full UUID + paste it anywhere (next disc,
              linked-issue field, Slack message…). Sidebar search also
              matches id prefix in 0.8.5, so this works as a round-trip
              "agent quotes id → user finds disc in sidebar". Discreet
              ghost-text styling so it doesn't compete with the title. */}
          <button
            type="button"
            className="disc-id-pill"
            data-tour-id="discussion-id-pill"
            data-copied={isDiscIdCopied}
            onClick={(e) => {
              e.stopPropagation();
              void copyDiscussionId();
            }}
            title={t('disc.idPillTooltip', discussion.id)}
            aria-label={t('disc.idPillTooltip', discussion.id)}
          >
            {isDiscIdCopied ? <Check size={8} /> : null}
            #{discussion.id.slice(0, 8)}
          </button>
          <DiscussionSessionBinding discussionId={discussion.id} toast={toast} t={t} />
          <ContextHelp title={t('contextHelp.discussion.title')}>
            <p>{t('contextHelp.discussion.intro')}</p>
            <ul>
              <li>{t('contextHelp.discussion.mainAgent')}</li>
              <li>{t('contextHelp.discussion.participants')}</li>
              <li>{t('contextHelp.discussion.messages')}</li>
              <li>{t('contextHelp.discussion.outputs')}</li>
            </ul>
            <p className="kr-context-help-agent-note">{t('contextHelp.discussion.mcp')}</p>
          </ContextHelp>
          </div>
          <div className="disc-chat-header-presence">
            <DiscParticipantsHeader discId={discussion.id} toast={toast} t={t} />
          </div>
        </div>
        <div className="disc-chat-header-sub">
          <span className="disc-chat-context-project">
            {discussion.project_id
              ? (projects.find(p => p.id === discussion.project_id)?.name ?? '?')
              : t('disc.general')}
          </span>
          <span className="disc-native-agent-control">
            {nativeAgentDisabled ? (
              <button
                type="button"
                className="kr-agent-switch-btn disc-native-agent-disabled"
                data-testid="disc-native-agent-disabled"
                disabled={sending || nativeAgentModeSaving}
                title={t('disc.nativeAgentEnable')}
                aria-label={t('disc.nativeAgentEnable')}
                onClick={() => void updateNativeAgentMode(false)}
              >
                {nativeAgentModeSaving
                  ? <Loader2 size={9} className="spin" />
                  : <Power size={9} />}
                <span>{t('disc.nativeAgentDisabled')}</span>
              </button>
            ) : (
              <>
                <AgentSwitchPicker
                  currentAgent={discussion.agent}
                  availableAgents={installedAgentsList.map(agent => agent.agent_type)}
                  disabled={sending || nativeAgentDisabled === null || nativeAgentModeSaving}
                  title={t('disc.switchAgent')}
                  ariaLabel={t('disc.switchAgent')}
                  suffix={t('disc.targetDiscussionAgent')}
                  displayName={
                    AGENT_MENTIONS.find(mention => mention.type === discussion.agent)?.trigger
                    ?? discussion.agent
                  }
                  onChange={async agent => {
                    try {
                      await discussionsApi.update(discussion.id, { agent });
                      onAgentSwitch(agent);
                    } catch (err) {
                      toast(String(err), 'error');
                      throw err;
                    }
                  }}
                />
                <button
                  type="button"
                  className="disc-native-agent-toggle"
                  disabled={sending || nativeAgentDisabled === null || nativeAgentModeSaving}
                  title={t('disc.nativeAgentDisable')}
                  aria-label={t('disc.nativeAgentDisable')}
                  onClick={() => void updateNativeAgentMode(true)}
                >
                  {nativeAgentModeSaving
                    ? <Loader2 size={9} className="spin" />
                    : <PowerOff size={9} />}
                </button>
              </>
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
          {sessionWorkspaces.map(workspace => (
            <span
              key={workspace.id}
              className="disc-session-worktree-badge"
              data-state={workspace.state}
              title={[
                workspace.session_agent_type,
                workspace.workspace_path,
                workspace.head_sha?.slice(0, 10),
              ].filter(Boolean).join(' · ')}
            >
              <GitBranch size={8} />
              <span>{workspace.branch}</span>
              {workspace.task_reference && (
                <span className="disc-session-worktree-task">
                  {workspace.task_reference}
                </span>
              )}
              <span className="disc-session-worktree-agent">
                {workspace.session_agent_type ?? t('git.workspaceExternal')}
              </span>
            </span>
          ))}
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
          {hasConfiguredContext && (
            <button
              type="button"
              className="disc-chat-context-summary"
              onClick={onToggleSettingsPanel}
              title={t('disc.configuredContextSummary', profileCount, skillCount, directiveCount)}
              aria-label={t('disc.configuredContextSummary', profileCount, skillCount, directiveCount)}
            >
              {profileCount > 0 && <span><UserCircle size={10} /> {profileCount}</span>}
              {skillCount > 0 && <span><Zap size={10} /> {skillCount}</span>}
              {directiveCount > 0 && <span><FileText size={10} /> {directiveCount}</span>}
            </button>
          )}
        </div>
      </div>
      <div className="disc-chat-header-actions" data-tour-id="disc-output-controls">
        {/* 0.10.0 — pending-learnings badge (self-contained; hidden when 0). */}
        <LearningsBadge t={t} toast={toast} />
        {onToggleMessageSearch && (
          <button
            type="button"
            className="disc-icon-btn"
            data-active={showMessageSearch}
            onClick={onToggleMessageSearch}
            title={t('disc.messageSearch.open')}
            aria-label={t('disc.messageSearch.open')}
            aria-expanded={showMessageSearch}
          >
            <Search size={13} />
          </button>
        )}
        <button
          type="button"
          className="disc-icon-btn"
          data-active={showSettingsPanel}
          onClick={onToggleSettingsPanel}
          title={t('disc.settingsPanel')}
          aria-label={t('disc.settingsPanel')}
          aria-expanded={showSettingsPanel}
        >
          <Settings size={13} />
        </button>

        <button
          type="button"
          className="disc-icon-btn"
          disabled={exporting}
          onClick={async () => {
            if (exportInFlight.current) return;
            exportInFlight.current = true;
            setExporting(true);
            try {
              const { filename, blob } = await discussionsApi.exportDiscussion(discussion.id);
              triggerDownload(filename, blob);
              toast(t('disc.portability.exportDone'), 'success');
            } catch (error) {
              toast(t('disc.portability.exportError', String(error)), 'error');
            } finally {
              exportInFlight.current = false;
              setExporting(false);
            }
          }}
          title={t('disc.portability.exportHint')}
          aria-label={t('disc.portability.export')}
        >
          {exporting ? <Loader2 size={13} className="spin" /> : <Download size={13} />}
        </button>

        <button
          type="button"
          className="disc-plan-btn"
          data-active={showPlanPanel}
          onClick={onTogglePlanPanel}
          title={t('planning.openPlan')}
          aria-label={t('planning.openPlan')}
          aria-expanded={showPlanPanel}
        >
          <ListTodo size={13} />
          <span>{t('planning.short')}</span>
          <span className="disc-plan-count">{planCompleted}/{planTotal}</span>
          {planLater > 0 && <span className="disc-plan-later">+{planLater}</span>}
          {pendingProposalItemCount > 0 && (
            <span
              className="disc-plan-pending"
              title={t(
                'planning.pendingProposalTitle',
                pendingProposalCount,
                pendingProposalItemCount,
              )}
            >
              {pendingProposalItemCount}
            </span>
          )}
        </button>

        {discussion.project_id && (
          <button
            className="disc-icon-btn"
            data-active={showGitPanel}
            onClick={onToggleGitPanel}
            title={pendingFilesCount > 0
              ? t('git.pendingFilesTooltip', pendingFilesCount)
              : t('git.filesBtn')}
            aria-label={t('git.filesBtn')}
            aria-expanded={showGitPanel}
          >
            <GitBranch size={13} />
            {pendingFilesCount > 0 && (
              <span className="disc-icon-btn-badge" aria-label={t('git.pendingFilesTooltip', pendingFilesCount)}>
                {pendingFilesCount > 9 ? '9+' : pendingFilesCount}
              </span>
            )}
          </button>
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
