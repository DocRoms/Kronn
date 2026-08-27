import { useEffect, useState } from 'react';
import { useT } from '../lib/I18nContext';
import type { CampaignView } from '../lib/api';
import { readCollapsed, writeCollapsed } from '../lib/orch-panel-state';

interface Props {
  view: CampaignView | null;
  /** Called with the task reference the user chose to launch. */
  onLaunch: (taskReference: string) => void;
  busyTaskReference?: string | null;
}

/**
 * The campaign surface on a discussion's plan: which tasks can be launched right
 * now, why the others cannot, and what the principal currently owes.
 */
export function TaskCampaignPanel({ view, onLaunch, busyTaskReference }: Props) {
  const { t } = useT();
  const runId = view?.run.id ?? '';
  // Read once on mount: the stored answer is about this campaign, and re-reading
  // on every render would fight the user's click.
  const [collapsed, setCollapsed] = useState(() => (runId ? readCollapsed(runId) : false));

  // The panel is mounted before its request resolves in the real plan view.
  // Rehydrate when that asynchronous campaign identity becomes known.
  useEffect(() => {
    if (!runId) return;
    let active = true;
    queueMicrotask(() => {
      if (active) setCollapsed(readCollapsed(runId));
    });
    return () => { active = false; };
  }, [runId]);

  const toggle = () => {
    const next = !collapsed;
    setCollapsed(next);
    if (runId) writeCollapsed(runId, next);
  };

  const candidates = view?.candidates ?? [];
  const launchable = candidates.filter(c => c.launchable);
  const attention = view?.principal_attention;
  const live =
    (attention?.active_executions ?? 0) +
    (attention?.awaiting_review ?? 0) +
    (attention?.awaiting_human ?? 0);

  // Nothing running and nothing to start: the section would be an empty box
  // asking to be understood. It only earns its place once there is a decision
  // to take or work to watch.
  if (!view || (live === 0 && launchable.length === 0)) return null;

  return (
    <section className="orch-campaign" data-testid="orch-campaign">
      <button
        type="button"
        className="orch-campaign-toggle"
        data-testid="orch-campaign-toggle"
        aria-expanded={!collapsed}
        aria-controls="orch-campaign-body"
        onClick={toggle}
      >
        {collapsed ? t('orch.expand') : t('orch.collapse')}
      </button>

      {collapsed ? null : (
      <div id="orch-campaign-body">
      {attention && live > 0 && (
        <p className="orch-campaign-attention" data-testid="orch-campaign-attention">
          {t('orch.attention', attention.active_executions, attention.awaiting_review, attention.awaiting_human)}
        </p>
      )}

      <ul className="orch-campaign-list">
        {candidates.map(candidate => {
          const reference = candidate.task.reference;
          const busy = busyTaskReference === reference;
          return (
            <li key={reference} className="orch-campaign-item" data-testid={`orch-candidate-${reference}`}>
              <span className="orch-campaign-task">
                <span className="orch-campaign-ref">{reference}</span>
                <span className="orch-campaign-title">{candidate.task.title}</span>
              </span>

              {candidate.launchable ? (
                <button
                  type="button"
                  className="orch-campaign-launch"
                  data-testid={`orch-launch-${reference}`}
                  disabled={busy}
                  onClick={() => onLaunch(reference)}
                >
                  {busy ? t('orch.launching') : t('orch.launch')}
                </button>
              ) : (
                // No button at all rather than a disabled one: a greyed CTA still
                // reads as "this is the way", and the way is elsewhere until the
                // reason below is dealt with.
                <ul className="orch-campaign-reasons" data-testid={`orch-reasons-${reference}`}>
                  {candidate.reasons.map(reason => (
                    <li key={reason.code} className="orch-campaign-reason">
                      {reason.detail}
                    </li>
                  ))}
                </ul>
              )}
            </li>
          );
        })}
      </ul>
      </div>
      )}
    </section>
  );
}
