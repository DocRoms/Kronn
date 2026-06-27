import { useState, useEffect, useMemo } from 'react';
import { Leaf, ExternalLink, Loader2, Info, X, Square, ChevronDown, ChevronUp, HelpCircle, ArrowUpCircle, Copy } from 'lucide-react';
import { rtk as rtkApi } from '../../lib/api';
import type { AgentDetection } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';
import { RTK_APPLICABLE } from '../../lib/constants';

interface CompressionSectionProps {
  agents: AgentDetection[];
  /** Called after a successful `activate` so the parent can refetch agents
   *  and see the `rtk_hook_configured` flags flip to true. */
  onActivated?: () => void;
  /** Optional — when provided, activation errors surface as a toast
   *  instead of a silent console.warn. */
  toast?: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

// RTK_APPLICABLE now lives in lib/constants.ts (shared with the new-discussion
// "no RTK" cost warning). Mirrors `rtk_flag_for` in backend/src/api/rtk.rs.

const RTK_GITHUB_URL = 'https://github.com/rtk-ai/rtk';
const RTK_INSTALL_CMD = 'curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/main/install.sh | sh';

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}k`;
  return `${n}`;
}

export function CompressionSection({ agents, onActivated, toast, t }: CompressionSectionProps) {
  const [activating, setActivating] = useState(false);
  const [showInstallModal, setShowInstallModal] = useState(false);
  const [showDetails, setShowDetails] = useState(false);
  const [showSobriety, setShowSobriety] = useState(false);
  const [savings, setSavings] = useState<{
    total: number;
    ratio: number;
    samples: number;
    available: boolean;
  } | null>(null);
  // Freshness pill — only renders when the backend has both an installed
  // and a latest-known version AND the comparator (in Rust) says the
  // installed is older. We don't recompute it client-side to keep the
  // single source of truth on the backend (see `core::versions`).
  const [versionInfo, setVersionInfo] = useState<{
    installed: string | null;
    latest: string;
    updateAvailable: boolean;
    updateCommand: string;
  } | null>(null);
  const [showUpdateModal, setShowUpdateModal] = useState(false);

  // Any installed RTK-applicable agent => binary either detected or not.
  // We only trust `rtk_available` from an installed agent because the flag
  // is populated by `find_binary` during detection; a non-detected agent
  // row may have `rtk_available: false` simply because detection skipped.
  const rtkBinaryAvailable = agents.some(a => a.rtk_available);

  const { applicable, configured } = useMemo(() => {
    const app = agents.filter(a =>
      RTK_APPLICABLE.has(a.agent_type) && (a.installed || a.runtime_available),
    );
    const cfg = app.filter(a => a.rtk_hook_configured).length;
    return { applicable: app.length, configured: cfg };
  }, [agents]);

  const remaining = applicable - configured;
  const allConfigured = applicable > 0 && remaining === 0;
  const noneConfigured = configured === 0;

  // Fetch savings once; only refresh on explicit activation so we don't
  // poll `rtk gain` on every re-render of a large agent list.
  useEffect(() => {
    let cancelled = false;
    if (!rtkBinaryAvailable) {
      setSavings(null);
      setVersionInfo(null);
      return;
    }
    rtkApi.savings().then(s => {
      if (!cancelled) setSavings({
        total: s.total_tokens_saved,
        ratio: s.ratio_percent,
        samples: s.sample_count,
        available: s.available,
      });
    }).catch(() => {
      if (!cancelled) setSavings({ total: 0, ratio: 0, samples: 0, available: false });
    });
    rtkApi.version().then(v => {
      if (!cancelled) setVersionInfo({
        installed: v.installed,
        latest: v.latest_known,
        updateAvailable: v.update_available,
        updateCommand: v.update_command,
      });
    }).catch(() => {
      if (!cancelled) setVersionInfo(null);
    });
    return () => { cancelled = true; };
  }, [rtkBinaryAvailable, configured]);

  const handleActivate = async () => {
    setActivating(true);
    try {
      // Target ONLY the agents that aren't already wired. Pre-fix the
      // button always sent the full applicable list, including the
      // already-configured ones — `rtk init -g` is a no-op for them
      // but the backend's success aggregation hid per-agent failures
      // for the ONE agent the user actually expected to flip. User
      // report 2026-05-10: clicking "Enable on the 1 remaining" did
      // strictly nothing (TD-20260510-rtk-last-agent-button). Now we
      // send the unconfigured agents only AND surface per-agent
      // results so a silent-no-op becomes visible.
      const targetAgents = agents
        .filter(a =>
          RTK_APPLICABLE.has(a.agent_type)
          && (a.installed || a.runtime_available)
          && !a.rtk_hook_configured,
        )
        .map(a => a.agent_type);
      if (targetAgents.length === 0) {
        toast?.(t('config.rtk.activateNoTarget'), 'info');
        return;
      }
      const res = await rtkApi.activate(targetAgents);
      // Backend returns `success: false` when `rtk init -g` exits non-zero.
      // The human-facing title stays short; full stderr ships in `copyable`
      // so the user can paste it to their tech colleague without screen-
      // shotting. `persistent` defaults to true for errors.
      if (!res.success) {
        const stderr = (res.stderr || res.stdout || '').trim();
        toast?.(t('config.rtk.activateError'), 'error', {
          copyable: stderr || undefined,
        });
        console.warn('RTK activate non-zero exit:', res);
      } else {
        // Per-agent breakdown — surface partial failures even when the
        // aggregate `success: true`. Pre-fix, a single agent failing
        // silently inside an otherwise-OK call showed a green toast and
        // the user didn't know why their CTA "did nothing".
        // Defensive `?? []` for older backends that don't yet return
        // the per_agent array (and for unit-test mocks that omit it).
        const failed = (res.per_agent ?? []).filter(a => !a.success);
        if (failed.length > 0) {
          const detail = failed
            .map(a => `${a.agent_type}: ${(a.stderr || a.stdout || '').trim() || '(no output)'}`)
            .join('\n\n');
          toast?.(t('config.rtk.activatePartial', failed.length, targetAgents.length), 'warning', {
            copyable: detail,
          });
          console.warn('RTK activate per-agent failures:', failed);
        } else {
          toast?.(t('config.rtk.activateSuccess'), 'success');
        }
      }
    } catch (e) {
      toast?.(t('config.rtk.activateError'), 'error', {
        copyable: String(e),
      });
      console.warn('RTK activate failed:', e);
    } finally {
      setActivating(false);
      // Defer the parent refetch slightly to let RTK's filesystem
      // writes flush — without this, the `agentsApi.detect()` re-run
      // would race the AGENTS.md / settings.json writes and the badge
      // would stick on its old state until the next manual refetch.
      // Always fires (even on error) so the user sees the post-attempt
      // state, never a stale snapshot.
      setTimeout(() => onActivated?.(), 200);
    }
  };

  const handleDeactivate = async () => {
    setActivating(true);
    try {
      // Only target agents that are currently configured — sending
      // `--uninstall` to a never-wired agent is a no-op + extra noise
      // in the toast. We're conservative on purpose.
      const configuredAgents = agents
        .filter(a => RTK_APPLICABLE.has(a.agent_type) && a.rtk_hook_configured)
        .map(a => a.agent_type);
      if (configuredAgents.length === 0) return;
      const res = await rtkApi.deactivate(configuredAgents);
      if (!res.success) {
        const stderr = (res.stderr || res.stdout || '').trim();
        toast?.(t('config.rtk.deactivateError'), 'error', {
          copyable: stderr || undefined,
        });
        console.warn('RTK deactivate non-zero exit:', res);
      } else {
        toast?.(t('config.rtk.deactivateSuccess'), 'success');
      }
    } catch (e) {
      toast?.(t('config.rtk.deactivateError'), 'error', {
        copyable: String(e),
      });
      console.warn('RTK deactivate failed:', e);
    } finally {
      setActivating(false);
      // Same FS-flush deferral as `handleActivate`.
      setTimeout(() => onActivated?.(), 200);
    }
  };

  // When no RTK-applicable agent is installed, the section has nothing to
  // act on. Hide entirely to keep Maria's Config page clean.
  if (applicable === 0) return null;

  // Visual state derives from `allConfigured` (green, unobtrusive) vs any
  // other case (amber, call-to-action). We ask Intl to format the counter.
  const stateClass = allConfigured ? 'set-compression-state-ok' : 'set-compression-state-attn';

  return (
    <>
      <div className={`set-compression ${stateClass}`}>
        <div className="set-compression-head">
          <div className="set-compression-icon">
            <Leaf size={16} />
          </div>
          <div className="flex-1">
            <div className="flex-row gap-2" style={{ alignItems: 'center' }}>
              <span className="font-semibold text-base">{t('config.rtk.title')}</span>
              <button
                type="button"
                className="set-compression-info-btn"
                onClick={() => setShowSobriety(v => !v)}
                aria-expanded={showSobriety}
                aria-label={t('config.rtk.sobrietyTitle')}
                title={t('config.rtk.sobrietyTitle')}
              >
                <HelpCircle size={12} />
              </button>
              {/* Freshness pill — only shown when the backend confirms
               *  installed < latest_known under lenient semver. Click
               *  opens a small modal with a copyable upgrade command
               *  (RTK install.sh is idempotent — re-runs upgrade). */}
              {versionInfo?.updateAvailable && versionInfo.installed && (
                <button
                  type="button"
                  className="set-compression-update-pill"
                  onClick={() => setShowUpdateModal(true)}
                  aria-label={t('config.rtk.updateAvailableAria', versionInfo.installed, versionInfo.latest)}
                  title={t('config.rtk.updateAvailableTitle', versionInfo.installed, versionInfo.latest)}
                >
                  <ArrowUpCircle size={10} />
                  <span>{t('config.rtk.updateAvailable', versionInfo.latest)}</span>
                </button>
              )}
            </div>
            <p className="set-compression-explainer">{t('config.rtk.explainer')}</p>
            {showSobriety && (
              <div className="set-compression-sobriety">
                <div className="set-compression-sobriety-title">
                  {t('config.rtk.sobrietyTitle')}
                </div>
                <p className="set-compression-sobriety-body">
                  {t('config.rtk.sobrietyBody')}
                </p>
              </div>
            )}
          </div>
        </div>

        <div className="set-compression-status">
          <span className="set-compression-dot" aria-hidden="true" />
          <span className="set-compression-status-text">
            {allConfigured
              ? t('config.rtk.stateAll')
              : noneConfigured
                ? t('config.rtk.stateNone')
                : t('config.rtk.statePartial', configured, applicable)}
          </span>
          {savings?.available && savings.total > 0 && (
            <>
              <span className="set-compression-savings">
                · {t('config.rtk.savings', formatTokens(savings.total))}
              </span>
              <button
                type="button"
                className="set-compression-details-toggle"
                onClick={() => setShowDetails(v => !v)}
                aria-expanded={showDetails}
              >
                {t('config.rtk.detailsToggle')}
                {showDetails ? <ChevronUp size={10} /> : <ChevronDown size={10} />}
              </button>
            </>
          )}
        </div>

        {showDetails && savings?.available && (
          <div className="set-compression-details">
            <div className="set-compression-stat">
              <div className="set-compression-stat-label">{t('config.rtk.statTokens')}</div>
              <div className="set-compression-stat-value">{formatTokens(savings.total)}</div>
            </div>
            <div className="set-compression-stat">
              <div className="set-compression-stat-label">{t('config.rtk.statRatio')}</div>
              <div className="set-compression-stat-value">{savings.ratio.toFixed(0)}%</div>
            </div>
            <div className="set-compression-stat">
              <div className="set-compression-stat-label">{t('config.rtk.statSamples')}</div>
              <div className="set-compression-stat-value">{savings.samples.toLocaleString()}</div>
            </div>
          </div>
        )}

        {!rtkBinaryAvailable && (
          <div className="set-compression-warning">
            <Info size={12} />
            <span>{t('config.rtk.notInstalled')}</span>
          </div>
        )}

        <div className="set-compression-actions">
          {!allConfigured && (
            rtkBinaryAvailable ? (
              <button
                type="button"
                className="set-compression-cta"
                onClick={handleActivate}
                disabled={activating}
              >
                {activating ? <Loader2 size={12} className="spin" /> : <Square size={10} style={{ fill: 'currentColor' }} />}
                {activating
                  ? t('config.rtk.activating')
                  : noneConfigured
                    ? t('config.rtk.activateAll')
                    : t('config.rtk.activateRemaining', remaining)}
              </button>
            ) : (
              <button
                type="button"
                className="set-compression-cta"
                onClick={() => setShowInstallModal(true)}
              >
                {t('config.rtk.installCta')}
              </button>
            )
          )}
          {/* Deactivate button — visible whenever at least one agent has
              an RTK hook wired. Lets the user back out without manually
              editing settings.json / AGENTS.md / shell rc. */}
          {rtkBinaryAvailable && configured > 0 && (
            <button
              type="button"
              className="set-compression-uninstall"
              onClick={handleDeactivate}
              disabled={activating}
              title={t('config.rtk.deactivateHint')}
            >
              {t('config.rtk.deactivate', configured)}
            </button>
          )}

          <span className="set-compression-attrib">
            {t('config.rtk.poweredBy')}{' '}
            <a
              href={RTK_GITHUB_URL}
              target="_blank"
              rel="noreferrer"
              className="set-compression-link"
            >
              RTK <ExternalLink size={10} />
            </a>
            {' '}({t('config.rtk.openSource')})
          </span>
        </div>
      </div>

      {showUpdateModal && versionInfo && (
        <div className="dash-modal-overlay" onClick={() => setShowUpdateModal(false)}>
          <div
            className="dash-modal set-compression-modal"
            onClick={e => e.stopPropagation()}
            role="dialog"
            aria-modal="true"
            aria-labelledby="rtk-update-title"
            onKeyDown={e => { if (e.key === 'Escape') setShowUpdateModal(false); }}
          >
            <div className="dash-modal-header">
              <h3 id="rtk-update-title" className="dash-modal-title">
                {t('config.rtk.updateModalTitle')}
              </h3>
              <button
                onClick={() => setShowUpdateModal(false)}
                className="dash-modal-close"
                aria-label="Close"
              >
                <X size={16} />
              </button>
            </div>
            <div className="set-compression-modal-body">
              <p>
                {t('config.rtk.updateModalBody',
                  versionInfo.installed ?? '?',
                  versionInfo.latest)}
              </p>
              <div className="set-compression-install-label">{t('config.rtk.installCommand')}</div>
              <pre className="set-compression-install-cmd">{versionInfo.updateCommand}</pre>
              <button
                type="button"
                className="set-compression-copy-btn"
                onClick={() => navigator.clipboard.writeText(versionInfo.updateCommand).catch(() => {})}
                aria-label={t('common.copy')}
              >
                <Copy size={12} /> {t('common.copy')}
              </button>
            </div>
          </div>
        </div>
      )}

      {showInstallModal && (
        <div className="dash-modal-overlay" onClick={() => setShowInstallModal(false)}>
          <div
            className="dash-modal set-compression-modal"
            onClick={e => e.stopPropagation()}
            role="dialog"
            aria-modal="true"
            aria-labelledby="rtk-install-title"
            onKeyDown={e => { if (e.key === 'Escape') setShowInstallModal(false); }}
          >
            <div className="dash-modal-header">
              <h3 id="rtk-install-title" className="dash-modal-title">
                {t('config.rtk.installCta')}
              </h3>
              <button
                onClick={() => setShowInstallModal(false)}
                className="dash-modal-close"
                aria-label="Close"
              >
                <X size={16} />
              </button>
            </div>
            <div className="set-compression-modal-body">
              <p>{t('config.rtk.installHelp')}</p>
              <div className="set-compression-install-label">{t('config.rtk.installCommand')}</div>
              <pre className="set-compression-install-cmd">{RTK_INSTALL_CMD}</pre>
              <div className="set-compression-modal-links">
                <a href={RTK_GITHUB_URL} target="_blank" rel="noreferrer" className="set-compression-link">
                  {t('config.rtk.viewOnGithub')} <ExternalLink size={10} />
                </a>
                <a href={`${RTK_GITHUB_URL}#readme`} target="_blank" rel="noreferrer" className="set-compression-link">
                  {t('config.rtk.viewDocs')} <ExternalLink size={10} />
                </a>
              </div>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
