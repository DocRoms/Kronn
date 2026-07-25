import { useState, useEffect, useRef } from 'react';
import '../pages/DiscussionsPage.css';
import { discussions as discussionsApi } from '../lib/api';
import type { Project, AgentDetection, Discussion, AgentType } from '../types/generated';
import { isUsable, isValidationDisc, isBriefingDisc, isBootstrapDisc } from '../lib/constants';
import type { ToastFn } from '../hooks/useToast';
import {
  GitBranch,
  Trash2,
  Pencil, ShieldCheck, Check, Zap, FileText, Settings, Rocket,
  Menu, Lock, Unlock, Star,
  FlaskConical, Info, UserCircle,
  ListTodo,
} from 'lucide-react';
import { MatrixText } from './MatrixText';
import { LearningsBadge } from './LearningsBadge';
import { DiscParticipantsHeader } from './DiscParticipantsHeader';
import { AgentSwitchPicker } from './AgentSwitchPicker';

export interface ChatHeaderProps {
  discussion: Discussion;
  projects: Project[];
  agents: AgentDetection[];
  showGitPanel: boolean;
  showPlanPanel?: boolean;
  showSettingsPanel?: boolean;
  planCompleted?: number;
  planTotal?: number;
  planLater?: number;
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
  planCompleted = 0,
  planTotal = 0,
  planLater = 0,
  isMobile,
  sending,
  pendingFilesCount,
  onRequestTestMode,
  onToggleGitPanel,
  onTogglePlanPanel,
  onToggleSettingsPanel,
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
  const discIdResetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => {
    if (discIdResetTimer.current) clearTimeout(discIdResetTimer.current);
  }, []);

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
        <div className="disc-chat-header-top">
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
          <span className="relative flex-row gap-1">
            <AgentSwitchPicker
              currentAgent={discussion.agent}
              availableAgents={installedAgentsList.map(agent => agent.agent_type)}
              disabled={sending}
              title={t('disc.switchAgent')}
              ariaLabel={t('disc.switchAgent')}
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
      <div className="disc-chat-header-actions">
        {/* 0.10.0 — pending-learnings badge (self-contained; hidden when 0). */}
        <LearningsBadge t={t} toast={toast} />
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
