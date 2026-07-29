import { useEffect } from 'react';
import { useParams } from 'react-router';
import { Loader2 } from 'lucide-react';
import { useDashboardContext } from '../lib/dashboardContext';
import { ProjectList } from '../components/ProjectList';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { useT } from '../lib/I18nContext';

export function ProjectsRoute() {
  const { t } = useT();
  const { projectId } = useParams<{ projectId?: string }>();
  const ctx = useDashboardContext();
  // Depend on the setter, NOT on `ctx`: the outlet context is a plain object
  // rebuilt on every Dashboard render, so listing it would re-expand the card
  // on each one and fight the user collapsing it. `setExpandedId` is a
  // useState setter, hence stable.
  const { setExpandedId } = ctx;

  useEffect(() => {
    if (projectId) setExpandedId(projectId);
  }, [projectId, setExpandedId]);

  return (
    <ErrorBoundary mode="zone" label="Projects">
      {ctx.projectsLoading && (
        <div className="dash-loading-bar">
          <Loader2 size={14} className="spin" />
          <span className="text-sm text-muted">{t('projects.loading')}</span>
        </div>
      )}
      <ProjectList
        projects={ctx.projects}
        activeAudits={ctx.activeAudits}
        discussions={ctx.allDiscussions}
        discussionsByProject={ctx.discussionsByProject}
        driftByProject={ctx.driftByProject}
        agents={ctx.agents}
        allSkills={ctx.allSkills}
        mcpConfigs={ctx.mcpOverview.configs}
        workflows={ctx.workflowList}
        configLanguage={ctx.configLanguage}
        toast={ctx.toast}
        onSetDiscPrefill={ctx.setDiscPrefill}
        onRefetch={ctx.refetch}
        onRefetchDiscussions={ctx.refetchDiscussions}
        onRefetchSkills={ctx.refetchSkills}
        onRefetchDrift={ctx.handleRefetchDrift}
        expandedId={ctx.expandedId}
        onSetExpandedId={ctx.setExpandedId}
      />
    </ErrorBoundary>
  );
}
