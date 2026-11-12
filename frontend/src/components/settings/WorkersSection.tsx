// KT-1345 — Settings > Workers: the review-round limit every launch path starts from.
//
// A task execution gets a number of `request_changes` rounds before it escalates.
// Single-task launches made through MCP could not set it and were stuck on 3, so
// the default lives here, server-side: the campaign dialog is pre-filled with it,
// MCP launches and workflow-delegated subtasks use it, and each launch can still
// carry its own. « Unlimited » lifts the cap — stalls (no delivery, watchdog) still
// escalate and a campaign's token budget still stops it, but a worker looping on a
// review that never converges burns tokens, which the warning says.
//
// KT-1633 — the same card holds the « background priority » switch: workers, their
// validations and workflow Agent / Exec steps run at a lower CPU priority so the
// owner's applications stay responsive. On by default; saved with the same button.
// KT-1632 — the same section holds the cap on local task workers running at once
// (default 2): builds, not agent processes, load the machine, so a launch beyond
// the cap waits `Queued` and starts when a slot frees.
import { useEffect, useState } from 'react';
import { AlertTriangle, Bot } from 'lucide-react';
import { config as configApi } from '../../lib/api';
import type { WorkersConfig } from '../../types/generated';
import { userError } from '../../lib/userError';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import { MAX_REVIEW_ROUNDS, MIN_REVIEW_ROUNDS } from '../../lib/reviewRounds';
import { DEFAULT_HEAVY_OUTPUT_KB, MAX_HEAVY_OUTPUT_KB, MIN_HEAVY_OUTPUT_KB } from '../../lib/heavyOutput';
import { DEFAULT_TASK_WORKERS, MAX_TASK_WORKERS, MIN_TASK_WORKERS } from '../../lib/taskWorkers';
import {
  commandOutlivesGeneration,
  DEFAULT_BASH_DEFAULT_TIMEOUT_MIN,
  DEFAULT_BASH_MAX_TIMEOUT_MIN,
  MAX_BASH_TIMEOUT_MIN,
  MIN_BASH_TIMEOUT_MIN,
} from '../../lib/workerShellTimeouts';
import '../../pages/SettingsPage.css';

interface Props {
  t: (key: string, ...args: (string | number)[]) => string;
}

export function WorkersSection({ t }: Props) {
  const [saved, setSaved] = useState<WorkersConfig | null>(null);
  const [rounds, setRounds] = useState('5');
  const [unlimited, setUnlimited] = useState(false);
  const [background, setBackground] = useState(true);
  const [taskWorkers, setTaskWorkers] = useState(String(DEFAULT_TASK_WORKERS));
  const [heavyKb, setHeavyKb] = useState(String(DEFAULT_HEAVY_OUTPUT_KB));
  const [bashDefault, setBashDefault] = useState(String(DEFAULT_BASH_DEFAULT_TIMEOUT_MIN));
  const [bashMax, setBashMax] = useState(String(DEFAULT_BASH_MAX_TIMEOUT_MIN));
  // The generation limit (Settings > Server) cuts the whole turn whatever a command is
  // allowed, so it is shown next to the per-command cap. Read-only here.
  const [generationMin, setGenerationMin] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => configApi.getServerConfig())
      .then(cfg => { if (live) setGenerationMin(cfg?.agent_global_timeout_min ?? null); })
      .catch(() => { /* the limit is a hint: the form works without it */ });
    return () => { live = false; };
  }, []);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => configApi.getWorkers())
      .then(value => {
        if (!live) return;
        setSaved(value);
        setRounds(String(value.default_max_review_rounds));
        setUnlimited(value.review_rounds_unlimited);
        // A server that predates the switch leaves it on, as the server's own default does.
        setBackground(value.background_priority !== false);
        setTaskWorkers(String(value.max_concurrent_task_workers));
        setHeavyKb(String(value.heavy_output_kb ?? DEFAULT_HEAVY_OUTPUT_KB));
        setBashDefault(String(value.bash_default_timeout_min ?? DEFAULT_BASH_DEFAULT_TIMEOUT_MIN));
        setBashMax(String(value.bash_max_timeout_min ?? DEFAULT_BASH_MAX_TIMEOUT_MIN));
      })
      .catch(cause => { if (live) setError(userError(cause)); });
    return () => { live = false; };
  }, []);

  const count = Number(rounds);
  const validCount = Number.isInteger(count) && count >= MIN_REVIEW_ROUNDS && count <= MAX_REVIEW_ROUNDS;
  const workerCap = Number(taskWorkers);
  const validWorkerCap = Number.isInteger(workerCap)
    && workerCap >= MIN_TASK_WORKERS && workerCap <= MAX_TASK_WORKERS;
  // Unlimited keeps the integer for when the box is unticked again, so an
  // unparseable field only blocks the save while it would actually be used.
  const heavy = Number(heavyKb);
  const validHeavy = Number.isInteger(heavy) && heavy >= MIN_HEAVY_OUTPUT_KB && heavy <= MAX_HEAVY_OUTPUT_KB;
  const bashDefaultMin = Number(bashDefault);
  const bashMaxMin = Number(bashMax);
  const validBashMax = Number.isInteger(bashMaxMin)
    && bashMaxMin >= MIN_BASH_TIMEOUT_MIN && bashMaxMin <= MAX_BASH_TIMEOUT_MIN;
  const validBashDefault = Number.isInteger(bashDefaultMin)
    && bashDefaultMin >= MIN_BASH_TIMEOUT_MIN && validBashMax && bashDefaultMin <= bashMaxMin;
  const validBash = validBashMax && validBashDefault;
  const valid = (unlimited || validCount) && validWorkerCap && validHeavy && validBash;
  const roundsUnchanged = saved !== null
    && unlimited === saved.review_rounds_unlimited
    && background === (saved.background_priority !== false)
    && (unlimited || count === saved.default_max_review_rounds);
  const unchanged = roundsUnchanged
    && saved !== null && workerCap === saved.max_concurrent_task_workers
    && heavy === (saved.heavy_output_kb ?? DEFAULT_HEAVY_OUTPUT_KB)
    && bashDefaultMin === (saved.bash_default_timeout_min ?? DEFAULT_BASH_DEFAULT_TIMEOUT_MIN)
    && bashMaxMin === (saved.bash_max_timeout_min ?? DEFAULT_BASH_MAX_TIMEOUT_MIN);

  const save = useAsyncGuard(async () => {
    if (!saved || !valid) return;
    setError(null);
    try {
      const next = await configApi.saveWorkers({
        default_max_review_rounds: validCount ? count : saved.default_max_review_rounds,
        review_rounds_unlimited: unlimited,
        background_priority: background,
        max_concurrent_task_workers: workerCap,
        heavy_output_kb: heavy,
        bash_default_timeout_min: bashDefaultMin,
        bash_max_timeout_min: bashMaxMin,
      });
      setSaved(next);
      setRounds(String(next.default_max_review_rounds));
      setUnlimited(next.review_rounds_unlimited);
      setBackground(next.background_priority !== false);
      setTaskWorkers(String(next.max_concurrent_task_workers));
      setHeavyKb(String(next.heavy_output_kb ?? DEFAULT_HEAVY_OUTPUT_KB));
      setBashDefault(String(next.bash_default_timeout_min ?? DEFAULT_BASH_DEFAULT_TIMEOUT_MIN));
      setBashMax(String(next.bash_max_timeout_min ?? DEFAULT_BASH_MAX_TIMEOUT_MIN));
    } catch (cause) {
      setError(userError(cause));
    }
  });

  const current = saved === null
    ? '…'
    : saved.review_rounds_unlimited
      ? t('config.workers.currentUnlimited')
      : t('config.workers.currentRounds', saved.default_max_review_rounds);

  return (
    <div id="settings-workers" className="set-card" data-testid="workers-section">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <Bot size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('config.workers.title')}</span>
        </div>
        <p className="set-hint">{t('config.workers.hint')}</p>

        <div className="flex-row gap-4 mb-3">
          <span className="label">{t('config.workers.reviewRounds')}</span>
          <span className="text-base font-semibold text-accent" style={{ marginLeft: 'auto' }} data-testid="workers-current">
            {current}
          </span>
        </div>
        <p className="text-sm text-faint mb-4" data-testid="workers-applies">{t('config.workers.appliesTo')}</p>

        <div className="flex-row gap-3">
          <input
            type="number"
            className="set-input set-input-sm"
            min={MIN_REVIEW_ROUNDS}
            max={MAX_REVIEW_ROUNDS}
            aria-label={t('config.workers.reviewRounds')}
            value={rounds}
            disabled={saved === null || unlimited}
            onChange={event => setRounds(event.target.value)}
            onKeyDown={event => { if (event.key === 'Enter' && valid && !unchanged) void save(); }}
            data-testid="workers-rounds-input"
          />
          <label className="flex-row gap-3">
            <input
              type="checkbox"
              checked={unlimited}
              disabled={saved === null}
              onChange={event => setUnlimited(event.target.checked)}
              data-testid="workers-unlimited"
            />
            <span className="text-sm">{t('config.workers.unlimited')}</span>
          </label>
          <button
            type="button"
            className="set-icon-btn text-accent"
            disabled={saved === null || !valid || unchanged}
            onClick={() => void save()}
            data-testid="workers-save"
          >
            {t('common.save')}
          </button>
        </div>
        {!unlimited && !validCount && (
          <p role="alert" className="default-docs-dir-error" data-testid="workers-invalid">
            {t('config.workers.invalid', MIN_REVIEW_ROUNDS, MAX_REVIEW_ROUNDS)}
          </p>
        )}
        {unlimited && (
          <div className="set-expose-warn" role="alert" data-testid="workers-unlimited-warning">
            <AlertTriangle size={13} />
            <span>{t('config.workers.unlimitedWarning')}</span>
          </div>
        )}
        <div className="flex-row gap-4 mt-4">
          <label className="flex-row gap-3">
            <input
              type="checkbox"
              checked={background}
              disabled={saved === null}
              onChange={event => setBackground(event.target.checked)}
              data-testid="workers-background-priority"
            />
            <span className="text-sm">{t('config.workers.backgroundPriority')}</span>
          </label>
        </div>
        <p className="text-sm text-faint mb-4" data-testid="workers-background-hint">
          {t('config.workers.backgroundPriorityHint')}
        </p>

        <div className="flex-row gap-4 mb-3" style={{ marginTop: 16 }}>
          <span className="label">{t('config.workers.maxConcurrent')}</span>
          <span className="text-base font-semibold text-accent" style={{ marginLeft: 'auto' }} data-testid="workers-cap-current">
            {saved === null ? '…' : t('config.workers.maxConcurrentCurrent', saved.max_concurrent_task_workers)}
          </span>
        </div>
        <p className="text-sm text-faint mb-4" data-testid="workers-cap-hint">{t('config.workers.maxConcurrentHint')}</p>
        <div className="flex-row gap-3">
          <input
            type="number"
            className="set-input set-input-sm"
            min={MIN_TASK_WORKERS}
            max={MAX_TASK_WORKERS}
            aria-label={t('config.workers.maxConcurrent')}
            value={taskWorkers}
            disabled={saved === null}
            onChange={event => setTaskWorkers(event.target.value)}
            onKeyDown={event => { if (event.key === 'Enter' && valid && !unchanged) void save(); }}
            data-testid="workers-cap-input"
          />
        </div>
        {!validWorkerCap && (
          <p role="alert" className="default-docs-dir-error" data-testid="workers-cap-invalid">
            {t('config.workers.maxConcurrentInvalid', MIN_TASK_WORKERS, MAX_TASK_WORKERS)}
          </p>
        )}

        <div className="flex-row gap-4 mb-3" style={{ marginTop: 16 }}>
          <span className="label">{t('config.workers.heavyOutput')}</span>
          <span className="text-base font-semibold text-accent" style={{ marginLeft: 'auto' }} data-testid="workers-heavy-current">
            {saved === null ? '…' : t('config.workers.heavyOutputCurrent', saved.heavy_output_kb ?? DEFAULT_HEAVY_OUTPUT_KB)}
          </span>
        </div>
        <p className="text-sm text-faint mb-4" data-testid="workers-heavy-hint">{t('config.workers.heavyOutputHint')}</p>
        <div className="flex-row gap-3">
          <input
            type="number"
            className="set-input set-input-sm"
            min={MIN_HEAVY_OUTPUT_KB}
            max={MAX_HEAVY_OUTPUT_KB}
            aria-label={t('config.workers.heavyOutput')}
            value={heavyKb}
            disabled={saved === null}
            onChange={event => setHeavyKb(event.target.value)}
            onKeyDown={event => { if (event.key === 'Enter' && valid && !unchanged) void save(); }}
            data-testid="workers-heavy-input"
          />
        </div>
        {!validHeavy && (
          <p role="alert" className="default-docs-dir-error" data-testid="workers-heavy-invalid">
            {t('config.workers.heavyOutputInvalid', MIN_HEAVY_OUTPUT_KB, MAX_HEAVY_OUTPUT_KB)}
          </p>
        )}

        <div className="flex-row gap-4 mb-3" style={{ marginTop: 16 }}>
          <span className="label">{t('config.workers.shellTimeouts')}</span>
          <span className="text-base font-semibold text-accent" style={{ marginLeft: 'auto' }} data-testid="workers-shell-current">
            {saved === null
              ? '…'
              : t(
                'config.workers.shellTimeoutsSummary',
                saved.bash_default_timeout_min ?? DEFAULT_BASH_DEFAULT_TIMEOUT_MIN,
                saved.bash_max_timeout_min ?? DEFAULT_BASH_MAX_TIMEOUT_MIN,
                generationMin ?? '…',
              )}
          </span>
        </div>
        <p className="text-sm text-faint mb-4" data-testid="workers-shell-hint">{t('config.workers.shellTimeoutsHint')}</p>
        <div className="flex-row gap-3">
          <label className="flex-row gap-3">
            <span className="text-sm">{t('config.workers.bashDefault')}</span>
            <input
              type="number"
              className="set-input set-input-sm"
              min={MIN_BASH_TIMEOUT_MIN}
              max={MAX_BASH_TIMEOUT_MIN}
              aria-label={t('config.workers.bashDefault')}
              value={bashDefault}
              disabled={saved === null}
              onChange={event => setBashDefault(event.target.value)}
              onKeyDown={event => { if (event.key === 'Enter' && valid && !unchanged) void save(); }}
              data-testid="workers-bash-default-input"
            />
          </label>
          <label className="flex-row gap-3">
            <span className="text-sm">{t('config.workers.bashMax')}</span>
            <input
              type="number"
              className="set-input set-input-sm"
              min={MIN_BASH_TIMEOUT_MIN}
              max={MAX_BASH_TIMEOUT_MIN}
              aria-label={t('config.workers.bashMax')}
              value={bashMax}
              disabled={saved === null}
              onChange={event => setBashMax(event.target.value)}
              onKeyDown={event => { if (event.key === 'Enter' && valid && !unchanged) void save(); }}
              data-testid="workers-bash-max-input"
            />
          </label>
          <span className="flex-row gap-3" data-testid="workers-generation-limit">
            <span className="text-sm">{t('config.workers.generationLimit')}</span>
            <span className="text-base font-semibold">{generationMin ?? '…'}</span>
          </span>
        </div>
        <p className="text-sm text-faint mb-4" data-testid="workers-generation-hint">{t('config.workers.generationLimitHint')}</p>
        {!validBash && (
          <p role="alert" className="default-docs-dir-error" data-testid="workers-shell-invalid">
            {t('config.workers.shellTimeoutsInvalid', MIN_BASH_TIMEOUT_MIN, MAX_BASH_TIMEOUT_MIN)}
          </p>
        )}
        {validBash && commandOutlivesGeneration(bashMaxMin, generationMin) && (
          <div className="set-expose-warn" role="alert" data-testid="workers-shell-outlive-warning">
            <AlertTriangle size={13} />
            <span>{t('config.workers.shellTimeoutsOutlive')}</span>
          </div>
        )}
        {error && <p role="alert" className="default-docs-dir-error" data-testid="workers-error">{error}</p>}
      </div>
    </div>
  );
}
