import { useState } from 'react';
import { ExternalLink, Pencil, Settings, Star } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { projectsUsingSkill, skillOrigin, type RepositorySkillOrigin } from '../lib/automationSkills';
import type { Project, Skill } from '../types/generated';
import { ConfirmDeleteButton } from './ConfirmDeleteButton';
import { CopyIdPill } from './CopyIdPill';
import { FileText } from './RepositoryResourceContent';
import './RepositoryResourceSheets.css';
import './SkillSheet.css';
import { SkillVariablesBadge } from './SkillVariablesBadge';

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

function SkillBadges({ skill, repository }: { skill: Skill; repository?: Pick<RepositorySkillOrigin, 'root'> }) {
  const { t } = useT();
  return (
    <>
      {repository ? (
        // A skill only the repository holds has no catalog category: its origin says where it lives.
        <span className="skill-sheet-badge" data-origin="repository" data-testid="skill-origin">
          {t('automation.skill.originRepository', repository.root)}
        </span>
      ) : (
        <>
          <span className="skill-sheet-badge" data-testid="skill-category">{t(`skills.${skill.category.toLowerCase()}`)}</span>
          <span className="skill-sheet-badge" data-origin={skillOrigin(skill)}>{t(ORIGIN_LABEL[skillOrigin(skill)])}</span>
        </>
      )}
      <SkillVariablesBadge skill={skill} />
      {skill.token_estimate > 0 && (
        <span className="skill-sheet-badge" title={t('config.tokenCostHint')}>~{skill.token_estimate} tok</span>
      )}
    </>
  );
}

interface CardProps extends FavoriteProps {
  skill: Skill;
  projectCount: number;
  /** Set for a skill only a repository holds. */
  repository?: Pick<RepositorySkillOrigin, 'root'>;
  onOpen: () => void;
}

/** A skill in the main column while none is open: same card as the other
 *  types, opening its sheet. */
export function SkillCard({ skill, pinned, onTogglePinned, projectCount, repository, onOpen }: CardProps) {
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
        <SkillBadges skill={skill} repository={repository} />
        <span className="skill-card-projects">{t('automation.skill.projectCount', projectCount)}</span>
      </div>
    </div>
  );
}

/** What the SKILL.md section shows while there is no text to show. */
export type SkillContentStatus = 'loading' | { error: string };

interface SheetProps extends FavoriteProps {
  skill: Skill;
  projects: readonly Project[];
  /** The projects using the skill, when default skills do not tell it all: a
   *  skill referenced or published from a repository (KT-921). */
  usedBy?: readonly Project[];
  /** Set for a skill only a repository holds: its SKILL.md is read from there. */
  repository?: Pick<RepositorySkillOrigin, 'root'>;
  /** Set while the SKILL.md is being read from the repository, or when it cannot be. */
  contentStatus?: SkillContentStatus;
  /** Present for a skill the user wrote; a built-in one cannot be deleted. */
  onDelete?: () => Promise<void>;
  /** Opens the existing Settings screen. */
  onOpenSettings?: () => void;
  /** Edits a custom skill in place. */
  onEdit?: () => void;
  onError?: (message: string) => void;
}

/** The sheet of a skill, read in place: what it is, who uses it, and its
 *  SKILL.md rendered as safe Markdown (never raw HTML) or as source. There is
 *  no launch and no variable here — a skill is read, not run. */
export function SkillSheet({ skill, projects, usedBy, repository, contentStatus, pinned, onTogglePinned, onDelete, onOpenSettings, onEdit, onError }: SheetProps) {
  const { t } = useT();
  const [view, setView] = useState<TextView>('rendered');
  const users = usedBy ?? projectsUsingSkill(skill.id, projects);
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
          <SkillBadges skill={skill} repository={repository} />
          {skill.project_id && (
            <span className="skill-sheet-badge" data-origin="project" data-testid="skill-project">
              {t('skills.projectBadge', projects.find(project => project.id === skill.project_id)?.name ?? skill.project_id)}
            </span>
          )}
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
          {contentStatus === 'loading' ? (
            <p className="skill-sheet-muted" role="status">{t('automation.skill.contentLoading')}</p>
          ) : contentStatus ? (
            <p className="skill-sheet-muted" role="alert">{t('automation.skill.contentError', contentStatus.error)}</p>
          ) : skill.content.trim()
            ? <FileText text={skill.content} markdown={view === 'rendered'} />
            : <p className="skill-sheet-muted">{t('projects.repositoryResources.content.empty')}</p>}
        </section>

        <footer className="skill-sheet-actions">
          <p className="skill-sheet-muted">
            {t(repository ? 'automation.skill.repositoryHint' : skill.is_builtin ? 'automation.skill.builtinHint' : 'automation.skill.editHint')}
          </p>
          <div className="skill-sheet-buttons">
            {onEdit && (
              <button type="button" className="skill-sheet-action" onClick={onEdit}>
                <Pencil size={12} aria-hidden="true" /> {t('skills.edit')}
              </button>
            )}
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
