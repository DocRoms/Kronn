import { useCallback, useLayoutEffect, useRef } from 'react';
import { useParams } from 'react-router';
import { ProjectList } from '../components/ProjectList';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { useDashboardContext } from '../lib/dashboardContext';
import type { DashboardPage } from '../lib/routes';

export function ProjectsRoute() {
  const ctx = useDashboardContext();
  const nav = useKronnNavigate();
  // The address owns the open project: `/projects/<id>` is the selection.
  const { projectId } = useParams<{ projectId?: string }>();
  const expandedId = projectId ?? null;
  // Picking a project is a step Back can undo. Landing on the bare list, the
  // page opens its first project on its own: that one replaces the address
  // instead, so Back does not bounce the reader between the two.
  const addressed = useRef(expandedId);
  useLayoutEffect(() => { addressed.current = expandedId; }, [expandedId]);
  const selectProject = useCallback((id: string | null) => {
    if (id === addressed.current) return;
    if (id) nav.toProject(id, { replace: addressed.current === null });
    else nav.toPage('projects');
  }, [nav]);
  return (
    <ProjectList
      projects={ctx.projects}
      loading={ctx.projectsLoading}
      favoritesReady={ctx.projectsLoaded}
      activeAudits={ctx.activeAudits}
      discussions={ctx.allDiscussions}
      discussionsByProject={ctx.discussionsByProject}
      driftByProject={ctx.driftByProject}
      agents={ctx.agents}
      allSkills={ctx.allSkills}
      mcpConfigs={ctx.mcpOverview.configs}
      workflows={ctx.workflowList}
      configLanguage={ctx.configLanguage}
      modelTiers={ctx.agentAccess?.model_tiers ?? null}
      toast={ctx.toast}
      onAddProject={ctx.openAddProject}
      onNavigate={(target) => {
        if (target.startsWith('mcps:')) {
          nav.toPlugin(target.slice('mcps:'.length));
        } else if (target.startsWith('planning:')) {
          nav.toPlanningTask(target.slice('planning:'.length));
        } else {
          nav.toPage(target as DashboardPage);
        }
      }}
      onSetDiscPrefill={ctx.setDiscPrefill}
      onAutoRunDiscussion={(discussionId) => nav.toDiscussion(discussionId, { autoRun: true })}
      onOpenDiscussion={nav.toDiscussion}
      onRefetch={ctx.refetchProjects}
      onRefetchDiscussions={ctx.refetchDiscussions}
      onRefetchSkills={ctx.refetchSkills}
      onRefetchDrift={ctx.refetchDrift}
      expandedId={expandedId}
      onSetExpandedId={selectProject}
    />
  );
}
