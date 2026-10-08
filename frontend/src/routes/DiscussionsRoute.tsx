import { useCallback, useLayoutEffect, useMemo, useRef } from 'react';
import { useLocation, useParams, useSearchParams } from 'react-router';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { useDashboardContext } from '../lib/dashboardContext';
import { DISCUSSION_MESSAGE_PARAM, type DashboardPage, type DiscussionsIntent } from '../lib/routes';
import { DiscussionsPage } from '../pages/DiscussionsPage';
import { useLocationIntent } from './useLocationIntent';

export function DiscussionsRoute() {
  const ctx = useDashboardContext();
  const nav = useKronnNavigate();
  // The address owns the open discussion: `/discussions/<id>?message=<id>`.
  // A bare `/discussions` reopens where the previous visit left off.
  // `/discussions/compare/<runId>`: a comparison has the main area.
  const { discussionId, compareRunId } = useParams<{ discussionId?: string; compareRunId?: string }>();
  const [search] = useSearchParams();
  // New on every navigation: renews the request below (see `addressToken`).
  const location = useLocation();
  const messageId = discussionId ? search.get(DISCUSSION_MESSAGE_PARAM) : null;
  const [intent, consumeIntent] = useLocationIntent<DiscussionsIntent>();
  const { setDiscPrefill, setActiveDiscussionId } = ctx;
  // An arrival from Planning: this discussion's Git panel, on a workspace.
  const gitWorkspaceId = intent?.gitWorkspaceId;
  const gitWorkspaceTarget = useMemo(
    () => (gitWorkspaceId && discussionId ? { discussionId, workspaceId: gitWorkspaceId } : null),
    [gitWorkspaceId, discussionId],
  );
  // The page lists these acknowledgements in effect dependencies: they must
  // keep their identity across the shell's renders.
  const handlePrefillConsumed = useCallback(() => setDiscPrefill(null), [setDiscPrefill]);
  // Picking a discussion is a step Back can undo; the one the page restores
  // on its own replaces the bare address instead. The address is read through
  // a ref: the page lists this callback in an effect's dependencies.
  const addressed = useRef(discussionId);
  useLayoutEffect(() => { addressed.current = discussionId; }, [discussionId]);
  const comparing = useRef(compareRunId);
  useLayoutEffect(() => { comparing.current = compareRunId; }, [compareRunId]);
  const handleActiveDiscussionChange = useCallback((id: string | null) => {
    setActiveDiscussionId(id);
    // Under a comparison, the discussion the page keeps behind it is not the address.
    if (comparing.current) return;
    const current = addressed.current;
    if (id === (current ?? null)) return;
    if (id) nav.toDiscussion(id, { replace: current === undefined });
    else nav.toPage('discussions');
  }, [setActiveDiscussionId, nav]);
  // Opening a comparison is a step Back can undo; closing one returns to the
  // discussions, where the page names the discussion it then shows.
  const handleCompareChange = useCallback((runId: string | null) => {
    if (runId === (comparing.current ?? null)) return;
    if (runId) nav.toDiscussionCompare(runId);
    else nav.toPage('discussions');
  }, [nav]);
  // The address is the selection: nothing to acknowledge once it is open.
  const noop = useCallback(() => {}, []);
  return (
    <DiscussionsPage
      projects={ctx.projects}
      agents={ctx.agents}
      allDiscussions={ctx.allDiscussions}
      configLanguage={ctx.configLanguage}
      agentAccess={ctx.agentAccess}
      refetchDiscussions={ctx.refetchDiscussions}
      refetchProjects={ctx.refetchProjects}
      onNavigate={(target, opts) => {
        // The requested page wins: a project id only opens that project when
        // Projects is where the caller goes (the dev kickoff names its project
        // but stays on Discussions, where its prefilled form opens).
        if (target === 'projects' && opts?.projectId) nav.toProject(opts.projectId, opts.projectAt);
        // A Configuration section has its own address, scrolled to on arrival.
        else if (target === 'settings' && opts?.scrollTo) {
          nav.toSettingsSection(opts.scrollTo, opts.modelTier ? { modelTier: opts.modelTier } : undefined);
        }
        // Already on Discussions: nothing to navigate (the dev kickoff only
        // prefills a new discussion, whose form opens over the current one).
        else if (target === 'discussions') return;
        else if (!opts?.workflowId && !(target === 'workflows' && opts?.quickPromptId)) nav.toPage(target as DashboardPage);
        // Sidebar batch pastille → the parent workflow's own address.
        if (opts?.workflowId) nav.toWorkflow(opts.workflowId);
        // An improved Quick Prompt: its address, flashed on arrival.
        else if (target === 'workflows' && opts?.quickPromptId) {
          nav.toAutomation({ tab: 'quickPrompts', resourceId: opts.quickPromptId }, { intent: { highlightQuickPrompt: true } });
        }
      }}
      prefill={ctx.discPrefill}
      onPrefillConsumed={handlePrefillConsumed}
      onSetDiscPrefill={ctx.setDiscPrefill}
      autoRunDiscussionId={intent?.autoRun ? discussionId ?? null : null}
      onAutoRunConsumed={consumeIntent}
      onLaunchWorkflowFromPreset={(presetId, projectId) => nav.toWorkflows({ preset: { presetId, projectId } })}
      openDiscussionId={intent?.autoRun ? null : discussionId ?? null}
      addressToken={location}
      compareRunId={compareRunId ?? null}
      onCompareChange={handleCompareChange}
      gitWorkspaceTarget={gitWorkspaceTarget}
      onGitWorkspaceConsumed={consumeIntent}
      onOpenDiscConsumed={noop}
      focusBatchId={intent?.focusBatch?.id ?? null}
      focusBatchMode={intent?.focusBatch?.mode ?? 'batch'}
      onFocusBatchConsumed={consumeIntent}
      toast={ctx.toast}
      sendingMap={ctx.sendingMap}
      setSendingMap={ctx.setSendingMap}
      queuedMap={ctx.queuedMap}
      setQueuedMap={ctx.setQueuedMap}
      sendingStartMap={ctx.sendingStartMap}
      setSendingStartMap={ctx.setSendingStartMap}
      streamingMap={ctx.streamingMap}
      setStreamingMap={ctx.setStreamingMap}
      noteStreamTick={ctx.noteStreamTick}
      abortControllers={ctx.abortControllers}
      cleanupStream={ctx.cleanupStream}
      markDiscussionSeen={ctx.markDiscussionSeen}
      markAllDiscussionsSeen={ctx.markAllDiscussionsSeen}
      onActiveDiscussionChange={handleActiveDiscussionChange}
      initialActiveDiscussionId={discussionId ?? ctx.restorableDiscussionId}
      initialMessageId={messageId}
      lastSeenMsgCount={ctx.lastSeenMsgCount}
      mcpConfigs={ctx.mcpOverview.configs}
      mcpIncompatibilities={ctx.mcpOverview.incompatibilities}
    />
  );
}
