// KT-1742 — a project's own lane warm-up settings: they override the global
// entry of Settings > Workers field by field, and are stored with the project's
// workspace settings (next to its worktree hooks, which a save leaves as they are).
import { useEffect, useState } from 'react';
import { Flame, Loader2 } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { userError } from '../lib/userError';
import { LaneWarmFields } from './LaneWarmFields';
import {
  EMPTY_LANE_WARM_DRAFT, configOf, draftOf, draftProblem, sameDraft,
  type LaneWarmDraft,
} from '../lib/laneWarm';
import type { ProjectWorkspace } from '../types/generated';

interface Props {
  projectId: string;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ProjectLaneWarmSetting({ projectId, t }: Props) {
  // `null` = loaded and the project has no workspace settings yet.
  const [workspace, setWorkspace] = useState<ProjectWorkspace | null | undefined>(undefined);
  const [saved, setSaved] = useState<LaneWarmDraft>(EMPTY_LANE_WARM_DRAFT);
  const [draft, setDraft] = useState<LaneWarmDraft>(EMPTY_LANE_WARM_DRAFT);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => projectsApi.workspace(projectId))
      .then(value => {
        if (!live) return;
        setWorkspace(value);
        const loaded = draftOf(value?.lane_warm);
        setSaved(loaded);
        setDraft(loaded);
      })
      .catch(e => { if (live) setError(userError(e)); });
    return () => { live = false; };
  }, [projectId]);

  const loaded = workspace !== undefined;
  const invalid = draftProblem(draft) !== null;
  const unchanged = sameDraft(draft, saved);

  const save = async () => {
    if (!loaded || invalid || unchanged) return;
    setSaving(true);
    setError(null);
    try {
      // The whole workspace is replaced: its hooks go back as they were read.
      const next: ProjectWorkspace = {
        hooks: workspace?.hooks ?? {},
        lane_warm: configOf(draft),
      };
      await projectsApi.setWorkspace(projectId, next);
      setWorkspace(next);
      setSaved(draft);
    } catch (e) {
      setError(userError(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="project-agent-files" data-testid="project-lane-warm">
      <h4>
        <Flame size={14} aria-hidden="true" /> {t('laneWarm.projectTitle')}
        {(saving || !loaded) && !error && <Loader2 size={12} className="spin" aria-hidden="true" />}
      </h4>
      <p className="project-agent-files-help">{t('laneWarm.projectHelp')}</p>
      <LaneWarmFields
        draft={draft}
        onChange={setDraft}
        scope="project"
        disabled={!loaded || saving}
        idPrefix="project-lane-warm"
        t={t}
      />
      <div className="flex-row gap-3">
        <button
          type="button"
          className="btn btn-secondary"
          disabled={!loaded || saving || invalid || unchanged}
          onClick={() => void save()}
          data-testid="project-lane-warm-save"
        >
          {t('common.save')}
        </button>
      </div>
      {error && <p role="alert" className="project-agent-files-error">{error}</p>}
    </section>
  );
}
