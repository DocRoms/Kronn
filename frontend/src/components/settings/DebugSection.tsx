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
import { userError } from '../../lib/userError';
import type { ToastFn } from '../../hooks/useToast';
// Note: lucide-react 1.x removed brand icons (Github, Gitlab, …) — use
// `ExternalLink` for the GitHub-issue CTA. Brand icons live in
// `simple-icons` if we ever want to re-add them.
import { AlertTriangle, Bug, Copy, ExternalLink, HardDrive, MessageSquare, Pause, Play, RefreshCw, Trash2 } from 'lucide-react';
import '../../pages/SettingsPage.css';

export interface DebugSectionProps {
  serverDebugMode: boolean;
  setServerDebugMode: (v: boolean) => void;
  debugModeNeedsRestart: boolean;
  setDebugModeNeedsRestart: (v: boolean) => void;
  discussionNotesEnabled: boolean;
  setDiscussionNotesEnabled: (v: boolean) => void;
  toast: ToastFn;
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
  discussionNotesEnabled,
  setDiscussionNotesEnabled,
  toast,
  t,
}: DebugSectionProps) {
  const [lines, setLines] = useState<string[]>([]);
  const [buffered, setBuffered] = useState(0);
  const [capacity, setCapacity] = useState(0);
  const [loading, setLoading] = useState(true);
  const [follow, setFollow] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const viewerRef = useRef<HTMLPreElement | null>(null);

  // Storage-weight indicator. Held locally so this section owns its own
  // config round-trip instead of widening the parent's props.
  const MIB = 1024 * 1024;
  const [weightEnabled, setWeightEnabled] = useState<boolean | null>(null);
  const [amberMib, setAmberMib] = useState('');
  const [redMib, setRedMib] = useState('');
  const [weightSaving, setWeightSaving] = useState(false);
  const [weightError, setWeightError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    configApi
      .getServerConfig()
      .then(cfg => {
        if (cancelled) return;
        const weight = cfg.discussion_weight;
        setWeightEnabled(weight?.enabled ?? true);
        setAmberMib(String(Math.round((weight?.amber_bytes ?? 0) / MIB)));
        setRedMib(String(Math.round((weight?.red_bytes ?? 0) / MIB)));
      })
      .catch(() => {
        // Leaving it null keeps the controls out rather than showing a
        // guessed state the user could then "save" over the real one.
        if (!cancelled) setWeightEnabled(null);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Persists the whole section at once: a pair is only meaningful together. */
  const saveWeightConfig = async (next: { enabled: boolean; amber: number; red: number }) => {
    if (!Number.isFinite(next.amber) || !Number.isFinite(next.red) || next.amber <= 0 || next.amber >= next.red) {
      setWeightError(t('settings.discWeightInvalid'));
      return false;
    }
    setWeightError(null);
    setWeightSaving(true);
    try {
      await configApi.setServerConfig({
        discussion_weight: {
          enabled: next.enabled,
          amber_bytes: Math.round(next.amber * MIB),
          red_bytes: Math.round(next.red * MIB),
        },
      });
      return true;
    } catch (error) {
      toast(t('common.actionFailed', userError(error)), 'error');
      return false;
    } finally {
      setWeightSaving(false);
    }
  };

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
    let active = true;
    debugApi.getLogs(TAIL_LINES)
      .then(resp => {
        if (!active) return;
        setLines(resp.lines);
        setBuffered(resp.buffered);
        setCapacity(resp.capacity);
      })
      .catch(error => {
        if (active) setError(error instanceof Error ? error.message : String(error));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => { active = false; };
  }, []);

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
                  const prevNeedsRestart = debugModeNeedsRestart;
                  setServerDebugMode(next);
                  setDebugModeNeedsRestart(true);
                  // No refetch feeds this toggle — without an explicit revert
                  // the switch would stick at a value the backend never saved.
                  try { await configApi.setServerConfig({ debug_mode: next }); }
                  catch (err) {
                    setServerDebugMode(!next);
                    setDebugModeNeedsRestart(prevNeedsRestart);
                    toast(t('common.actionFailed', userError(err)), 'error');
                  }
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

        <div className="mt-8">
          <div className="flex-row gap-4 mb-3" style={{ alignItems: 'center' }}>
            <MessageSquare size={12} className="text-tertiary" />
            <span className="label" style={{ marginBottom: 0 }}>{t('settings.discussionNotes')}</span>
            <button
              type="button"
              role="switch"
              aria-label={t('settings.discussionNotes')}
              aria-checked={discussionNotesEnabled}
              className="set-agent-access-switch"
              style={{ marginLeft: 'auto' }}
              onClick={async () => {
                const previous = discussionNotesEnabled;
                const next = !previous;
                setDiscussionNotesEnabled(next);
                try {
                  await configApi.setServerConfig({ discussion_notes_enabled: next });
                } catch (error) {
                  setDiscussionNotesEnabled(previous);
                  toast(t('common.actionFailed', userError(error)), 'error');
                }
              }}
            >
              <span className="set-toggle-track" data-on={discussionNotesEnabled}>
                <span
                  className="set-toggle-thumb"
                  data-on={discussionNotesEnabled}
                  style={{ left: discussionNotesEnabled ? 16 : 1 }}
                />
              </span>
              <span className={discussionNotesEnabled ? 'text-accent' : 'text-muted'}>
                {discussionNotesEnabled ? t('config.enabled') : t('config.disabled')}
              </span>
            </button>
          </div>
          <div className="set-hint-xs">{t('settings.discussionNotesHint')}</div>
        </div>

        {weightEnabled !== null && (
          <div className="mt-8" data-testid="disc-weight-settings">
            <div className="flex-row gap-4 mb-3" style={{ alignItems: 'center' }}>
              <HardDrive size={12} className="text-tertiary" />
              <span className="label" style={{ marginBottom: 0 }}>{t('settings.discWeightTitle')}</span>
              <button
                type="button"
                role="switch"
                aria-label={t('settings.discWeightTitle')}
                aria-checked={weightEnabled}
                className="set-agent-access-switch"
                style={{ marginLeft: 'auto' }}
                disabled={weightSaving}
                data-testid="disc-weight-toggle"
                onClick={async () => {
                  const previous = weightEnabled;
                  const next = !previous;
                  setWeightEnabled(next);
                  const ok = await saveWeightConfig({
                    enabled: next,
                    amber: Number(amberMib),
                    red: Number(redMib),
                  });
                  if (!ok) setWeightEnabled(previous);
                }}
              >
                <span className="set-toggle-track" data-on={weightEnabled}>
                  <span className="set-toggle-thumb" data-on={weightEnabled} style={{ left: weightEnabled ? 16 : 1 }} />
                </span>
                <span className={weightEnabled ? 'text-accent' : 'text-muted'}>
                  {weightEnabled ? t('config.enabled') : t('config.disabled')}
                </span>
              </button>
            </div>
            <div className="flex-row gap-4" style={{ alignItems: 'flex-end' }}>
              <label className="flex-col gap-1">
                <span className="text-2xs text-muted">{t('settings.discWeightAmber')}</span>
                <input
                  type="number"
                  min={1}
                  className="input input-compact"
                  style={{ width: 90 }}
                  value={amberMib}
                  data-testid="disc-weight-amber"
                  disabled={!weightEnabled || weightSaving}
                  onChange={e => setAmberMib(e.target.value)}
                />
              </label>
              <label className="flex-col gap-1">
                <span className="text-2xs text-muted">{t('settings.discWeightRed')}</span>
                <input
                  type="number"
                  min={1}
                  className="input input-compact"
                  style={{ width: 90 }}
                  value={redMib}
                  data-testid="disc-weight-red"
                  disabled={!weightEnabled || weightSaving}
                  onChange={e => setRedMib(e.target.value)}
                />
              </label>
              <button
                type="button"
                className="btn btn-ghost"
                disabled={!weightEnabled || weightSaving}
                data-testid="disc-weight-save"
                onClick={async () => {
                  const ok = await saveWeightConfig({
                    enabled: weightEnabled,
                    amber: Number(amberMib),
                    red: Number(redMib),
                  });
                  if (ok) toast(t('settings.discWeightSaved'), 'success');
                }}
              >
                {t('common.save')}
              </button>
            </div>
            {weightError && (
              <div className="set-hint-xs text-error" role="alert" data-testid="disc-weight-error">
                {weightError}
              </div>
            )}
            <div className="set-hint-xs">{t('settings.discWeightHint')}</div>
          </div>
        )}

        {/* Live viewer — always visible (we capture at info even when
            debug_mode is off, so there's always SOMETHING useful to show). */}
        <div className="mt-8">
          <div className="flex-row gap-4 mb-3" style={{ alignItems: 'center' }}>
            <span className="label" style={{ marginBottom: 0 }}>{t('settings.debugLogsTitle')}</span>
            <div className="flex-row gap-2" style={{ marginLeft: 'auto', alignItems: 'center' }}>
              <button
                type="button"
                className="btn btn-ghost"
                onClick={() => setFollow(f => !f)}
                title={follow ? t('settings.debugLogsStopFollow') : t('settings.debugLogsStartFollow')}
                aria-pressed={follow}
              >
                {follow ? <Pause size={12} /> : <Play size={12} />}
                <span className="text-xs">{follow ? t('settings.debugLogsStopFollow') : t('settings.debugLogsStartFollow')}</span>
              </button>
              <button
                type="button"
                className="btn btn-ghost"
                onClick={() => void refresh()}
                disabled={loading}
                title={t('settings.debugLogsRefresh')}
              >
                <RefreshCw size={12} className={loading ? 'animate-spin' : ''} />
                <span className="text-xs">{t('settings.debugLogsRefresh')}</span>
              </button>
              <button
                type="button"
                className="btn btn-ghost"
                onClick={() => void handleCopy()}
                disabled={lines.length === 0}
                title={t('settings.debugLogsCopy')}
              >
                <Copy size={12} />
                <span className="text-xs">{t('settings.debugLogsCopy')}</span>
              </button>
              <button
                type="button"
                className="btn btn-ghost"
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
              className="btn btn-accent"
              onClick={() => void handleReportBug()}
              disabled={reporting}
              title={t('settings.debugReportHint')}
            >
              <ExternalLink size={13} />
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
