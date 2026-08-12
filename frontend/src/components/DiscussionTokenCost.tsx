/** What a discussion cost, in the header — KT-254.
 *
 *  TWO figures side by side, and deliberately no total. The agents Kronn spawns
 *  report a cost PER REPLY; a joined CLI reports a RUNNING TOTAL for its whole
 *  session, which also covers file reads, test runs and work in other rooms.
 *  Adding them would double-count the CLI's own messages AND charge this room for
 *  work done elsewhere, and the result would carry no unit anyone could name.
 *
 *  The CLI figure is absent rather than zero when nothing was measured. On one
 *  real session, 4.1 billion tokens of traffic were stored as zero — a "0" in this
 *  slot is the exact reading a user must never be given. */
import { useEffect, useState } from 'react';
import { telemetry } from '../lib/api';
import { compactTokens } from '../lib/tokenFormat';
import type { DiscussionTokenCost as Cost } from '../types/generated';

interface Props {
  discussionId: string;
  t: (key: string, ...args: string[]) => string;
}

export function DiscussionTokenCost({ discussionId, t }: Props) {
  const [cost, setCost] = useState<Cost | null>(null);

  useEffect(() => {
    let live = true;
    // Deferred into a promise so a SYNCHRONOUS throw becomes a rejection. A cost
    // badge must never be able to take the header down with it: the title, the
    // agent switcher and the presence list are what the room needs to work.
    Promise.resolve()
      .then(() => telemetry.discussionCost(discussionId))
      // A failed fetch shows nothing rather than a zero: not knowing is not free.
      .then(value => { if (live) setCost(value); })
      .catch(() => { if (live) setCost(null); });
    return () => { live = false; };
  }, [discussionId]);

  if (!cost) return null;

  const hasInApp = cost.in_app_tokens > 0;
  const cliKnown = cost.cli_traffic_tokens !== null;
  // Nothing to say at all: no agent replied and no CLI session was ever joined.
  if (!hasInApp && cost.cli_sessions === 0) return null;

  return (
    <span className="disc-token-cost" data-testid="disc-token-cost">
      {hasInApp && (
        <span
          className="disc-token-cost-part"
          data-part="in-app"
          title={t('disc.tokenCostInAppHint')}
        >
          {t('disc.tokenCostInApp')}&nbsp;{compactTokens(cost.in_app_tokens)}
        </span>
      )}
      {cost.cli_sessions > 0 && (
        cliKnown ? (
          <span
            className="disc-token-cost-part"
            data-part="cli"
            title={t('disc.tokenCostCliHint')}
          >
            {t('disc.tokenCostCli')}&nbsp;
            {compactTokens(cost.cli_traffic_tokens as number)}
            {/* Coverage next to the figure, not hidden in a tooltip: the number
                only means something once you know how many sessions it covers. */}
            {cost.cli_sessions_unmeasured > 0 && (
              <span className="disc-token-cost-partial">
                {' '}
                {t(
                  'disc.tokenCostPartial',
                  String(cost.cli_sessions_measured),
                  String(cost.cli_sessions),
                )}
              </span>
            )}
          </span>
        ) : (
          <span
            className="disc-token-cost-part"
            data-part="cli-unknown"
            title={t('disc.tokenCostCliUnknownHint')}
          >
            {t('disc.tokenCostCli')}&nbsp;{t('disc.tokenCostUnknown')}
          </span>
        )
      )}
    </span>
  );
}
