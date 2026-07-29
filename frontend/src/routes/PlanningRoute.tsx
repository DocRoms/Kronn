import { useParams } from 'react-router';
import { useDashboardContext } from '../lib/dashboardContext';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { PlanningPage } from '../pages/PlanningPage';
import { ErrorBoundary } from '../components/ErrorBoundary';

export function PlanningRoute() {
  const { taskId } = useParams<{ taskId?: string }>();
  const ctx = useDashboardContext();
  const nav = useKronnNavigate();
  return (
    <ErrorBoundary mode="zone" label="Planning">
      <PlanningPage
        selectedTaskId={taskId ?? null}
        projects={ctx.projects}
        discussions={ctx.allDiscussions}
        toast={ctx.toast}
        onNavigateDiscussion={(discussionId) => nav.toDiscussion(discussionId)}
      />
    </ErrorBoundary>
  );
}
