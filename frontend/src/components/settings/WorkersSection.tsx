// KT-1345 — Settings > Workers: the review-round limit every launch path starts from.
//
// A task execution gets a number of `request_changes` rounds before it escalates.
// Single-task launches made through MCP could not set it and were stuck on 3, so
// the default lives here, server-side: the campaign dialog is pre-filled with it,
// MCP launches and workflow-delegated subtasks use it, and each launch can still
// carry its own. « Unlimited » lifts the cap — stalls (no delivery, watchdog) still
// escalate and a campaign's token budget still stops it, but a worker looping on a
// review that never converges burns tokens, which the warning says.
import { useEffect, useState } from 'react';
import { AlertTriangle, Bot } from 'lucide-react';
import { config as configApi } from '../../lib/api';
import type { WorkersConfig } from '../../types/generated';
import { userError } from '../../lib/userError';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import { MAX_REVIEW_ROUNDS, MIN_REVIEW_ROUNDS } from '../../lib/reviewRounds';
import '../../pages/SettingsPage.css';

interface Props {
  t: (key: string, ...args: (string | number)[]) => string;
}

export function WorkersSection({ t }: Props) {
  const [saved, setSaved] = useState<WorkersConfig | null>(null);
  const [rounds, setRounds] = useState('5');
  const [unlimited, setUnlimited] = useState(false);
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
      })
      .catch(cause => { if (live) setError(userError(cause)); });
    return () => { live = false; };
  }, []);

  const count = Number(rounds);
  const validCount = Number.isInteger(count) && count >= MIN_REVIEW_ROUNDS && count <= MAX_REVIEW_ROUNDS;
  // Unlimited keeps the integer for when the box is unticked again, so an
  // unparseable field only blocks the save while it would actually be used.
  const valid = unlimited || validCount;
  const unchanged = saved !== null
    && unlimited === saved.review_rounds_unlimited
    && (unlimited || count === saved.default_max_review_rounds);

  const save = useAsyncGuard(async () => {
    if (!saved || !valid) return;
    setError(null);
    try {
      const next = await configApi.saveWorkers({
        default_max_review_rounds: validCount ? count : saved.default_max_review_rounds,
        review_rounds_unlimited: unlimited,
      });
      setSaved(next);
      setRounds(String(next.default_max_review_rounds));
      setUnlimited(next.review_rounds_unlimited);
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
        {error && <p role="alert" className="default-docs-dir-error" data-testid="workers-error">{error}</p>}
      </div>
    </div>
  );
}
