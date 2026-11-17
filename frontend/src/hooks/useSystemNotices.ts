// Everything the install has to tell its owner outside a page goes to the
// bell's Notifications tab (KT-1686): the key notices (KT-1007), an update
// (per version, put away in localStorage), and a failed reset.
import { useEffect } from 'react';
import { config as configApi, version as versionApi } from '../lib/api';
import { clearCredentialLabel } from '../lib/clearCredentials';
import { useT } from '../lib/I18nContext';
import { setNotices, type NoticeReport } from '../lib/notifications';
import { settingsSectionPath } from '../lib/routes';
import { safeSetItem } from '../lib/safeStorage';
import { useAuthFailureNotices } from './useAuthFailureNotices';

export const UPDATE_DISMISS_KEY = 'kronn:update-dismissed-version';
/** "Later" on the advice to protect the secrets: the time it comes back. */
export const PROTECT_SNOOZE_KEY = 'kronn:protect-secrets-snoozed-until';
export const PROTECT_SNOOZE_MS = 7 * 24 * 60 * 60 * 1000;

function protectSnoozed(now: number): boolean {
  try {
    const until = Number(localStorage.getItem(PROTECT_SNOOZE_KEY));
    return Number.isFinite(until) && until > now;
  } catch {
    return false;
  }
}

export function useSystemNotices(resetError: string | null, dismissResetError: () => void): void {
  const { t } = useT();
  useAuthFailureNotices();

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => configApi.getRecoveryStatus())
      .then(status => {
        if (!live) return;
        const recovery = settingsSectionPath('settings-recovery');
        const notices: NoticeReport[] = [];
        if (status.key_locked) {
          const clear = (status.clear_credentials ?? []).map(c => clearCredentialLabel(c, t)).join(', ');
          notices.push({ id: 'locked', kind: 'key_locked', args: clear ? [clear] : [], link: recovery, category: 'kronn' });
        } else {
          // Advice, not an alert: a recovery phrase already answers it, and
          // "Later" puts it away for a week. Settings > Recovery keeps saying it.
          const answered = status.configured && status.matches_key;
          if (status.config_holds_key && !answered && !protectSnoozed(Date.now())) {
            notices.push({
              id: 'beside-data',
              kind: 'key_beside_data',
              link: recovery,
              category: 'kronn',
              dismiss: () => safeSetItem(PROTECT_SNOOZE_KEY, String(Date.now() + PROTECT_SNOOZE_MS)),
            });
          }
          if (status.credentials_unavailable) {
            notices.push({ id: 'credentials', kind: 'credentials_unavailable', args: [status.credentials_unavailable], category: 'kronn' });
          }
        }
        if (status.config_set_aside) {
          notices.push({ id: 'set-aside', kind: 'config_set_aside', args: [status.config_set_aside], category: 'kronn', dismiss: () => {} });
        }
        setNotices('key', notices);
      })
      .catch(() => {});
    return () => { live = false; };
    // Read once: the wording of the names follows the language of that moment.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => versionApi.check())
      .then(info => {
        if (!live || info.up_to_date || !info.latest) return;
        const latest = info.latest;
        let dismissed: string | null = null;
        try { dismissed = localStorage.getItem(UPDATE_DISMISS_KEY); } catch { /* storage refused */ }
        // Put away for THIS version only: the next release shows again.
        if (dismissed === latest) return;
        setNotices('update', [{
          id: latest,
          kind: 'update_available',
          args: [info.current, latest],
          link: info.release_url ?? undefined,
          external: true,
          category: 'kronn',
          dismiss: () => safeSetItem(UPDATE_DISMISS_KEY, latest),
        }]);
      })
      .catch(error => console.warn('Update check failed:', error));
    return () => { live = false; };
  }, []);

  useEffect(() => {
    setNotices('reset', resetError
      ? [{ id: 'failed', kind: 'reset_error', args: [resetError], category: 'kronn', dismiss: dismissResetError }]
      : []);
  }, [resetError, dismissResetError]);
}
