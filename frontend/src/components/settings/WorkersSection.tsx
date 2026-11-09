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
import { DEFAULT_TASK_WORKERS, MAX_TASK_WORKERS, MIN_TASK_WORKERS } from '../../lib/taskWorkers';
import '../../pages/SettingsPage.css';

interface Props {
  t: (key: string, ...args: (string | number)[]) => string;
}

export function WorkersSection({ t }: Props) {
  const [saved, setSaved] = useState<WorkersConfig | null>(null);
  const [rounds, setRounds] = useState('5');
  const [unlimited, setUnlimited] = useState(false);
  const [taskWorkers, setTaskWorkers] = useState(String(DEFAULT_TASK_WORKERS));
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => configApi.getWorkers())
      .then(value => {
        if (!live) return;
        setSaved(value);
        setRounds(String(value.default_max_review_rounds));
        setUnlimited(value.review_rounds_unlimited);
        setTaskWorkers(String(value.max_concurrent_task_workers));
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
  const valid = (unlimited || validCount) && validWorkerCap;
  const roundsUnchanged = saved !== null
    && unlimited === saved.review_rounds_unlimited
    && (unlimited || count === saved.default_max_review_rounds);
  const unchanged = roundsUnchanged
    && saved !== null && workerCap === saved.max_concurrent_task_workers;

  const save = useAsyncGuard(async () => {
    if (!saved || !valid) return;
    setError(null);
    try {
      const next = await configApi.saveWorkers({
        default_max_review_rounds: validCount ? count : saved.default_max_review_rounds,
        review_rounds_unlimited: unlimited,
        max_concurrent_task_workers: workerCap,
      });
      setSaved(next);
      setRounds(String(next.default_max_review_rounds));
      setUnlimited(next.review_rounds_unlimited);
      setTaskWorkers(String(next.max_concurrent_task_workers));
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
        {error && <p role="alert" className="default-docs-dir-error" data-testid="workers-error">{error}</p>}
      </div>
    </div>
  );
}
