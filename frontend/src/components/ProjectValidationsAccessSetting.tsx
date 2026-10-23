// KT-1433 — how far a project's integration validations reach beyond the lane sandbox.
import { useCallback, useEffect, useState } from 'react';
import { Loader2, ShieldAlert } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { userError } from '../lib/userError';
import type { ProjectValidationsAccess, ValidationsAccess } from '../types/generated';

interface Props {
  projectId: string;
  t: (key: string, ...args: (string | number)[]) => string;
}

const LEVELS: ValidationsAccess[] = ['sandbox', 'docker', 'full'];

export function ProjectValidationsAccessSetting({ projectId, t }: Props) {
  const [state, setState] = useState<ProjectValidationsAccess | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Full access is only applied once the warning was read and confirmed.
  const [confirmingFull, setConfirmingFull] = useState(false);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => projectsApi.validationsAccess(projectId))
      .then(value => { if (live) setState(value); })
      .catch(e => { if (live) setError(userError(e)); });
    return () => { live = false; };
  }, [projectId]);

  const apply = useCallback(async (access: ValidationsAccess) => {
    setSaving(true);
    setError(null);
    try {
      setState(await projectsApi.setValidationsAccess(projectId, access));
      setConfirmingFull(false);
    } catch (e) {
      setError(userError(e));
    } finally {
      setSaving(false);
    }
  }, [projectId]);

  const choose = (access: ValidationsAccess) => {
    if (state?.access === access) return;
    if (access === 'full') {
      setConfirmingFull(true);
      return;
    }
    setConfirmingFull(false);
    void apply(access);
  };

  return (
    <section className="project-agent-files" data-testid="project-validations-access">
      <h4>
        <ShieldAlert size={14} aria-hidden="true" /> {t('projects.validationsAccess.title')}
        {saving && <Loader2 size={12} className="spin" aria-hidden="true" />}
      </h4>
      <p className="project-agent-files-help">{t('projects.validationsAccess.help')}</p>
      {LEVELS.map(level => (
        <label key={level} className="project-agent-files-option" data-selected={state?.access === level}>
          <input
            type="radio"
            name={`validations-access-${projectId}`}
            checked={state?.access === level}
            disabled={!state || saving}
            onChange={() => choose(level)}
            data-testid={`project-validations-access-${level}`}
          />
          <span>
            <strong>{t(`projects.validationsAccess.${level}`)}</strong>
            <small>{t(`projects.validationsAccess.${level}Hint`)}</small>
          </span>
        </label>
      ))}
      {state && state.access !== 'full' && !confirmingFull && (state.docker_socket || state.docker_error) && (
        <p className="project-agent-files-where" data-testid="project-validations-access-socket">
          {state.docker_socket
            ? <>{t('projects.validationsAccess.socket')} <code>{state.docker_socket}</code></>
            : t('projects.validationsAccess.noSocket', state.docker_error ?? '')}
        </p>
      )}
      {confirmingFull && (
        <div className="project-validations-access-warning" role="alertdialog" aria-labelledby={`validations-full-${projectId}`} data-testid="project-validations-access-full-warning">
          <strong id={`validations-full-${projectId}`}>{t('projects.validationsAccess.fullWarningTitle')}</strong>
          <p>{t('projects.validationsAccess.fullWarning')}</p>
          <div className="project-validations-access-warning-actions">
            <button type="button" className="btn btn-secondary" onClick={() => setConfirmingFull(false)} disabled={saving}>
              {t('projects.validationsAccess.cancel')}
            </button>
            <button type="button" className="btn btn-danger" onClick={() => void apply('full')} disabled={saving} data-testid="project-validations-access-full-confirm">
              {t('projects.validationsAccess.confirmFull')}
            </button>
          </div>
        </div>
      )}
      {error && <p role="alert" className="project-agent-files-error">{error}</p>}
    </section>
  );
}
