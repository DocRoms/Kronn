import { useState } from 'react';
import { ListChecks, Loader2 } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { projects as projectsApi } from '../lib/api';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import type { Project } from '../types/generated';

export interface TdTasksButtonProps {
  project: Project;
  auditActive: boolean;
  toast: (msg: string, type: 'success' | 'error' | 'warning' | 'info') => void;
}

/**
 * « En faire des tâches Kronn » (KT-1167): the project's TD sheets become
 * planning tasks, one parent per theme. The sheets stay in the repository;
 * a second click only reports the tasks already linked.
 */
export function TdTasksButton({ project, auditActive, toast }: TdTasksButtonProps) {
  const { t } = useT();
  const [busy, setBusy] = useState(false);

  const convert = useAsyncGuard(async () => {
    setBusy(true);
    try {
      const outcome = await projectsApi.convertTdTasks(project.id);
      toast(
        t('audit.tdTasks.done', String(outcome.created), String(outcome.already_linked), String(outcome.skipped)),
        outcome.created > 0 ? 'success' : 'info',
      );
    } catch (e) {
      toast(t('audit.tdTasks.error', String(e instanceof Error ? e.message : e)), 'error');
    } finally {
      setBusy(false);
    }
  });

  if (auditActive || (project.tech_debt_count ?? 0) === 0) return null;

  return (
    <button
      type="button"
      className="dash-drift-update-btn"
      data-testid="project-td-tasks"
      disabled={busy}
      onClick={(e) => { e.stopPropagation(); void convert(); }}
      title={t('audit.tdTasks.hint')}
    >
      {busy ? <Loader2 size={9} className="spin" /> : <ListChecks size={9} />}
      {t('audit.tdTasks.button')}
    </button>
  );
}
