import { useT } from '../../lib/I18nContext';
import type { WorkflowProjectScope } from '../../types/generated';

type Mode = 'home' | 'all' | 'chosen';

interface Props {
  /** `null` = the workflow serves its home project only. */
  value: WorkflowProjectScope | null;
  onChange: (scope: WorkflowProjectScope | null) => void;
  projects: Array<{ id: string; name: string }>;
  /** The project whose repository carries the workflow; always served. */
  homeProjectId: string;
}

function modeOf(value: WorkflowProjectScope | null): Mode {
  if (!value) return 'home';
  return value.type === 'All' ? 'all' : 'chosen';
}

/** KT-851 — which projects a workflow serves; the run's project is chosen at trigger time. */
export function WorkflowProjectScopeControl({ value, onChange, projects, homeProjectId }: Props) {
  const { t } = useT();
  const mode = modeOf(value);
  const chosen = value?.type === 'Projects' ? value.project_ids : [];
  const others = projects.filter(project => project.id !== homeProjectId);

  const selectMode = (next: Mode) => {
    if (next === 'home') onChange(null);
    else if (next === 'all') onChange({ type: 'All' });
    else onChange({ type: 'Projects', project_ids: chosen });
  };
  const toggle = (projectId: string, served: boolean) => {
    const ids = served ? [...chosen, projectId] : chosen.filter(id => id !== projectId);
    onChange({ type: 'Projects', project_ids: ids });
  };

  return (
    <fieldset className="wf-scope mt-6" data-testid="workflow-project-scope">
      <legend className="wf-label">{t('wiz.projectScope')}</legend>
      <div className="wf-scope-options">
        {(['home', 'all', 'chosen'] as Mode[]).map(option => (
          <label key={option} className="wf-checkbox-label">
            <input
              type="radio"
              name="workflow-project-scope"
              checked={mode === option}
              onChange={() => selectMode(option)}
            />
            {t(`wiz.projectScope.${option}`)}
          </label>
        ))}
      </div>
      {mode === 'chosen' && (
        <div className="wf-scope-projects">
          {others.length === 0 && <p className="wf-field-hint">{t('wiz.projectScope.none')}</p>}
          {others.map(project => (
            <label key={project.id} className="wf-checkbox-label">
              <input
                type="checkbox"
                checked={chosen.includes(project.id)}
                onChange={e => toggle(project.id, e.target.checked)}
              />
              {project.name}
            </label>
          ))}
        </div>
      )}
      <p className="wf-field-hint wf-scope-hint">
        {t(homeProjectId ? 'wiz.projectScope.hint' : 'wiz.projectScope.hintGlobal')}
      </p>
    </fieldset>
  );
}
