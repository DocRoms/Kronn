import { useEffect, useState } from 'react';
import { version as versionApi } from '../lib/api';
import type { VersionCheck } from '../types/generated';
import { useT } from '../lib/I18nContext';

/**
 * Self-hosted Kronn auto-update banner.
 *
 * Polls `GET /api/version/check` once on mount, renders a subtle pill
 * in the top-right when a newer version is available on GitHub. Click
 * opens the release page in a new tab. Dismissable per-version via
 * localStorage so users who've seen "v0.7.2 available" once aren't
 * nagged on every page reload.
 *
 * # Why backend-side fetch
 *
 * Kronn ships in many install shapes (deb / Tauri / Docker / cargo).
 * No single auto-installer would work everywhere, so the banner just
 * links to the release page and lets the user pick `make bump` /
 * `apt upgrade` / Tauri auto-updater themselves. The backend takes
 * the GitHub fetch off the user's IP rate budget and caches the
 * result for 6 h.
 */
const DISMISS_KEY = 'kronn:update-dismissed-version';

export function UpdateBanner() {
  const { t } = useT();
  const [info, setInfo] = useState<VersionCheck | null>(null);
  const [dismissed, setDismissed] = useState<string | null>(() => {
    try { return localStorage.getItem(DISMISS_KEY); } catch { return null; }
  });

  useEffect(() => {
    let cancelled = false;
    versionApi.check()
      .then(v => { if (!cancelled) setInfo(v); })
      .catch(e => console.warn('Update check failed:', e));
    return () => { cancelled = true; };
  }, []);

  if (!info || info.up_to_date || !info.latest) return null;
  // User dismissed THIS specific latest version → keep hidden until a
  // newer one ships. Dismissing 0.7.2 doesn't suppress 0.7.3.
  if (dismissed === info.latest) return null;

  const handleDismiss = () => {
    // `info.latest` is guaranteed non-null at this point: the early return
    // above (`!info.latest`) bails out before we render the dismiss button.
    // Hoist it to a local so TypeScript follows the narrowing.
    const latest = info.latest;
    if (!latest) return;
    try { localStorage.setItem(DISMISS_KEY, latest); } catch { /* incognito */ }
    setDismissed(latest);
  };

  return (
    <div
      className="kronn-update-banner"
      role="status"
      aria-live="polite"
    >
      <span className="kronn-update-banner-text">
        {t('app.updateAvailable', info.current, info.latest)}
      </span>
      {info.release_url && (
        <a
          className="kronn-update-banner-link"
          href={info.release_url}
          target="_blank"
          rel="noreferrer"
        >
          {t('app.updateOpenRelease')}
        </a>
      )}
      <button
        className="kronn-update-banner-dismiss"
        onClick={handleDismiss}
        aria-label={t('app.updateDismiss')}
        title={t('app.updateDismiss')}
      >
        ×
      </button>
    </div>
  );
}
