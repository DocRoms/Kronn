// KT-1344 — what the discussion header's third row used to fold.
//
// The header's "Details" button opens the side panel on its settings tab, so
// the id, the cost, the project and the worktree state live there: one place
// for the details instead of a fold that repeated the panel. The session
// worktrees are polled here, while the section is on screen, rather than by a
// header that no longer shows them.
import { useEffect, useRef, useState } from 'react';
import { Check, FlaskConical, GitBranch, Info, Lock, Unlock } from 'lucide-react';
import { discussions as discussionsApi } from '../lib/api';
import type { Discussion, DiscussionWorkspace, Project } from '../types/generated';
import type { ToastFn } from '../hooks/useToast';
import { DiscussionSessionBinding } from './DiscussionSessionBinding';
import { DiscussionTokenCost } from './DiscussionTokenCost';
import { GithubConnectionChip } from './project/ProjectGithubRow';

export interface DiscussionDetailsSectionProps {
  discussion: Discussion;
  projects: Project[];
  onRequestTestMode: () => void;
  onDiscussionUpdated: () => void;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function DiscussionDetailsSection({
  discussion, projects, onRequestTestMode, onDiscussionUpdated, toast, t,
}: DiscussionDetailsSectionProps) {
  const [isDiscIdCopied, setIsDiscIdCopied] = useState(false);
  const [sessionWorkspaces, setSessionWorkspaces] = useState<DiscussionWorkspace[]>([]);
  const discIdResetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => {
    if (discIdResetTimer.current) clearTimeout(discIdResetTimer.current);
  }, []);

  useEffect(() => {
    let current = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const load = () => {
      // Partial API mocks are used by focused tests, and a stale hot-reloaded
      // frontend can briefly run against the previous API object. In both
      // cases the declaration chips are optional decoration.
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

  const isolatedBranch = discussion.workspace_mode === 'Isolated' && discussion.worktree_branch;

  return (
    <section
      className="disc-settings-section disc-settings-details"
      data-testid="disc-details-section"
    >
      <h3>{t('disc.headerDetails')}</h3>
      <div className="disc-settings-details-row">
        {/* 0.8.5 — short disc-id pill. Surfaces the id so a user reading an
            agent summary like "Disc 3 — 04a9c927" can click → copy the full
            UUID and paste it anywhere (next disc, linked-issue field, Slack
            message…). Sidebar search also matches id prefix. */}
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
      </div>
      <div className="disc-settings-details-row">
        {/* KT-254 — what this room cost, as two figures. */}
        <DiscussionTokenCost discussionId={discussion.id} t={t} />
        <span className="disc-chat-context-project">
          {discussion.project_id
            ? (projects.find(p => p.id === discussion.project_id)?.name ?? '?')
            : t('disc.general')}
        </span>
        {discussion.project_id && <GithubConnectionChip projectId={discussion.project_id} />}
      </div>
      {(isolatedBranch || sessionWorkspaces.length > 0) && (
        <div className="disc-settings-details-row">
          {isolatedBranch && (
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
        </div>
      )}
      {/* Test-mode CTA — only while the worktree is active and we're not
          already testing. Hidden in Direct mode (no branch to swap) and while
          in test mode (global banner is the exit path). */}
      {isolatedBranch
        && !!discussion.workspace_path
        && !discussion.test_mode_restore_branch && (
        <div className="disc-settings-details-row">
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
        </div>
      )}
    </section>
  );
}
