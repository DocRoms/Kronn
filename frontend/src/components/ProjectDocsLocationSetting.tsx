// KT-263 — where the project keeps its AI doc, overriding the global default.
import { useEffect, useState } from 'react';
import { BookOpen, Loader2 } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { userError } from '../lib/userError';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import type { ProjectDocsLocation } from '../types/generated';

interface Props {
  projectId: string;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ProjectDocsLocationSetting({ projectId, t }: Props) {
  const [state, setState] = useState<ProjectDocsLocation | null>(null);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    // Inside the promise: a failure here stays in this setting, never in the card.
    Promise.resolve()
      .then(() => projectsApi.docsLocation(projectId))
      .then(value => {
        if (!live) return;
        setState(value);
        setDraft(value.override_dir ?? '');
      })
      .catch(e => { if (live) setError(userError(e)); });
    return () => { live = false; };
  }, [projectId]);

  const save = useAsyncGuard(async (dir: string | null) => {
    setSaving(true);
    setError(null);
    try {
      const value = await projectsApi.setDocsLocation(projectId, dir);
      setState(value);
      setDraft(value.override_dir ?? '');
    } catch (e) {
      setError(userError(e));
    } finally {
      setSaving(false);
    }
  });

  const trimmed = draft.trim();
  const unchanged = trimmed === (state?.override_dir ?? '');

  return (
    <section className="project-docs-location" data-testid="project-docs-location">
      <h4>
        <BookOpen size={14} aria-hidden="true" /> {t('projects.docsLocation.title')}
        {saving && <Loader2 size={12} className="spin" aria-hidden="true" />}
      </h4>
      <p className="project-docs-location-help">{t('projects.docsLocation.help')}</p>
      {state && (
        <p className="project-docs-location-effective" data-testid="project-docs-location-effective">
          {state.effective_dir
            ? t(`projects.docsLocation.source.${state.source}`, `${state.effective_dir}/`)
            : t('projects.docsLocation.none')}
        </p>
      )}
      {state?.refused && (
        <p role="alert" className="project-docs-location-error" data-testid="project-docs-location-refused">
          {t('projects.docsLocation.refused', state.refused)}
        </p>
      )}
      <div className="project-docs-location-row">
        <input
          type="text"
          className="set-input set-input-sm"
          value={draft}
          placeholder={state?.default_dir ?? 'docs'}
          aria-label={t('projects.docsLocation.inputLabel')}
          disabled={!state || saving}
          onChange={e => setDraft(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter' && !unchanged) void save(trimmed || null); }}
          data-testid="project-docs-location-input"
        />
        <button
          type="button"
          className="set-icon-btn"
          disabled={!state || saving || unchanged}
          onClick={() => void save(trimmed || null)}
          data-testid="project-docs-location-save"
        >
          {t('common.save')}
        </button>
        {state?.override_dir && (
          <button
            type="button"
            className="set-icon-btn"
            disabled={saving}
            onClick={() => void save(null)}
            data-testid="project-docs-location-reset"
          >
            {t('projects.docsLocation.reset')}
          </button>
        )}
      </div>
      {error && <p role="alert" className="project-docs-location-error">{error}</p>}
    </section>
  );
}
