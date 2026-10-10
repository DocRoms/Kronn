/**
 * KT-1108 — what a reply's agent is doing, live, from the server's own
 * reports: its startup phases with their times, the tools it calls by
 * category, the inactivity delay left while it is silent, and why it stopped.
 * Never a tool's name, a path, a URL or any agent text: the frames carry none.
 */
import { useId, useState } from 'react';
import { Check, ChevronDown, ChevronRight, Hourglass, OctagonX } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { ACTIVITY_CATEGORY_ICONS, activityCategoryLabel, asActivityCategory } from '../lib/activity-category';
import {
  formatDuration,
  formatStepDuration,
  idleRemainingMs,
  phaseElapsedMs,
  runElapsedMs,
  silentMs,
  type LiveRun,
} from '../lib/agentRunProgress';
import { useSecondTicker } from '../hooks/useSecondTicker';
import type { AgentRunPhase } from '../types/generated';

/** A silence shorter than this is ordinary pacing, not worth a countdown. */
export const IDLE_COUNTDOWN_AFTER_MS = 5_000;

function relativeAge(iso: string, now: number, locale: string): string {
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return '';
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  const format = new Intl.RelativeTimeFormat(locale, { numeric: 'auto', style: 'narrow' });
  if (seconds < 60) return format.format(-seconds, 'second');
  if (seconds < 3600) return format.format(-Math.floor(seconds / 60), 'minute');
  return format.format(-Math.floor(seconds / 3600), 'hour');
}

export function AgentRunProgressPanel({ run, agentLabel }: { run: LiveRun; agentLabel: string }) {
  const { t, locale } = useT();
  const stopped = run.progress.stopped;
  // Ticks only while the run lives: a stopped run's durations are frozen.
  const now = useSecondTicker(!stopped);
  const [open, setOpen] = useState(false);
  const listId = useId();
  const { progress } = run;
  const phaseLabel = (phase: AgentRunPhase) => t(`runProgress.phase.${phase}`, agentLabel);
  const startup = progress.timeline;
  const beforeOutput = startup.some(mark => mark.phase === progress.phase);
  const remaining = idleRemainingMs(run, now);
  const silent = silentMs(run, now);
  const showCountdown = remaining !== null && silent >= IDLE_COUNTDOWN_AFTER_MS;
  const entries = progress.activity;

  return (
    <div className="disc-run-progress" data-testid="agent-run-progress" data-phase={progress.phase}>
      <div className="disc-run-progress-head">
        {/* Only the phase is announced; its ticking time is not read aloud. */}
        <span className="disc-run-progress-phase" role="status" aria-live="polite" data-testid="run-phase">
          {phaseLabel(progress.phase)}
          {progress.phase === 'opening_session' && progress.mcp_servers
            ? ` · ${t('runProgress.mcpServers', progress.mcp_servers)}`
            : ''}
        </span>
        <span className="disc-run-progress-time" aria-hidden="true" data-testid="run-phase-elapsed">
          {formatDuration(phaseElapsedMs(run, now))}
        </span>
        <span className="disc-run-progress-total" aria-hidden="true" data-testid="run-elapsed">
          {t('runProgress.total', formatDuration(runElapsedMs(run, now)))}
        </span>
      </div>

      {beforeOutput && startup.length > 1 && (
        <ol className="disc-run-progress-steps" aria-label={t('runProgress.steps')}>
          {startup.map((mark, index) => {
            const next = startup[index + 1];
            const current = !next;
            const spent = next ? next.at_ms - mark.at_ms : null;
            return (
              <li key={mark.phase} data-current={current} data-testid="run-step">
                {current
                  ? <span className="disc-pulse-dot" aria-hidden="true" />
                  : <Check size={10} aria-hidden="true" />}
                <span>{phaseLabel(mark.phase)}</span>
                {spent !== null && <span className="disc-run-progress-time">{formatStepDuration(spent)}</span>}
              </li>
            );
          })}
        </ol>
      )}

      {showCountdown && (
        <div className="disc-run-progress-idle" data-testid="run-idle-countdown">
          <Hourglass size={11} aria-hidden="true" />
          <span>{t('runProgress.idleLeft', formatDuration(silent), formatDuration(Math.ceil(remaining / 1000) * 1000))}</span>
        </div>
      )}

      {stopped && stopped !== 'finished' && (
        <div className="disc-run-progress-stop" role="alert" data-testid="run-stopped">
          <OctagonX size={11} aria-hidden="true" />
          <span>
            {t(
              `runProgress.stop.${stopped}`,
              agentLabel,
              stopped === 'idle' ? formatDuration(progress.silent_ms) : phaseLabel(progress.phase),
            )}
          </span>
        </div>
      )}

      <div className="disc-run-progress-activity">
        <button
          type="button"
          className="disc-logs-toggle"
          aria-expanded={open}
          aria-controls={listId}
          onClick={() => setOpen(value => !value)}
          data-testid="run-activity-toggle"
        >
          {open ? <ChevronDown size={10} aria-hidden="true" /> : <ChevronRight size={10} aria-hidden="true" />}
          {t('runProgress.activity', progress.tool_calls)}
        </button>
        {open && (
          <div id={listId}>
            {entries.length === 0 ? (
              <p className="disc-run-progress-empty">{t('runProgress.activityEmpty')}</p>
            ) : (
              <ul className="disc-run-progress-list" aria-label={t('runProgress.activityLabel')}>
                {entries.map((entry, index) => {
                  const Icon = ACTIVITY_CATEGORY_ICONS[asActivityCategory(entry.category)];
                  return (
                    <li key={`${entry.at}-${index}`} data-testid="run-activity-entry">
                      <Icon size={11} aria-hidden="true" />
                      <span>{activityCategoryLabel(t, entry.category)}</span>
                      <time className="disc-run-progress-time" dateTime={entry.at}>
                        {relativeAge(entry.at, now, locale)}
                      </time>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
