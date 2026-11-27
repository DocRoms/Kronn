import { useCallback, useContext, useEffect, useMemo, useRef, type ReactNode } from 'react';
import { AppLink } from '../AppLink';
import { HeaderScrollLine } from '../HeaderScrollLine';
import { HeaderScrollLineContext } from '../headerScrollLineContext';
import { DetailHeader } from '../ui/DetailHeader';
import { SectionHeader } from '../ui/SectionHeader';
import { useT } from '../../lib/I18nContext';
import { settingsSectionPath, type SettingsSectionDef, type SettingsSubPage } from '../../lib/routes';
import { SettingsFold } from './SettingsFold';
import { revealLinkInLine, useSubPageBand } from './useSubPageBand';
import './SettingsNav.css';

interface SettingsListedSectionProps {
  /** Its anchor (`settings-…`), on the card. */
  id: string;
  title: ReactNode;
  description?: ReactNode;
  /** Folded until opened (the catalogue, the usage, the API call log). */
  folded?: boolean;
  testId?: string;
  children: ReactNode;
}

/**
 * KT-1923 — a section of Settings that used to sit inside another one (the
 * local models, the media sources, the catalogue and the usage were parts of
 * Agents) or that is folded: a card with its own anchor and a section header.
 */
export function SettingsListedSection({ id, title, description, folded = false, testId, children }: SettingsListedSectionProps) {
  if (folded) {
    return (
      <div id={id} className="set-card set-listed-section" data-testid={testId}>
        <div className="set-section">
          <SettingsFold id={id} title={title} description={description}>{children}</SettingsFold>
        </div>
      </div>
    );
  }
  const titleId = `${id}-title`;
  return (
    <section id={id} className="set-card set-listed-section" aria-labelledby={titleId} data-testid={testId}>
      <div className="set-section">
        <SectionHeader titleId={titleId} title={title} description={description} />
        {children}
      </div>
    </section>
  );
}

interface SettingsSubPageHeadProps {
  page: SettingsSubPage;
  sections: readonly SettingsSectionDef[];
  /** Whether the sub-page is the visible one: only then does its band follow the scroll. */
  active: boolean;
  /** A plain click on one of the section links. */
  onSection: (anchor: string) => void;
}

/** KT-1923 — the head of a sub-page of Settings: its name, its sentence, then
 *  a row of links to its sections.
 *  KT-2017 — that row is a band, a sibling of the head inside the sub-page's
 *  box (a sticky element only sticks inside its parent), so it stays under
 *  the app's top bar while the sub-page scrolls: one line that scrolls
 *  sideways when the links do not fit, the section being read marked
 *  (`aria-current="location"`). The name and sentence scroll away. */
export function SettingsSubPageHead({ page, sections, active, onSection }: SettingsSubPageHeadProps) {
  const { t } = useT();
  const head = useRef<HTMLDivElement>(null);
  const band = useRef<HTMLElement>(null);
  const sectionIds = useMemo(() => sections.map(section => section.id), [sections]);
  const { current, stuck, mark } = useSubPageBand({ band, head, sectionIds, active });
  const follow = useCallback((anchor: string) => {
    mark(anchor);
    onSection(anchor);
  }, [mark, onSection]);
  return (
    <>
      <div ref={head} className="set-subpage-head">
        <DetailHeader
          titleId={`settings-subpage-${page}-title`}
          title={t(`settings.pages.${page}.title`)}
          description={t(`settings.pages.${page}.hint`)}
        />
      </div>
      <nav
        ref={band}
        className="set-subpage-band"
        aria-label={t('settings.subPage.sections', t(`settings.pages.${page}.title`))}
        data-stuck={stuck || undefined}
        data-testid={`settings-subpage-band-${page}`}
      >
        <HeaderScrollLine className="set-subpage-band-line">
          <SubPageBandLinks sections={sections} current={current} active={active} onSection={follow} />
        </HeaderScrollLine>
      </nav>
    </>
  );
}

interface SubPageBandLinksProps {
  sections: readonly SettingsSectionDef[];
  current: string | null;
  active: boolean;
  onSection: (anchor: string) => void;
}

/** The links of the band, inside its line: the marked one is kept in view. */
function SubPageBandLinks({ sections, current, active, onSection }: SubPageBandLinksProps) {
  const { t } = useT();
  const line = useContext(HeaderScrollLineContext)?.scroller ?? null;
  useEffect(() => {
    if (!active || !current || !line) return;
    const link = [...line.querySelectorAll<HTMLElement>('a[data-section]')].find(a => a.dataset.section === current);
    if (link) revealLinkInLine(line, link);
  }, [active, current, line]);
  return (
    <ul className="set-subpage-links">
      {sections.map(section => (
        <li key={section.id}>
          <AppLink
            to={settingsSectionPath(section.id)}
            data-section={section.id}
            aria-current={section.id === current ? 'location' : undefined}
            onNavigate={() => onSection(section.id)}
          >
            {t(section.title)}
          </AppLink>
        </li>
      ))}
    </ul>
  );
}
