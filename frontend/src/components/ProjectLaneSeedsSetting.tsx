// KT-1438 — the gitignored paths a project's task worktrees are seeded with.
import { useCallback, useEffect, useState } from 'react';
import { FolderInput, Loader2, ShieldAlert, Sparkles, Trash2 } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { userError } from '../lib/userError';
import { formatBytes } from '../lib/live-page-weight';
import type { LaneSeedCandidate, LaneSeedSecretFinding, ProjectLaneSeeds } from '../types/generated';

interface Props {
  projectId: string;
  t: (key: string, ...args: (string | number)[]) => string;
}

/** What waits for the human to accept secret-looking paths. */
interface Held {
  seeds: string[];
  findings: LaneSeedSecretFinding[];
}

const sameList = (a: string[], b: string[]) => a.length === b.length && a.every((path, i) => path === b[i]);

export function ProjectLaneSeedsSetting({ projectId, t }: Props) {
  const [state, setState] = useState<ProjectLaneSeeds | null>(null);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [held, setHeld] = useState<Held | null>(null);
  const [candidates, setCandidates] = useState<LaneSeedCandidate[] | null>(null);
  const [looking, setLooking] = useState(false);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => projectsApi.laneSeeds(projectId))
      .then(value => { if (live) setState(value); })
      .catch(e => { if (live) setError(userError(e)); });
    return () => { live = false; };
  }, [projectId]);

  // Secret-looking paths are not saved until the human confirmed them: the
  // answer lists them and nothing was stored.
  const save = useCallback(async (seeds: string[], confirm: string[] = []) => {
    setSaving(true);
    setError(null);
    try {
      const next = await projectsApi.setLaneSeeds(projectId, seeds, confirm);
      if (next.needs_confirmation.length > 0) {
        setHeld({ seeds, findings: next.needs_confirmation });
        setState(next);
      } else {
        setHeld(null);
        setState(next);
        setDraft('');
      }
    } catch (e) {
      setError(userError(e));
    } finally {
      setSaving(false);
    }
  }, [projectId]);

  const add = (path: string) => {
    const wanted = path.trim();
    if (!state || !wanted || state.seeds.includes(wanted)) return;
    void save([...state.seeds, wanted]);
  };

  const remove = (path: string) => {
    if (!state) return;
    void save(state.seeds.filter(seed => seed !== path));
  };

  const suggest = async () => {
    setLooking(true);
    setError(null);
    try {
      setCandidates(await projectsApi.laneSeedCandidates(projectId));
    } catch (e) {
      setError(userError(e));
    } finally {
      setLooking(false);
    }
  };

  // The file's list is a proposal: it is only seeded once adopted here.
  const profileProposal = !!state?.profile_seeds && !sameList(state.profile_seeds, state.seeds);

  const secretFiles = (finding: LaneSeedSecretFinding) => (
    <>
      {t('projects.laneSeeds.secretFiles', finding.files.join(', '))}
      {finding.truncated ? ` ${t('projects.laneSeeds.secretMore')}` : ''}
    </>
  );

  return (
    <section className="project-agent-files" data-testid="project-lane-seeds">
      <h4>
        <FolderInput size={14} aria-hidden="true" /> {t('projects.laneSeeds.title')}
        {(saving || looking) && <Loader2 size={12} className="spin" aria-hidden="true" />}
      </h4>
      <p className="project-agent-files-help">{t('projects.laneSeeds.help')}</p>

      {state && state.seeds.length === 0 && (
        <p className="project-agent-files-where" data-testid="project-lane-seeds-empty">{t('projects.laneSeeds.empty')}</p>
      )}
      {state && state.seeds.length > 0 && (
        <ul className="project-lane-seeds-list" data-testid="project-lane-seeds-list">
          {state.seeds.map(path => (
            <li key={path} className="project-lane-seeds-item">
              <code>{path}</code>
              <button
                type="button"
                className="btn btn-secondary"
                onClick={() => remove(path)}
                disabled={saving}
                aria-label={`${t('projects.laneSeeds.remove')} ${path}`}
                data-testid={`project-lane-seeds-remove-${path}`}
              >
                <Trash2 size={12} aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      )}

      <form
        className="project-lane-seeds-add"
        onSubmit={event => { event.preventDefault(); add(draft); }}
      >
        <input
          type="text"
          value={draft}
          onChange={event => setDraft(event.target.value)}
          placeholder={t('projects.laneSeeds.placeholder')}
          aria-label={t('projects.laneSeeds.placeholder')}
          disabled={!state || saving}
          data-testid="project-lane-seeds-input"
        />
        <button type="submit" className="btn btn-secondary" disabled={!state || saving || !draft.trim()} data-testid="project-lane-seeds-add">
          {t('projects.laneSeeds.add')}
        </button>
        <button type="button" className="btn btn-secondary" onClick={() => void suggest()} disabled={!state || looking} data-testid="project-lane-seeds-suggest">
          <Sparkles size={12} aria-hidden="true" /> {t('projects.laneSeeds.suggest')}
        </button>
      </form>

      {candidates && (
        <div data-testid="project-lane-seeds-candidates">
          {candidates.length === 0 && <p className="project-agent-files-where">{t('projects.laneSeeds.noCandidate')}</p>}
          <ul className="project-lane-seeds-list">
            {candidates.map(candidate => (
              <li key={candidate.path} className="project-lane-seeds-item">
                <span className="project-lane-seeds-candidate">
                  <code>{candidate.path}</code>
                  <small>
                    {candidate.size_bytes === null ? t('projects.laneSeeds.sizeUnknown') : formatBytes(candidate.size_bytes)}
                    {' · '}
                    {candidate.reason}
                  </small>
                </span>
                <button
                  type="button"
                  className="btn btn-secondary"
                  onClick={() => add(candidate.path)}
                  disabled={saving || !state || state.seeds.includes(candidate.path)}
                  data-testid={`project-lane-seeds-candidate-${candidate.path}`}
                >
                  {t('projects.laneSeeds.add')}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}

      {held && (
        <div className="project-validations-access-warning" role="alertdialog" aria-labelledby={`lane-seeds-secret-${projectId}`} data-testid="project-lane-seeds-secret-warning">
          <strong id={`lane-seeds-secret-${projectId}`}>
            <ShieldAlert size={14} aria-hidden="true" /> {t('projects.laneSeeds.secretTitle')}
          </strong>
          <p>{t('projects.laneSeeds.secretWarning')}</p>
          <ul className="project-lane-seeds-list">
            {held.findings.map(finding => (
              <li key={finding.path} data-testid={`project-lane-seeds-secret-${finding.path}`}>
                <code>{finding.path}</code>
                <small> {secretFiles(finding)}</small>
              </li>
            ))}
          </ul>
          <div className="project-validations-access-warning-actions">
            <button type="button" className="btn btn-secondary" onClick={() => setHeld(null)} disabled={saving}>
              {t('projects.laneSeeds.cancel')}
            </button>
            <button type="button" className="btn btn-danger" onClick={() => void save(held.seeds, held.findings.map(finding => finding.path))} disabled={saving} data-testid="project-lane-seeds-secret-confirm">
              {t('projects.laneSeeds.secretConfirm')}
            </button>
          </div>
        </div>
      )}

      {state && state.secret_findings.length > 0 && !held && (
        <div className="project-agent-files-error" role="alert" data-testid="project-lane-seeds-secret-findings">
          <strong><ShieldAlert size={14} aria-hidden="true" /> {t('projects.laneSeeds.secretDeclaredTitle')}</strong>
          <ul className="project-lane-seeds-list">
            {state.secret_findings.map(finding => (
              <li key={finding.path}>
                <code>{finding.path}</code>
                <small> {secretFiles(finding)}</small>
              </li>
            ))}
          </ul>
        </div>
      )}
      {state?.secret_scan_error && (
        <p role="alert" className="project-agent-files-error" data-testid="project-lane-seeds-scan-error">
          {t('projects.laneSeeds.secretScanError', state.secret_scan_error)}
        </p>
      )}

      {profileProposal && state?.profile_seeds && (
        <p className="project-agent-files-where" data-testid="project-lane-seeds-profile-proposal">
          {t('projects.laneSeeds.profileProposal', state.profile_seeds.join(', ') || '—')}{' '}
          <button
            type="button"
            className="btn btn-secondary"
            onClick={() => void save(state.profile_seeds ?? [])}
            disabled={saving}
            data-testid="project-lane-seeds-use-profile"
          >
            {t('projects.laneSeeds.useProfile')}
          </button>
        </p>
      )}
      {state?.profile_file_written && (
        <p className="project-agent-files-cleaned" data-testid="project-lane-seeds-file-written">{t('projects.laneSeeds.fileWritten')}</p>
      )}
      {state?.profile_file_error && (
        <p role="alert" className="project-agent-files-error">{t('projects.laneSeeds.fileError', state.profile_file_error)}</p>
      )}
      {state?.profile_error && (
        <p role="alert" className="project-agent-files-error">{t('projects.laneSeeds.profileError', state.profile_error)}</p>
      )}

      {state?.last_run && (
        <div data-testid="project-lane-seeds-last-run">
          <p className="project-agent-files-help">{t('projects.laneSeeds.lastRun', state.last_run.at)}</p>
          <ul className="project-lane-seeds-list">
            {state.last_run.seeds.map(seed => (
              <li key={seed.path} className="project-lane-seeds-item" data-outcome={seed.outcome}>
                <span className="project-lane-seeds-candidate">
                  <code>{seed.path}</code>
                  <small data-testid={`project-lane-seeds-outcome-${seed.path}`}>
                    {t(`projects.laneSeeds.outcome.${seed.outcome}`)}
                    {seed.detail ? ` — ${seed.detail}` : ''}
                  </small>
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
      {error && <p role="alert" className="project-agent-files-error">{error}</p>}
    </section>
  );
}
