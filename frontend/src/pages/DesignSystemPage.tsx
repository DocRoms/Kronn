import { useEffect, useRef, useState } from 'react';
import { ChevronLeft } from 'lucide-react';
import { AppLink } from '../components/AppLink';
import { ListHeader } from '../components/ui/ListHeader';
import { Tabs } from '../components/ui/Tabs';
import { DesignSystemComponents } from '../components/settings/DesignSystemComponents';
import { DesignSystemTokens } from '../components/settings/DesignSystemTokens';
import { GALLERY_COMPONENTS, TOKENS_ANCHOR, componentAnchor } from '../components/settings/designSystemGallery';
import { SETTINGS_APPEARANCE_SECTION_ID, designSystemPath, settingsSectionPath, type DesignSystemTab } from '../lib/routes';
import '../components/settings/DesignSystemSection.css';
import './SettingsPage.css';

interface Props {
  t: (key: string, ...args: (string | number)[]) => string;
  /** The tab shown, when the address holds it. Without it the page holds its own, from Tokens. */
  tab?: DesignSystemTab;
  /** The reader picked another tab (from the tabs or from the index). */
  onTabChange?: (tab: DesignSystemTab) => void;
  /** A plain click on the way back; without it the link loads Settings. */
  onBack?: () => void;
}

interface IndexEntry {
  tab: DesignSystemTab;
  anchor: string;
  label: string;
}

/** Scrolls to a heading of the page and gives it focus, without touching the address. */
function goTo(target: HTMLElement) {
  target.scrollIntoView?.({ behavior: 'smooth', block: 'start' });
  target.focus({ preventScroll: true });
}

/** KT-1987 — Settings > Design system, a sub-page of its own (it used to
 *  close the Settings page): a ListHeader with the way back to Settings,
 *  the index of every entry, then the tokens (KT-1878) and the components of
 *  components/ui/ (KT-1880) in two tabs. Linked from the appearance settings. */
export function DesignSystemPage({ t, tab, onTabChange, onBack }: Props) {
  const [own, setOwn] = useState<DesignSystemTab>('tokens');
  const selected = tab ?? own;
  const select = (next: DesignSystemTab) => {
    if (tab === undefined) setOwn(next);
    if (next !== selected) onTabChange?.(next);
  };

  // An index entry of the other tab: the tab first, then its heading, once drawn.
  const pending = useRef<string | null>(null);
  const [request, setRequest] = useState(0);
  useEffect(() => {
    const anchor = pending.current;
    const target = anchor ? document.getElementById(anchor) : null;
    if (!target) return;
    pending.current = null;
    goTo(target);
  }, [request, selected]);

  const index: IndexEntry[] = [
    { tab: 'tokens', anchor: TOKENS_ANCHOR, label: t('designSystem.tokens') },
    ...GALLERY_COMPONENTS.map(c => ({ tab: 'components' as const, anchor: componentAnchor(c.id), label: c.name })),
  ];
  const follow = (entry: IndexEntry) => {
    pending.current = entry.anchor;
    setRequest(n => n + 1);
    select(entry.tab);
  };

  return (
    <div className="ds-page" data-testid="design-system-page">
      <div className="set-card ds-card">
        <div className="set-section">
          <ListHeader
            className="ds-header"
            title={t('designSystem.title')}
            description={t('designSystem.hint')}
            leading={(
              <AppLink
                to={settingsSectionPath(SETTINGS_APPEARANCE_SECTION_ID)}
                className="ds-back"
                aria-label={t('designSystem.back')}
                title={t('designSystem.back')}
                data-testid="design-system-back"
                onNavigate={onBack}
              >
                <ChevronLeft size={16} aria-hidden="true" />
              </AppLink>
            )}
          />
          <nav className="ds-index" aria-label={t('designSystem.index')}>
            <ul>
              {index.map(entry => (
                <li key={entry.anchor}>
                  <AppLink to={designSystemPath(entry)} onNavigate={() => follow(entry)}>{entry.label}</AppLink>
                </li>
              ))}
            </ul>
          </nav>
          <Tabs
            className="ds-tabs"
            label={t('designSystem.parts')}
            value={selected}
            onChange={id => select(id as DesignSystemTab)}
            tabs={[
              { id: 'tokens', label: t('designSystem.tokens'), panel: <DesignSystemTokens t={t} /> },
              {
                id: 'components',
                label: t('designSystem.components'),
                panel: (
                  <section className="ds-part" aria-labelledby="ds-components">
                    <h2 id="ds-components" className="ds-part-title ds-sr-only" tabIndex={-1}>{t('designSystem.components')}</h2>
                    <p className="set-hint">{t('designSystem.componentsHint')}</p>
                    <DesignSystemComponents t={t} />
                  </section>
                ),
              },
            ]}
          />
        </div>
      </div>
    </div>
  );
}
