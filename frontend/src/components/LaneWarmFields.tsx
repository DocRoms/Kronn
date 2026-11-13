// KT-1742 — the lane warm-up fields, shared by Settings > Workers (the global
// entry) and a project's own override. Controlled: the caller owns the draft and
// the save button.
import type { LaneWarmDraft, SeedAlignmentChoice, EnvMode } from '../lib/laneWarm';
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
  return (
    <div className="lane-warm-fields" data-testid={`${idPrefix}-fields`}>
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
      {problem === 'timeout' && (
        <p role="alert" className="default-docs-dir-error" data-testid={`${idPrefix}-timeout-invalid`}>
          {t('laneWarm.timeoutInvalid', 1, MAX_FIRST_BUILD_TIMEOUT_MIN)}
        </p>
      )}
    </div>
  );
}
