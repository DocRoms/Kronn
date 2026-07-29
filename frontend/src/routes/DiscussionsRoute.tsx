import { useParams } from 'react-router';
import { useDashboardContext } from '../lib/dashboardContext';
import { DiscussionsPage } from '../pages/DiscussionsPage';
import { ErrorBoundary } from '../components/ErrorBoundary';

export function DiscussionsRoute() {
  const { discussionId } = useParams<{ discussionId?: string }>();
  const ctx = useDashboardContext();
  return (
    <ErrorBoundary mode="zone" label="Discussions">
      <DiscussionsPage
        projects={ctx.projects}
        agents={ctx.agents}
        allDiscussions={ctx.allDiscussions}
        configLanguage={ctx.configLanguage}
        agentAccess={ctx.agentAccess}
        refetchDiscussions={ctx.refetchDiscussions}
        refetchProjects={ctx.refetch}
        prefill={ctx.discPrefill}
        onPrefillConsumed={ctx.handlePrefillConsumed}
        onSetDiscPrefill={ctx.setDiscPrefill}
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
        onActiveDiscussionChange={ctx.setActiveDiscussionId}
        initialActiveDiscussionId={discussionId ?? ctx.activeDiscussionId}
        lastSeenMsgCount={ctx.lastSeenMsgCount}
        mcpConfigs={ctx.mcpOverview.configs}
        mcpIncompatibilities={ctx.mcpOverview.incompatibilities}
      />
    </ErrorBoundary>
  );
}
