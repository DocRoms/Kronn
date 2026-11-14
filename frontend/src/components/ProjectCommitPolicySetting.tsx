// KT-1750 — who the commits Kronn makes for a project are attributed to, and
// whether they carry a Signed-off-by. Both default to what the repository itself
// says (shown here, with where it comes from); the owner overrides them per project.
import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, GitCommitHorizontal, Loader2 } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { userError } from '../lib/userError';
import type { ProjectCommitPolicy, SignoffSetting } from '../types/generated';

interface Props {
  projectId: string;
  t: (key: string, ...args: (string | number)[]) => string;
}

const SIGNOFF: SignoffSetting[] = ['auto', 'always', 'never'];

export function ProjectCommitPolicySetting({ projectId, t }: Props) {
  const [policy, setPolicy] = useState<ProjectCommitPolicy | null>(null);
  const [name, setName] = useState('');
  const [email, setEmail] = useState('');
  const [signoff, setSignoff] = useState<SignoffSetting>('auto');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const show = useCallback((value: ProjectCommitPolicy) => {
    setPolicy(value);
    setName(value.settings.name ?? '');
    setEmail(value.settings.email ?? '');
    setSignoff(value.settings.signoff);
  }, []);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => projectsApi.commitPolicy(projectId))
      .then(value => { if (live) show(value); })
      .catch(e => { if (live) setError(userError(e)); });
    return () => { live = false; };
  }, [projectId, show]);

  const half = (name.trim() === '') !== (email.trim() === '');
  const unchanged = policy !== null
    && name.trim() === (policy.settings.name ?? '')
    && email.trim() === (policy.settings.email ?? '')
    && signoff === policy.settings.signoff;

  const save = async (next: { name: string; email: string; signoff: SignoffSetting }) => {
    setSaving(true);
    setError(null);
    try {
      // The workspace is saved whole: the worktree hooks go back as they were.
      const current = await projectsApi.workspace(projectId);
      const commit = {
        name: next.name.trim() === '' ? null : next.name.trim(),
        email: next.email.trim() === '' ? null : next.email.trim(),
        signoff: next.signoff,
      };
      await projectsApi.setWorkspace(projectId, { hooks: current?.hooks ?? {}, commit });
      show(await projectsApi.commitPolicy(projectId));
    } catch (e) {
      setError(userError(e));
    } finally {
      setSaving(false);
    }
  };

  const detected = policy?.detected;
  const effective = policy?.effective;

  return (
    <section className="project-agent-files" data-testid="project-commit-policy">
      <h4>
        <GitCommitHorizontal size={14} aria-hidden="true" /> {t('projects.commitPolicy.title')}
        {saving && <Loader2 size={12} className="spin" aria-hidden="true" />}
      </h4>
      <p className="project-agent-files-help">{t('projects.commitPolicy.help')}</p>

      {policy?.global_identity && effective && (
        <div className="set-expose-warn" role="alert" data-testid="project-commit-policy-global-warning">
          <AlertTriangle size={13} />
          <span>{t('projects.commitPolicy.globalWarning', effective.email)}</span>
        </div>
      )}

      <strong className="project-commit-policy-label">{t('projects.commitPolicy.identity')}</strong>
      <div className="flex-row gap-3">
        <input
          type="text"
          className="set-input"
          aria-label={t('projects.commitPolicy.name')}
          placeholder={detected?.name ?? t('projects.commitPolicy.name')}
          value={name}
          disabled={!policy || saving}
          onChange={event => setName(event.target.value)}
          data-testid="project-commit-policy-name"
        />
        <input
          type="email"
          className="set-input"
          aria-label={t('projects.commitPolicy.email')}
          placeholder={detected?.email ?? t('projects.commitPolicy.email')}
          value={email}
          disabled={!policy || saving}
          onChange={event => setEmail(event.target.value)}
          data-testid="project-commit-policy-email"
        />
      </div>
      {detected && (
        <p className="project-agent-files-where" data-testid="project-commit-policy-detected">
          {t(
            'projects.commitPolicy.detected',
            `${detected.name} <${detected.email}>`,
            t(`projects.commitPolicy.identitySource.${detected.identity_source}`),
          )}
        </p>
      )}
      {half && (
        <p role="alert" className="project-agent-files-error" data-testid="project-commit-policy-incomplete">
          {t('projects.commitPolicy.identityIncomplete')}
        </p>
      )}

      <strong className="project-commit-policy-label">{t('projects.commitPolicy.signoff')}</strong>
      <div className="flex-row gap-3">
        {SIGNOFF.map(value => (
          <label key={value} className="flex-row gap-2" data-selected={signoff === value}>
            <input
              type="radio"
              name={`commit-signoff-${projectId}`}
              checked={signoff === value}
              disabled={!policy || saving}
              onChange={() => setSignoff(value)}
              data-testid={`project-commit-policy-signoff-${value}`}
            />
            <span className="text-sm">{t(`projects.commitPolicy.signoff.${value}`)}</span>
          </label>
        ))}
      </div>
      {detected && (
        <p className="project-agent-files-where" data-testid="project-commit-policy-signoff-detected">
          {t(
            'projects.commitPolicy.signoffDetected',
            t(detected.signoff ? 'projects.commitPolicy.signoffYes' : 'projects.commitPolicy.signoffNo'),
            t(`projects.commitPolicy.signoffSource.${detected.signoff_source}`),
          )}
        </p>
      )}

      <div className="flex-row gap-3">
        <button
          type="button"
          className="btn btn-primary"
          disabled={!policy || saving || half || unchanged}
          onClick={() => void save({ name, email, signoff })}
          data-testid="project-commit-policy-save"
        >
          {t('projects.commitPolicy.save')}
        </button>
        <button
          type="button"
          className="btn btn-secondary"
          disabled={!policy || saving || (policy.settings.name == null && policy.settings.email == null && policy.settings.signoff === 'auto')}
          onClick={() => void save({ name: '', email: '', signoff: 'auto' })}
          data-testid="project-commit-policy-reset"
        >
          {t('projects.commitPolicy.useDetected')}
        </button>
      </div>
      {error && <p role="alert" className="project-agent-files-error">{error}</p>}
    </section>
  );
}
