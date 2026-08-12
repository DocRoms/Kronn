import { useEffect, useState } from 'react';
import { telemetry, measuredRatio } from '../lib/api';
import type { TelemetryCoverage } from '../types/generated';

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
      <div className="telemetry-coverage telemetry-coverage--error">
        <h3>Couverture télémétrie</h3>
        <p>Impossible de lire la couverture : {error}</p>
        <p className="telemetry-coverage__note">
          Ce n’est pas une couverture nulle — c’est une couverture inconnue.
        </p>
      </div>
    );
  }

  if (rows === null) {
    return (
      <div className="telemetry-coverage">
        <h3>Couverture télémétrie</h3>
        <p>Lecture…</p>
      </div>
    );
  }

  const cliRows = rows.filter((row) => row.sessions > 0);

  return (
    <div className="telemetry-coverage">
      <h3>Couverture télémétrie</h3>
      {cliRows.length === 0 ? (
        <p>Aucune session CLI enregistrée.</p>
      ) : (
        <table className="telemetry-coverage__table">
          <thead>
            <tr>
              <th>Agent</th>
              <th>Sessions</th>
              <th>Mesurées</th>
              <th>Couverture</th>
            </tr>
          </thead>
          <tbody>
            {cliRows.map((row) => {
              const ratio = measuredRatio(row);
              const measured = row.attributed - row.attributed_without_counters;
              const unknown = row.sessions - measured;
              return (
                <tr key={row.agent_type}>
                  <td>{row.agent_type}</td>
                  <td>{row.sessions}</td>
                  <td>{measured}</td>
                  <td>
                    {/* `null` only happens with zero sessions, filtered above —
                        but a nullish check beats a NaN reaching the DOM. */}
                    {ratio === null ? '—' : `${Math.round(ratio * 100)} %`}
                    {unknown > 0 && (
                      <span
                        className="telemetry-coverage__unknown"
                        title={
                          `${unknown} session(s) sans compteur natif lisible. ` +
                          `Leur coût est INCONNU, pas nul : Kronn ne les a pas lancées, ` +
                          `donc il n’a aucun compteur propre pour ce qu’elles publient.`
                        }
                      >
                        {' '}· {unknown} inconnue{unknown > 1 ? 's' : ''}
                      </span>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      <p className="telemetry-coverage__note">
        Une session sans compteur natif est <strong>inconnue</strong>, pas gratuite.
        Codex et Copilot n’ont pas encore de collecteur.
      </p>
    </div>
  );
}
