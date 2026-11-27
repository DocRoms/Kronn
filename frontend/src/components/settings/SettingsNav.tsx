import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { Palette } from 'lucide-react';
import { AppLink } from '../AppLink';
import { EmptyState } from '../ui/EmptyState';
import { SearchField } from '../ui/SearchField';
import { useT } from '../../lib/I18nContext';
import {
  SETTINGS_SUB_PAGES,
  designSystemPath,
  settingsSectionPath,
  settingsSubPagePath,
  type SettingsSubPage,
} from '../../lib/routes';
import {
  searchSettings,
  visibleSettingsSections,
  type SettingsSearchAgent,
  type SettingsSearchHit,
} from '../../lib/settingsSearch';
import '../ui/visuallyHidden.css';
import './SettingsNav.css';

interface SettingsNavProps {
  /** The sub-page shown. */
  current: SettingsSubPage;
  /** The API call log section exists, so it is counted and searched. */
  hasConfiguredApi: boolean;
  /** The agents of this Kronn, found by their name. */
  agents?: readonly SettingsSearchAgent[];
  /** Sub-pages with something live (debug capture on): a pulsing dot. */
  livePages?: readonly SettingsSubPage[];
  /** A plain click on a sub-page. */
  onSubPage: (page: SettingsSubPage) => void;
  /** A plain click on a search hit that names an anchor (a hit on a sub-page's name is `onSubPage`). */
  onHit: (hit: Exclude<SettingsSearchHit, { kind: 'page' }>) => void;
  /** A plain click on the design system link. */
  onOpenDesignSystem?: () => void;
  /** Below the list: the version and the tour progress. */
  footer?: ReactNode;
}

/**
 * KT-1923 — the list of Settings, on the left as on the other pages (above
 * the content on a narrow window): the search, the five sub-pages with their
 * section count, then, apart, the design system for contributors. A plain
 * navigation, written so the shell of lot 2 can take its place without
 * changing an address.
 */
export function SettingsNav({
  current, hasConfiguredApi, agents, livePages = [], onSubPage, onHit, onOpenDesignSystem, footer,
}: SettingsNavProps) {
  const { t, tp } = useT();
  const [query, setQuery] = useState('');
  const hitsRef = useRef<HTMLUListElement | null>(null);
  const resultsId = useId();
  const searching = query.trim() !== '';
  const hits = searching ? searchSettings(query, t, { hasConfiguredApi, agents }) : [];

  // Down from the field goes to the first hit; up and down move between hits.
  const focusHit = (index: number) => {
    const links = [...(hitsRef.current?.querySelectorAll<HTMLAnchorElement>('a') ?? [])];
    links[Math.max(0, Math.min(index, links.length - 1))]?.focus();
  };
  const onFieldKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown' && hits.length > 0) {
      event.preventDefault();
      focusHit(0);
    }
  };
  const onHitKeyDown = (event: KeyboardEvent<HTMLAnchorElement>, index: number) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      focusHit(index + (event.key === 'ArrowDown' ? 1 : -1));
    }
  };

  return (
    <nav className="set-nav set-subnav" aria-label={t('settings.nav.label')} data-testid="settings-nav">
      <SearchField
        className="set-subnav-search"
        label={t('settings.search.label')}
        placeholder={t('settings.search.placeholder')}
        size="sm"
        value={query}
        onChange={setQuery}
        onKeyDown={onFieldKeyDown}
        aria-controls={searching ? resultsId : undefined}
        data-testid="settings-search"
      />
      <p className="ui-visually-hidden" role="status" data-testid="settings-search-count">
        {searching ? tp('settings.search.count', hits.length) : ''}
      </p>
      {searching && (hits.length > 0 ? (
        <ul
          id={resultsId}
          ref={hitsRef}
          className="set-subnav-hits"
          aria-label={t('settings.search.results')}
          data-testid="settings-search-hits"
        >
          {hits.map((hit, index) => {
            const pageName = t(`settings.pages.${hit.page}.title`);
            if (hit.kind === 'page') {
              return (
                <li key={`page:${hit.page}`}>
                  <AppLink
                    to={settingsSubPagePath(hit.page)}
                    className="set-subnav-hit"
                    data-page-hit={hit.page}
                    onNavigate={() => onSubPage(hit.page)}
                    onKeyDown={event => onHitKeyDown(event, index)}
                  >
                    <span className="set-subnav-hit-label">{hit.label}</span>
                    <span className="set-subnav-hit-where">
                      {tp('settings.nav.sectionCount', visibleSettingsSections(hit.page, hasConfiguredApi).length)}
                    </span>
                  </AppLink>
                </li>
              );
            }
            return (
              <li key={`${hit.section.id}:${hit.anchor}:${hit.label}`}>
                <AppLink
                  to={settingsSectionPath(hit.anchor)}
                  className="set-subnav-hit"
                  data-section={hit.section.id}
                  onNavigate={() => onHit(hit)}
                  onKeyDown={event => onHitKeyDown(event, index)}
                >
                  <span className="set-subnav-hit-label">{hit.label}</span>
                  <span className="set-subnav-hit-where">
                    {hit.kind === 'section' ? pageName : t('settings.search.where', pageName, t(hit.section.title))}
                  </span>
                </AppLink>
              </li>
            );
          })}
        </ul>
      ) : (
        <div id={resultsId} className="set-subnav-empty">
          <EmptyState
            kind="noResult"
            title={t('settings.search.none')}
            clearLabel={t('settings.search.clear')}
            onClear={() => setQuery('')}
          />
        </div>
      ))}

      <ul className="set-subnav-pages">
        {SETTINGS_SUB_PAGES.map(page => {
          const count = visibleSettingsSections(page, hasConfiguredApi).length;
          return (
            <li key={page}>
              <AppLink
                to={settingsSubPagePath(page)}
                className="set-subnav-page"
                aria-current={page === current ? 'page' : undefined}
                data-testid={`settings-subpage-${page}`}
                onNavigate={() => onSubPage(page)}
              >
                <span className="set-subnav-page-name">{t(`settings.pages.${page}.title`)}</span>
                {livePages.includes(page) && <span className="set-nav-live-dot" aria-hidden="true" />}
                <span className="set-subnav-page-count" aria-hidden="true">{count}</span>
                <span className="ui-visually-hidden">{tp('settings.nav.sectionCount', count)}</span>
              </AppLink>
            </li>
          );
        })}
      </ul>

      <div className="set-subnav-apart">
        <p className="set-subnav-apart-title">{t('settings.nav.contributors')}</p>
        <AppLink
          to={designSystemPath()}
          className="set-subnav-page"
          data-testid="settings-nav-design-system"
          onNavigate={onOpenDesignSystem}
        >
          <Palette size={14} aria-hidden="true" />
          <span className="set-subnav-page-name">{t('designSystem.link')}</span>
        </AppLink>
      </div>

      {footer}
    </nav>
  );
}
