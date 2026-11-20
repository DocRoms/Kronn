// KT-1750 — who the commits Kronn makes for a project are attributed to, and
// whether they carry a Signed-off-by. The project inherits (Settings > Workers
// for every project, over the repository's own answers, over the machine's git
// config); this block overrides it for this project, and shows the EFFECTIVE value
// with where it comes from.
import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, GitCommitHorizontal, Loader2 } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { userError } from '../lib/userError';
import {
  commitFields,
  commitSettingsOf,
  isHalfIdentity,
  sameCommitSettings,
  SIGNOFF_SETTINGS,
} from '../lib/commitSettings';
import type { ProjectCommitPolicy, ProjectCommitSettings, SignoffSetting } from '../types/generated';

interface Props {
  projectId: string;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ProjectCommitPolicySetting({ projectId, t }: Props) {
  const [policy, setPolicy] = useState<ProjectCommitPolicy | null>(null);
  // What the card saved: from the policy, or — when the policy cannot be read —
  // from the workspace, so a save never overwrites what it could not see.
  const [stored, setStored] = useState<ProjectCommitSettings | undefined>(undefined);
  const [name, setName] = useState('');
  const [email, setEmail] = useState('');
  const [signoff, setSignoff] = useState<SignoffSetting>('auto');
  const [loadFailed, setLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const showFields = useCallback((settings: ProjectCommitSettings | undefined) => {
    const fields = commitFields(settings);
    setStored(settings);
    setName(fields.name);
    setEmail(fields.email);
    setSignoff(fields.signoff);
  }, []);

  const show = useCallback((value: ProjectCommitPolicy) => {
    setPolicy(value);
    setLoadFailed(false);
    showFields(value.settings);
  }, [showFields]);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => projectsApi.commitPolicy(projectId))
      .then(value => { if (live) show(value); })
      .catch(() => {
        if (!live) return;
        // The detected values cannot be read; the settings stay editable.
        setLoadFailed(true);
        Promise.resolve()
          .then(() => projectsApi.workspace(projectId))
          .then(workspace => { if (live) showFields(workspace?.commit); })
          .catch(() => { /* the form starts empty */ });
      });
    return () => { live = false; };
  }, [projectId, show, showFields]);

  const loading = policy === null && !loadFailed;
  const fields = { name, email, signoff };
  const half = isHalfIdentity(name, email);
  const unchanged = sameCommitSettings(fields, stored);
  const nothingSet = commitSettingsOf(commitFields(stored)) === undefined;

  const save = async (next: { name: string; email: string; signoff: SignoffSetting }) => {
    setSaving(true);
    setError(null);
    try {
      // The workspace is saved whole: every other part (hooks, lane warm-up,
      // managed roots) goes back as it is now.
      const current = await projectsApi.workspace(projectId);
      const commit = {
        name: next.name.trim() === '' ? null : next.name.trim(),
        email: next.email.trim() === '' ? null : next.email.trim(),
        signoff: next.signoff,
      };
      await projectsApi.setWorkspace(projectId, {
        ...current,
        hooks: current?.hooks ?? {},
        lane_warm: current?.lane_warm ?? {},
        commit,
      });
      try {
        show(await projectsApi.commitPolicy(projectId));
      } catch {
        // Saved, but the policy cannot be read back: show what was saved.
        setLoadFailed(true);
        showFields(commitSettingsOf(next));
      }
    } catch (e) {
      setError(userError(e));
    } finally {
      setSaving(false);
    }
  };

  const effective = policy?.effective;
  const formDisabled = loading || saving;

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
      {loadFailed && (
        <p role="alert" className="project-agent-files-error" data-testid="project-commit-policy-load-error">
          {t('projects.commitPolicy.loadError')}
        </p>
      )}

      <div className="project-commit-policy-group">
        <span className="text-sm">{t('projects.commitPolicy.identity')}</span>
        <div className="flex-row gap-3">
          <input
            type="text"
            className="set-input"
            aria-label={t('projects.commitPolicy.name')}
            placeholder={effective?.name ?? t('projects.commitPolicy.name')}
            value={name}
            disabled={formDisabled}
            onChange={event => setName(event.target.value)}
            data-testid="project-commit-policy-name"
          />
          <input
            type="email"
            className="set-input"
            aria-label={t('projects.commitPolicy.email')}
            placeholder={effective?.email ?? t('projects.commitPolicy.email')}
            value={email}
            disabled={formDisabled}
            onChange={event => setEmail(event.target.value)}
            data-testid="project-commit-policy-email"
          />
        </div>
        {effective && (
          <p className="project-agent-files-where" data-testid="project-commit-policy-effective">
            {t(
              'projects.commitPolicy.effective',
              `${effective.name} <${effective.email}>`,
              t(`projects.commitPolicy.identitySource.${effective.identity_source}`),
            )}
          </p>
        )}
        {half && (
          <p role="alert" className="project-agent-files-error" data-testid="project-commit-policy-incomplete">
            {t('projects.commitPolicy.identityIncomplete')}
          </p>
        )}
      </div>

      <div className="project-commit-policy-group">
        <span className="text-sm">{t('projects.commitPolicy.signoff')}</span>
        <div className="flex-row gap-3">
          {SIGNOFF_SETTINGS.map(value => (
            <label key={value} className="flex-row gap-2" data-selected={signoff === value}>
              <input
                type="radio"
                name={`commit-signoff-${projectId}`}
                checked={signoff === value}
                disabled={formDisabled}
                onChange={() => setSignoff(value)}
                data-testid={`project-commit-policy-signoff-${value}`}
              />
              <span className="text-sm">{t(`projects.commitPolicy.signoff.${value}`)}</span>
            </label>
          ))}
        </div>
        {effective && (
          <p className="project-agent-files-where" data-testid="project-commit-policy-signoff-effective">
            {t(
              'projects.commitPolicy.signoffEffective',
              t(effective.signoff ? 'projects.commitPolicy.signoffYes' : 'projects.commitPolicy.signoffNo'),
              t(`projects.commitPolicy.signoffSource.${effective.signoff_source}`),
            )}
          </p>
        )}
      </div>

      <div className="flex-row gap-3">
        <button
          type="button"
          className="btn btn-primary"
          disabled={formDisabled || half || unchanged}
          onClick={() => void save(fields)}
          data-testid="project-commit-policy-save"
        >
          {t('projects.commitPolicy.save')}
        </button>
        <button
          type="button"
          className="btn btn-secondary"
          disabled={formDisabled || loadFailed || nothingSet}
          title={loadFailed ? t('projects.commitPolicy.useDetectedUnavailable') : undefined}
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
