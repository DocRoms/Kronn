import { useEffect } from 'react';
import { Navigate, useLocation, useParams } from 'react-router';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { useT } from '../lib/I18nContext';
import {
  SETTINGS_APPEARANCE_SECTION_ID,
  designSystemAnchorTab,
  designSystemPath,
  designSystemTabFromSegment,
} from '../lib/routes';
import { revealWhenSettled } from '../lib/revealWhenSettled';
import { DesignSystemPage } from '../pages/DesignSystemPage';

/**
 * KT-1987 — `/config/design-system[/components][#<entry>]`. The tab is the
 * path, the entry the hash: each arrival brings the entry into view once the
 * gallery has settled. An entry asked on the other tab's address, or an
 * unknown tab, is sent to the address that shows it.
 */
export function DesignSystemRoute() {
  const { t } = useT();
  const nav = useKronnNavigate();
  const { designSystemTab } = useParams();
  const { hash, key: arrival } = useLocation();
  let anchor: string | null = null;
  try { anchor = decodeURIComponent(hash.slice(1)) || null; } catch { anchor = null; }
  const tab = designSystemTabFromSegment(designSystemTab);
  const anchorTab = anchor ? designSystemAnchorTab(anchor) : null;
  const misplaced = tab === null || (anchorTab !== null && anchorTab !== tab);

  useEffect(() => {
    if (misplaced || !anchor) return undefined;
    return revealWhenSettled(anchor);
  }, [misplaced, anchor, arrival]);

  if (misplaced) {
    return <Navigate to={designSystemPath({ tab: anchorTab ?? 'tokens', anchor: anchorTab ? anchor : null })} replace />;
  }
  return (
    <DesignSystemPage
      t={t}
      tab={tab}
      onTabChange={next => nav.toDesignSystem({ tab: next })}
      onBack={() => nav.toSettingsSection(SETTINGS_APPEARANCE_SECTION_ID)}
    />
  );
}
