import { useCallback, useLayoutEffect, useMemo, useRef } from 'react';
import { useParams, useSearchParams } from 'react-router';
import { ProjectList } from '../components/ProjectList';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { useDashboardContext } from '../lib/dashboardContext';
import { projectLocation, type DashboardPage, type ProjectLocation } from '../lib/routes';

export function ProjectsRoute() {
  const ctx = useDashboardContext();
  const nav = useKronnNavigate();
  // The address owns the open project: `/projects/<id>` is the selection.
  const { projectId, projectView } = useParams<{ projectId?: string; projectView?: string }>();
  const expandedId = projectId ?? null;
  // `/projects/<id>/<view>[?file=…&line=…|?folder=…]`: the view open in the
  // workspace, and what it shows. A bare `/projects/<id>` opens on the view
  // the reader used last.
  const [search] = useSearchParams();
  const location = useMemo(() => projectLocation(projectView, search), [projectView, search]);
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
  // Picking a view is a step Back can undo; naming what the view opened on
  // its own is not.
  const followLocation = useCallback((next: ProjectLocation, options?: { replace?: boolean }) => {
    if (addressed.current) nav.toProject(addressed.current, { ...next, replace: options?.replace });
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
      projectLocation={location}
      onProjectLocationChange={followLocation}
    />
  );
}
