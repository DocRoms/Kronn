import { useState } from 'react';
import { ExternalLink, Settings, Star } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { projectsUsingSkill, skillOrigin } from '../lib/automationSkills';
import type { Project, Skill } from '../types/generated';
import { ConfirmDeleteButton } from './ConfirmDeleteButton';
import { CopyIdPill } from './CopyIdPill';
import { FileText } from './RepositoryResourceContent';
import './RepositoryResourceSheets.css';
import './SkillSheet.css';

type TextView = 'rendered' | 'source';

const ORIGIN_LABEL = {
  kronn: 'config.originKronn',
  personal: 'config.originPersonal',
  external: 'config.originExternal',
} as const;

interface FavoriteProps {
  pinned: boolean;
  onTogglePinned: () => void;
}

function SkillFavorite({ skill, pinned, onTogglePinned }: FavoriteProps & { skill: Skill }) {
  const { t } = useT();
  return (
    <button
      type="button"
      className="wf-icon-btn qp-card-tool-btn"
      data-active={pinned}
      onClick={onTogglePinned}
      title={pinned ? t('wf.unpin') : t('wf.pin')}
      aria-pressed={pinned}
      aria-label={`${pinned ? t('wf.unpin') : t('wf.pin')} · ${skill.name}`}
    >
      <Star size={14} fill={pinned ? 'currentColor' : 'none'} />
    </button>
  );
}

function SkillBadges({ skill }: { skill: Skill }) {
  const { t } = useT();
  return (
    <>
      <span className="skill-sheet-badge" data-testid="skill-category">{t(`skills.${skill.category.toLowerCase()}`)}</span>
      <span className="skill-sheet-badge" data-origin={skillOrigin(skill)}>{t(ORIGIN_LABEL[skillOrigin(skill)])}</span>
      {skill.token_estimate > 0 && (
        <span className="skill-sheet-badge" title={t('config.tokenCostHint')}>~{skill.token_estimate} tok</span>
      )}
    </>
  );
}

interface CardProps extends FavoriteProps {
  skill: Skill;
  projectCount: number;
  onOpen: () => void;
}

/** A skill in the main column while none is open: same card as the other
 *  types, opening its sheet. */
export function SkillCard({ skill, pinned, onTogglePinned, projectCount, onOpen }: CardProps) {
  const { t } = useT();
  return (
    <div className="qp-card skill-card" data-kind="skill">
      <div className="qp-card-header">
        <button
          type="button"
          className="qp-card-identity skill-card-open"
          onClick={onOpen}
          aria-label={t('automation.openResource', skill.name)}
        >
          <span className="qp-card-icon" aria-hidden="true">{skill.icon}</span>
          <span className="qp-card-name">{skill.name}</span>
        </button>
        <div className="qp-card-head-controls">
          <SkillFavorite skill={skill} pinned={pinned} onTogglePinned={onTogglePinned} />
          <CopyIdPill id={skill.id} title={t('automation.copyId', skill.name)} />
        </div>
      </div>
      {skill.description && <p className="qp-card-desc">{skill.description}</p>}
      <div className="qp-card-meta">
        <SkillBadges skill={skill} />
        <span>{t('automation.skill.projectCount', projectCount)}</span>
      </div>
    </div>
  );
}

interface SheetProps extends FavoriteProps {
  skill: Skill;
  projects: readonly Project[];
  /** Present for a skill the user wrote; a built-in one cannot be deleted. */
  onDelete?: () => Promise<void>;
  /** Opens the existing Settings screen, the only place a skill is edited. */
  onOpenSettings?: () => void;
  onError?: (message: string) => void;
}

/** The sheet of a skill, read in place: what it is, who uses it, and its
 *  SKILL.md rendered as safe Markdown (never raw HTML) or as source. There is
 *  no launch and no variable here — a skill is read, not run. */
export function SkillSheet({ skill, projects, pinned, onTogglePinned, onDelete, onOpenSettings, onError }: SheetProps) {
  const { t } = useT();
  const [view, setView] = useState<TextView>('rendered');
  const users = projectsUsingSkill(skill.id, projects);
  const sourceUrl = skill.external && skill.source_url && /^https?:\/\//i.test(skill.source_url)
    ? skill.source_url
    : null;

  return (
    <article className="skill-sheet" data-testid="skill-sheet" aria-label={skill.name}>
      <header className="qp-card collection-detail-header skill-sheet-header" data-detail="true">
        <div className="qp-card-header">
          <div className="qp-card-identity">
            <span className="qp-card-icon" aria-hidden="true">{skill.icon}</span>
            <h2 className="qp-card-name">{skill.name}</h2>
          </div>
          <div className="qp-card-head-controls">
            <SkillFavorite skill={skill} pinned={pinned} onTogglePinned={onTogglePinned} />
            <CopyIdPill id={skill.id} title={t('automation.copyId', skill.name)} />
          </div>
        </div>
      </header>
      <div className="skill-sheet-body">
        <div className="skill-sheet-meta">
          <SkillBadges skill={skill} />
          {sourceUrl && (
            <a className="skill-sheet-link" href={sourceUrl} target="_blank" rel="noopener noreferrer">
              <ExternalLink size={11} aria-hidden="true" /> {t('skills.source')}
            </a>
          )}
        </div>
        {skill.description && <p className="skill-sheet-description" data-testid="skill-description">{skill.description}</p>}

        <section className="skill-sheet-section" data-testid="skill-projects">
          <h3>{t('automation.skill.projects')}</h3>
          {users.length === 0 ? (
            <p className="skill-sheet-muted">{t('automation.skill.projectsNone')}</p>
          ) : (
            <ul className="skill-sheet-projects">
              {users.map(project => <li key={project.id}>{project.name}</li>)}
            </ul>
          )}
        </section>

        <section className="skill-sheet-section" data-testid="skill-content">
          <div className="skill-sheet-section-head">
            <h3><code>SKILL.md</code></h3>
            <div className="skill-sheet-view" role="group" aria-label={t('projects.repositoryResources.content.viewLabel')}>
              {(['rendered', 'source'] as const).map(option => (
                <button
                  key={option}
                  type="button"
                  className="skill-sheet-chip"
                  aria-pressed={view === option}
                  onClick={() => setView(option)}
                >
                  {t(`projects.repositoryResources.content.${option}`)}
                </button>
              ))}
            </div>
          </div>
          {skill.content.trim()
            ? <FileText text={skill.content} markdown={view === 'rendered'} />
            : <p className="skill-sheet-muted">{t('projects.repositoryResources.content.empty')}</p>}
        </section>

        <footer className="skill-sheet-actions">
          <p className="skill-sheet-muted">
            {t(skill.is_builtin ? 'automation.skill.builtinHint' : 'automation.skill.editHint')}
          </p>
          <div className="skill-sheet-buttons">
            {onOpenSettings && (
              <button type="button" className="skill-sheet-action" onClick={onOpenSettings}>
                <Settings size={12} aria-hidden="true" /> {t('automation.skill.openSettings')}
              </button>
            )}
            {onDelete && (
              <ConfirmDeleteButton
                className="wf-icon-btn qp-card-tool-btn"
                label={t('disc.delete')}
                confirmLabel={t('automation.deleteConfirmAction')}
                itemName={skill.name}
                testId={`skill-delete-${skill.id}`}
                impact={() => (users.length === 0
                  ? t('automation.deleteImpactNone')
                  : t('automation.skill.deleteImpact', users.length))}
                onError={onError}
                onConfirm={onDelete}
              />
            )}
          </div>
        </footer>
      </div>
    </article>
  );
}
