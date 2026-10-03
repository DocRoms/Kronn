import { useCallback, useEffect, useRef, useState } from 'react';
import { projects as projectsApi } from '../lib/api';
import type { DriftCheckResponse, Project } from '../types/generated';

/**
 * Drift per audited project, for the Projects page only. Drift hashes every
 * mapped source of a project: it is asked once per project and audit state
 * while that page is open, not on each refresh of the project list.
 */
export function useProjectDrift(active: boolean, projects: Project[]) {
  const [driftByProject, setDriftByProject] = useState<Record<string, DriftCheckResponse>>({});
  const requested = useRef(new Set<string>());

  useEffect(() => {
    if (!active) return;
    for (const project of projects) {
      if (project.audit_status !== 'Audited' && project.audit_status !== 'Validated') continue;
      const key = `${project.id}:${project.audit_status}`;
      if (requested.current.has(key)) continue;
      requested.current.add(key);
      projectsApi.checkDrift(project.id).then(drift => {
        if (drift) setDriftByProject(prev => ({ ...prev, [project.id]: drift }));
      }).catch(() => requested.current.delete(key));
    }
  }, [active, projects]);

  /** After a partial audit: that project's drift changed for sure. */
  const refetchDrift = useCallback((projectId: string) => {
    projectsApi.checkDrift(projectId).then(drift => {
      if (drift) setDriftByProject(prev => ({ ...prev, [projectId]: drift }));
    }).catch(() => {});
  }, []);

  return { driftByProject, refetchDrift };
}
