import { useCallback, useLayoutEffect, useRef } from 'react';
import { useParams } from 'react-router';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { useDashboardContext } from '../lib/dashboardContext';
import { PlanningPage } from '../pages/PlanningPage';

export function PlanningRoute() {
  const ctx = useDashboardContext();
  const nav = useKronnNavigate();
  // The address owns the open task: `/planning/<id>` is the selection, and
  // picking or closing one is a step Back can undo.
  const { taskId } = useParams<{ taskId?: string }>();
  const addressed = useRef(taskId);
  useLayoutEffect(() => { addressed.current = taskId; }, [taskId]);
  const selectTask = useCallback((id: string | null) => {
    if (id === (addressed.current ?? null)) return;
    if (id) nav.toPlanningTask(id);
    else nav.toPage('planning');
  }, [nav]);
  // The page is memoized: a stable callback keeps the Dashboard's polls from
  // re-rendering it (KT-1148).
  const navigateDiscussion = useCallback(
    (discussionId: string, options?: { gitWorkspaceId?: string }) => nav.toDiscussion(discussionId, { gitWorkspaceId: options?.gitWorkspaceId }),
    [nav],
  );
  return (
    <PlanningPage
      selectedTaskId={taskId ?? null}
      onSelectedTaskChange={selectTask}
      projects={ctx.projects}
      discussions={ctx.allDiscussions}
      toast={ctx.toast}
      onNavigateDiscussion={navigateDiscussion}
    />
  );
}
