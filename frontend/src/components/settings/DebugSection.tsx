/**
 * Settings > Debug card.
 *
 * Pulled out of SettingsPage into its own component so it's easy to find
 * when diagnosing cross-platform issues (macOS agent detection, scanner
 * host-path mapping, etc.) and so the log-viewer concerns don't clutter
 * the main page.
 *
 * Backend: `GET /api/debug/logs?lines=N` returns the last N lines from
 * the in-memory ringbuffer fed by every `tracing` event. No file on
 * disk. Capture continues regardless of `debug_mode` — the flag only
 * controls the `tracing` level (info vs. debug), i.e. how verbose the
 * captured stream is.
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { agents as agentsApi, config as configApi, debugApi, fetchHealth } from '../../lib/api';
import { buildIssueUrl, KRONN_REPO_URL } from '../../lib/bug-report';
import { AlertTriangle, Bug, Copy, Github, Pause, Play, RefreshCw, Trash2 } from 'lucide-react';
import '../../pages/SettingsPage.css';

export interface DebugSectionProps {
  serverDebugMode: boolean;
  setServerDebugMode: (v: boolean) => void;
  debugModeNeedsRestart: boolean;
  setDebugModeNeedsRestart: (v: boolean) => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

/** How many lines to request on each refresh. */
const TAIL_LINES = 300;
/** Auto-refresh interval when the "follow" toggle is on. */
const AUTO_REFRESH_MS = 2000;

export function DebugSection({
  serverDebugMode,
  setServerDebugMode,
  debugModeNeedsRestart,
  setDebugModeNeedsRestart,
  t,
}: DebugSectionProps) {
  const [lines, setLines] = useState<string[]>([]);
  const [buffered, setBuffered] = useState(0);
  const [capacity, setCapacity] = useState(0);
  const [loading, setLoading] = useState(false);
  const [follow, setFollow] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const viewerRef = useRef<HTMLPreElement | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const resp = await debugApi.getLogs(TAIL_LINES);
      setLines(resp.lines);
      setBuffered(resp.buffered);
      setCapacity(resp.capacity);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  // Initial load + auto-refresh when following.
  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!follow) return;
    const id = setInterval(() => { void refresh(); }, AUTO_REFRESH_MS);
    return () => clearInterval(id);
  }, [follow, refresh]);

  // Keep the viewer pinned to the bottom on refresh when following — mimics
  // the `tail -f` feel. If the user scrolled up manually, respect that and
  // don't yank them back down.
  useEffect(() => {
    const el = viewerRef.current;
    if (!el || !follow) return;
    el.scrollTop = el.scrollHeight;
  }, [lines, follow]);

  const handleClear = useCallback(async () => {
    try {
      await debugApi.clearLogs();
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [refresh]);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(lines.join('\n'));
    } catch {
      // Clipboard may be unavailable (e.g. non-HTTPS origin in a browser
      // that gates it). Silent failure is fine — user still sees the text.
    }
  }, [lines]);

  // "Report a bug" — one-click flow.
  // Fetches version + host_os + detected agents in parallel, merges with
  // the already-loaded log buffer, and opens a GitHub issue form with
  // everything pre-filled. Secrets are redacted inside `buildIssueUrl`.
  // `reporting` drives the button's disabled/spinner state so users can
  // tell something is happening during the parallel fetches.
  const [reporting, setReporting] = useState(false);
  const handleReportBug = useCallback(async () => {
    setReporting(true);
    try {
      // Fire both env queries in parallel; both are best-effort so we
      // swallow errors individually rather than aborting the whole flow
      // when one endpoint hiccups. At worst the issue is missing a field,
      // which the user can fill on GitHub.
      const [health, agents] = await Promise.all([
        fetchHealth().catch(() => null),
        agentsApi.detect().catch(() => null),
      ]);
      const agentsSummary = (agents ?? []).map(a => {
        const status = a.installed ? 'installed' : (a.runtime_available ? 'runtime' : 'missing');
        const ver = a.version ? ` v${a.version}` : '';
        const loc = a.path ? ` (${a.path})` : '';
        return `${a.name}: ${status}${ver}${loc}`;
      });
      const url = buildIssueUrl({
        kronnVersion: health?.version ?? null,
        hostOs: health?.host_os ?? null,
        agentsSummary,
        logLines: lines,
        userAgent: typeof navigator !== 'undefined' ? navigator.userAgent : undefined,
      });
      // `noopener,noreferrer` keeps GitHub out of our window.opener chain.
      window.open(url, '_blank', 'noopener,noreferrer');
    } finally {
      setReporting(false);
    }
  }, [lines]);

  return (
    <div id="settings-debug" className="set-card">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <Bug size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('settings.debugSection')}</span>
          {/* Pulsing "LIVE" badge when debug_mode is active — the user
              explicitly asked for a visible signal so there's no ambiguity
              about whether verbose capture is on. Pure visual; the same
              state is also reflected in the sidebar nav via a pulsing dot
              next to "Debug". */}
          {serverDebugMode && (
            <span className="set-debug-live-badge" role="status" aria-label={t('settings.debugLiveLabel')}>
              <span className="set-debug-live-dot" aria-hidden="true" />
              {t('settings.debugLiveLabel')}
            </span>
          )}
          <span className="text-sm text-dim" style={{ marginLeft: 'auto' }}>
            {capacity > 0 ? t('settings.debugBufferedCount', buffered, capacity) : ''}
          </span>
        </div>

        {/* Toggle row — same visual language as the stall-timeout / max-agents
            controls so the two cards feel consistent. */}
        <div>
          <div className="flex-row gap-4 mb-3" style={{ alignItems: 'center' }}>
            <span className="label" style={{ marginBottom: 0 }}>{t('settings.debugMode')}</span>
            <label className="flex-row gap-2" style={{ cursor: 'pointer', marginLeft: 'auto', alignItems: 'center' }}>
              <input
                type="checkbox"
                checked={serverDebugMode}
                onChange={async e => {
                  const next = e.target.checked;
                  setServerDebugMode(next);
                  setDebugModeNeedsRestart(true);
                  try { await configApi.setServerConfig({ debug_mode: next }); } catch {}
                }}
              />
              <span className="text-sm">{serverDebugMode ? t('common.on') : t('common.off')}</span>
            </label>
          </div>
          <div className="set-hint-xs">{t('settings.debugModeHint')}</div>
          {debugModeNeedsRestart && (
            <div className="set-warning-callout">
              <AlertTriangle size={12} className="text-warning flex-shrink-0" />
              <span className="text-xs" style={{ color: 'rgba(var(--kr-warning-amber-rgb), 0.8)', lineHeight: 1.4 }}>
                {t('settings.debugModeRestart')}
              </span>
            </div>
          )}
        </div>

        {/* Live viewer — always visible (we capture at info even when
            debug_mode is off, so there's always SOMETHING useful to show). */}
        <div className="mt-8">
          <div className="flex-row gap-4 mb-3" style={{ alignItems: 'center' }}>
            <span className="label" style={{ marginBottom: 0 }}>{t('settings.debugLogsTitle')}</span>
            <div className="flex-row gap-2" style={{ marginLeft: 'auto', alignItems: 'center' }}>
              <button
                type="button"
                className="btn-ghost"
                onClick={() => setFollow(f => !f)}
                title={follow ? t('settings.debugLogsStopFollow') : t('settings.debugLogsStartFollow')}
                aria-pressed={follow}
              >
                {follow ? <Pause size={12} /> : <Play size={12} />}
                <span className="text-xs">{follow ? t('settings.debugLogsStopFollow') : t('settings.debugLogsStartFollow')}</span>
              </button>
              <button
                type="button"
                className="btn-ghost"
                onClick={() => void refresh()}
                disabled={loading}
                title={t('settings.debugLogsRefresh')}
              >
                <RefreshCw size={12} className={loading ? 'animate-spin' : ''} />
                <span className="text-xs">{t('settings.debugLogsRefresh')}</span>
              </button>
              <button
                type="button"
                className="btn-ghost"
                onClick={() => void handleCopy()}
                disabled={lines.length === 0}
                title={t('settings.debugLogsCopy')}
              >
                <Copy size={12} />
                <span className="text-xs">{t('settings.debugLogsCopy')}</span>
              </button>
              <button
                type="button"
                className="btn-ghost"
                onClick={() => void handleClear()}
                disabled={buffered === 0}
                title={t('settings.debugLogsClear')}
              >
                <Trash2 size={12} />
                <span className="text-xs">{t('settings.debugLogsClear')}</span>
              </button>
            </div>
          </div>

          {error && (
            <div className="set-warning-callout" role="alert">
              <AlertTriangle size={12} className="text-error flex-shrink-0" />
              <span className="text-xs">{error}</span>
            </div>
          )}

          {/* Monospace viewer — alignment relies on the 5-char level tag
              emitted by the backend (`BufferLayer::format_line`). */}
          <pre
            ref={viewerRef}
            className="set-debug-viewer"
            aria-label={t('settings.debugLogsTitle')}
            tabIndex={0}
          >
            {lines.length === 0
              ? <span className="text-dim">{t('settings.debugLogsEmpty')}</span>
              : lines.join('\n')}
          </pre>

          <div className="set-hint-xs">
            {t('settings.debugLogsHint')}
          </div>

          {/* Report-a-bug CTA — visually distinct so it stands out from
              the refresh/copy/clear row above (those act on the viewer;
              this one ships info OUT to GitHub). Secret redaction happens
              client-side inside `buildIssueUrl`. */}
          <div className="mt-8 flex-row gap-3" style={{ alignItems: 'center' }}>
            <button
              type="button"
              className="btn-accent"
              onClick={() => void handleReportBug()}
              disabled={reporting}
              title={t('settings.debugReportHint')}
            >
              <Github size={13} />
              <span>{t('settings.debugReportCta')}</span>
            </button>
            <span className="set-hint-xs" style={{ flex: 1 }}>
              {t('settings.debugReportHint')}{' '}
              <a
                href={`${KRONN_REPO_URL}/issues`}
                target="_blank"
                rel="noopener noreferrer"
                style={{ color: 'var(--kr-accent)', textDecoration: 'underline' }}
              >
                {t('settings.debugReportIssueList')}
              </a>
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
