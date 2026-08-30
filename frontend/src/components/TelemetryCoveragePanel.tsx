import { useEffect, useState } from 'react';
import { telemetry, measuredRatio } from '../lib/api';
import { AGENT_LABELS } from '../lib/constants';
import { useT } from '../lib/I18nContext';
import type { AgentType, TelemetryCoverage } from '../types/generated';

/**
 * KT-190 — how much of the CLI token spend Kronn can actually account for.
 *
 * Kronn measures the agents it spawns. A CLI that joined a room on its own was
 * never spawned, so everything it posts is stored with no counter: on one real
 * session, 4.1 billion tokens of traffic were recorded as zero. A total shown
 * without coverage beside it would therefore look complete while being mostly
 * blind — which is the single thing this panel exists to prevent.
 *
 * So it never renders a token figure. It renders how much is KNOWN, and names
 * what is not.
 */
export function TelemetryCoveragePanel() {
  const { t } = useT();
  const [rows, setRows] = useState<TelemetryCoverage[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    const fail = (err: unknown) => {
      // A failed query is not "no coverage": saying 0% here would invent a
      // measurement out of a network error.
      if (alive) setError(err instanceof Error ? err.message : String(err));
    };
    try {
      // try/catch as well as .catch(): a synchronous throw (the API surface
      // missing entirely) must degrade to "unknown" like any other failure.
      // This panel is mounted inside a shared zone, and telemetry is never
      // worth taking the page down for — the same rule the bridge follows.
      telemetry
        .coverage()
        .then((data) => {
          if (alive) setRows(data);
        })
        .catch(fail);
    } catch (err) {
      fail(err);
    }
    return () => {
      alive = false;
    };
  }, []);

  if (error) {
    return (
      <div className="telemetry-coverage telemetry-coverage--error" data-testid="telemetry-coverage-panel">
        <h3>{t('telemetryCoverage.title')}</h3>
        <p>{t('telemetryCoverage.readError', error)}</p>
        <p className="telemetry-coverage__note">
          {t('telemetryCoverage.errorNote')}
        </p>
      </div>
    );
  }

  if (rows === null) {
    return (
      <div className="telemetry-coverage" data-testid="telemetry-coverage-panel">
        <h3>{t('telemetryCoverage.title')}</h3>
        <p>{t('telemetryCoverage.loading')}</p>
      </div>
    );
  }

  const cliRows = rows.filter((row) => row.sessions > 0);

  return (
    <div className="telemetry-coverage" data-testid="telemetry-coverage-panel">
      <h3>{t('telemetryCoverage.title')}</h3>
      <p className="telemetry-coverage__intro">{t('telemetryCoverage.intro')}</p>
      {cliRows.length === 0 ? (
        <p>{t('telemetryCoverage.empty')}</p>
      ) : (
        <div className="telemetry-coverage__table-wrap">
          <table className="telemetry-coverage__table">
            <thead>
              <tr>
                <th>{t('telemetryCoverage.agent')}</th>
                <th>{t('telemetryCoverage.sessions')}</th>
                <th>{t('telemetryCoverage.measured')}</th>
                <th>{t('telemetryCoverage.coverage')}</th>
              </tr>
            </thead>
            <tbody>
              {cliRows.map((row) => {
                const ratio = measuredRatio(row);
                const measured = row.attributed - row.attributed_without_counters;
                const unknown = row.sessions - measured;
                return (
                  <tr key={row.agent_type}>
                    <td>{AGENT_LABELS[row.agent_type as AgentType] ?? row.agent_type}</td>
                    <td>{row.sessions}</td>
                    <td>{measured}</td>
                    <td>
                      {/* `null` only happens with zero sessions, filtered above —
                          but a nullish check beats a NaN reaching the DOM. */}
                      {ratio === null ? '—' : `${Math.round(ratio * 100)} %`}
                      {unknown > 0 && (
                        <span
                          className="telemetry-coverage__unknown"
                          title={t('telemetryCoverage.unknownHint', unknown)}
                        >
                          {' '}· {t(
                            unknown === 1
                              ? 'telemetryCoverage.unknownOne'
                              : 'telemetryCoverage.unknownMany',
                            unknown,
                          )}
                        </span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      <p className="telemetry-coverage__note">
        {t('telemetryCoverage.note')}
      </p>
    </div>
  );
}
