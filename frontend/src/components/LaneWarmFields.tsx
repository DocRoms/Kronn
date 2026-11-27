// KT-1742 — the lane warm-up fields, shared by Settings > Workers (the global
// entry) and a project's own override. Controlled: the caller owns the draft and
// the save button.
//
// Each field is a group (label, control, help, problem) and the groups are
// spaced apart, so a label never sits against the previous field's help line.
import { useEffect, useState } from 'react';
import { config as configApi } from '../lib/api';
import type { LaneWarmDraft, SeedAlignmentChoice, EnvMode } from '../lib/laneWarm';
import { MAX_VALIDATION_CEILING_SECS, MIN_VALIDATION_CEILING_SECS } from '../lib/validationCeiling';
import { DEFAULT_FIRST_BUILD_TIMEOUT_MIN, MAX_FIRST_BUILD_TIMEOUT_MIN, draftProblem } from '../lib/laneWarm';

interface Props {
  draft: LaneWarmDraft;
  onChange: (next: LaneWarmDraft) => void;
  /** `project`: an unset field follows Settings > Workers; `global`: it is detected. */
  scope: 'global' | 'project';
  disabled?: boolean;
  /** Distinguishes the ids when two forms are on one page. */
  idPrefix: string;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function LaneWarmFields({ draft, onChange, scope, disabled = false, idPrefix, t }: Props) {
  const unset = scope === 'project' ? t('laneWarm.inherit') : t('laneWarm.detected');
  const problem = draftProblem(draft);
  const set = (patch: Partial<LaneWarmDraft>) => onChange({ ...draft, ...patch });

  // The host's KRONN_WORKER_FIRST_BUILD_TIMEOUT_SECS. Order: project setting >
  // global setting > this variable > built-in default, so it only fills an empty
  // field and the owner is told when it is set.
  const [envSecs, setEnvSecs] = useState<number | null>(null);
  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => configApi.getFirstBuildEnvSecs())
      .then(secs => { if (live) setEnvSecs(typeof secs === 'number' && secs > 0 ? secs : null); })
      .catch(() => { if (live) setEnvSecs(null); });
    return () => { live = false; };
  }, []);

  return (
    <div className="lane-warm-fields" data-testid={`${idPrefix}-fields`}>
      <div className="lane-warm-group">
        <label className="lane-warm-field" htmlFor={`${idPrefix}-alignment`}>
          <span className="label">{t('laneWarm.seedAlignment')}</span>
          <select
            id={`${idPrefix}-alignment`}
            className="set-input"
            value={draft.seedAlignment}
            disabled={disabled}
            onChange={event => set({ seedAlignment: event.target.value as SeedAlignmentChoice })}
            data-testid={`${idPrefix}-alignment`}
          >
            <option value="">{unset}</option>
            <option value="on">{t('laneWarm.on')}</option>
            <option value="off">{t('laneWarm.off')}</option>
          </select>
        </label>
        <p className="text-sm text-faint">{t('laneWarm.seedAlignmentHelp')}</p>
      </div>

      <div className="lane-warm-group">
        <label className="lane-warm-field" htmlFor={`${idPrefix}-env-mode`}>
          <span className="label">{t('laneWarm.compilerEnv')}</span>
          <select
            id={`${idPrefix}-env-mode`}
            className="set-input"
            value={draft.envMode}
            disabled={disabled}
            onChange={event => set({ envMode: event.target.value as EnvMode })}
            data-testid={`${idPrefix}-env-mode`}
          >
            <option value="inherit">{unset}</option>
            <option value="custom">{t('laneWarm.custom')}</option>
            <option value="none">{t('laneWarm.none')}</option>
          </select>
        </label>
        {draft.envMode === 'custom' && (
          <textarea
            className="set-input"
            rows={3}
            spellCheck={false}
            aria-label={t('laneWarm.compilerEnv')}
            placeholder="CFLAGS_aarch64_apple_darwin=-save-temps=obj"
            value={draft.envText}
            disabled={disabled}
            onChange={event => set({ envText: event.target.value })}
            data-testid={`${idPrefix}-env-text`}
          />
        )}
        <p className="text-sm text-faint">{t('laneWarm.compilerEnvHelp')}</p>
        {problem === 'env' && (
          <p role="alert" className="default-docs-dir-error" data-testid={`${idPrefix}-env-invalid`}>
            {t('laneWarm.envInvalid')}
          </p>
        )}
      </div>

      <div className="lane-warm-group">
        <label className="lane-warm-field" htmlFor={`${idPrefix}-command`}>
          <span className="label">{t('laneWarm.command')}</span>
          <input
            id={`${idPrefix}-command`}
            type="text"
            className="set-input"
            spellCheck={false}
            placeholder={t('laneWarm.commandPlaceholder')}
            value={draft.command}
            disabled={disabled}
            onChange={event => set({ command: event.target.value })}
            data-testid={`${idPrefix}-command`}
          />
        </label>
        <p className="text-sm text-faint">{t('laneWarm.commandHelp')}</p>
      </div>

      {scope === 'project' && (
        <div className="lane-warm-group">
          <label className="lane-warm-field" htmlFor={`${idPrefix}-roots`}>
            <span className="label">{t('laneWarm.roots')}</span>
            <textarea
              id={`${idPrefix}-roots`}
              className="set-input"
              rows={2}
              spellCheck={false}
              placeholder={t('laneWarm.rootsPlaceholder')}
              value={draft.rootsText}
              disabled={disabled}
              onChange={event => set({ rootsText: event.target.value })}
              data-testid={`${idPrefix}-roots`}
            />
          </label>
          <p className="text-sm text-faint">{t('laneWarm.rootsHelp')}</p>
          {problem === 'roots' && (
            <p role="alert" className="default-docs-dir-error" data-testid={`${idPrefix}-roots-invalid`}>
              {t('laneWarm.rootsInvalid')}
            </p>
          )}
        </div>
      )}

      {scope === 'project' && (
        <div className="lane-warm-group">
          <label className="lane-warm-field" htmlFor={`${idPrefix}-executables`}>
            <span className="label">{t('laneWarm.executables')}</span>
            <textarea
              id={`${idPrefix}-executables`}
              className="set-input"
              rows={2}
              spellCheck={false}
              placeholder={t('laneWarm.executablesPlaceholder')}
              value={draft.executablesText}
              disabled={disabled}
              onChange={event => set({ executablesText: event.target.value })}
              data-testid={`${idPrefix}-executables`}
            />
          </label>
          <p className="text-sm text-faint">{t('laneWarm.executablesHelp')}</p>
          {problem === 'executables' && (
            <p role="alert" className="default-docs-dir-error" data-testid={`${idPrefix}-executables-invalid`}>
              {t('laneWarm.executablesInvalid')}
            </p>
          )}
        </div>
      )}

      {scope === 'project' && (
        <div className="lane-warm-group">
          <label className="lane-warm-field" htmlFor={`${idPrefix}-ceiling`}>
            <span className="label">{t('laneWarm.validationCeiling')}</span>
            <input
              id={`${idPrefix}-ceiling`}
              type="number"
              className="set-input set-input-sm"
              min={MIN_VALIDATION_CEILING_SECS}
              max={MAX_VALIDATION_CEILING_SECS}
              placeholder={t('laneWarm.inherit')}
              value={draft.validationCeiling}
              disabled={disabled}
              onChange={event => set({ validationCeiling: event.target.value })}
              data-testid={`${idPrefix}-ceiling`}
            />
          </label>
          <p className="text-sm text-faint">
            {t('laneWarm.validationCeilingHelp', MIN_VALIDATION_CEILING_SECS, MAX_VALIDATION_CEILING_SECS)}
          </p>
          {problem === 'ceiling' && (
            <p role="alert" className="default-docs-dir-error" data-testid={`${idPrefix}-ceiling-invalid`}>
              {t('laneWarm.validationCeilingInvalid', MIN_VALIDATION_CEILING_SECS, MAX_VALIDATION_CEILING_SECS)}
            </p>
          )}
        </div>
      )}

      <div className="lane-warm-group">
        <label className="lane-warm-field" htmlFor={`${idPrefix}-timeout`}>
          <span className="label">{t('laneWarm.timeout')}</span>
          <input
            id={`${idPrefix}-timeout`}
            type="number"
            className="set-input set-input-sm"
            min={1}
            max={MAX_FIRST_BUILD_TIMEOUT_MIN}
            placeholder={String(DEFAULT_FIRST_BUILD_TIMEOUT_MIN)}
            value={draft.timeout}
            disabled={disabled}
            onChange={event => set({ timeout: event.target.value })}
            data-testid={`${idPrefix}-timeout`}
          />
        </label>
        <p className="text-sm text-faint">{t('laneWarm.timeoutHelp', DEFAULT_FIRST_BUILD_TIMEOUT_MIN)}</p>
        {envSecs !== null && (
          <p className="text-sm text-faint" data-testid={`${idPrefix}-timeout-env`}>
            {t('laneWarm.timeoutEnvNotice', Math.max(1, Math.round(envSecs / 60)))}
          </p>
        )}
        {problem === 'timeout' && (
          <p role="alert" className="default-docs-dir-error" data-testid={`${idPrefix}-timeout-invalid`}>
            {t('laneWarm.timeoutInvalid', 1, MAX_FIRST_BUILD_TIMEOUT_MIN)}
          </p>
        )}
      </div>
    </div>
  );
}
