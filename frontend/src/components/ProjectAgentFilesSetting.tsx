// KT-971 — whether Kronn may write its agent files into a project's repository.
import { useCallback, useEffect, useState } from 'react';
import { FolderLock, Loader2 } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { userError } from '../lib/userError';
import type { AgentFilesPolicy, ProjectAgentFiles } from '../types/generated';

interface Props {
  projectId: string;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ProjectAgentFilesSetting({ projectId, t }: Props) {
  const [state, setState] = useState<ProjectAgentFiles | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    // Inside the promise: a failure here stays in this setting, never in the card.
    Promise.resolve()
      .then(() => projectsApi.agentFiles(projectId))
      .then(value => { if (live) setState(value); })
      .catch(e => { if (live) setError(userError(e)); });
    return () => { live = false; };
  }, [projectId]);

  const choose = useCallback(async (policy: AgentFilesPolicy) => {
    if (state?.policy === policy) return;
    setSaving(true);
    setError(null);
    try {
      setState(await projectsApi.setAgentFiles(projectId, policy));
    } catch (e) {
      setError(userError(e));
    } finally {
      setSaving(false);
    }
  }, [projectId, state?.policy]);

  const option = (policy: AgentFilesPolicy) => (
    <label className="project-agent-files-option" data-selected={state?.policy === policy}>
      <input
        type="radio"
        name={`agent-files-${projectId}`}
        checked={state?.policy === policy}
        disabled={!state || saving}
        onChange={() => void choose(policy)}
        data-testid={`project-agent-files-${policy}`}
      />
      <span>
        <strong>{t(`projects.agentFiles.${policy}`)}</strong>
        <small>{t(`projects.agentFiles.${policy}Hint`)}</small>
      </span>
    </label>
  );

  return (
    <section className="project-agent-files" data-testid="project-agent-files">
      <h4>
        <FolderLock size={14} aria-hidden="true" /> {t('projects.agentFiles.title')}
        {saving && <Loader2 size={12} className="spin" aria-hidden="true" />}
      </h4>
      <p className="project-agent-files-help">{t('projects.agentFiles.help')}</p>
      {option('repo')}
      {option('outside')}
      {state?.policy === 'outside' && state.outside_dir && (
        <p className="project-agent-files-where" data-testid="project-agent-files-where">
          {t('projects.agentFiles.where')} <code>{state.outside_dir}</code>
        </p>
      )}
      {state?.cleaned?.length ? (
        <p className="project-agent-files-cleaned" data-testid="project-agent-files-cleaned">
          {t('projects.agentFiles.cleaned', state.cleaned.join(', '))}
        </p>
      ) : null}
      {error && <p role="alert" className="project-agent-files-error">{error}</p>}
    </section>
  );
}
