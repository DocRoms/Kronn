import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, Check, ChevronRight, Search } from 'lucide-react';
import { projects as projectsApi, skills as skillsApi } from '../lib/api';
import { missingSkillLabel, skillPickerModel } from '../lib/discussionSkills';
import type { Project, ProjectUsedSkill, Skill } from '../types/generated';
import type { AutomationSkillEntry } from '../lib/automationSkills';
import './DiscussionSkillPicker.css';

interface Props {
  /** The discussion's project, `null` for a general discussion. */
  projectId: string | null;
  projects: readonly Project[];
  /** The catalog the caller already holds, shown while the picker reads its own. */
  catalog?: readonly Skill[];
  selectedIds: readonly string[];
  onToggle: (skillId: string) => void;
  t: (key: string, ...args: (string | number)[]) => string;
  /** The chip style of the surface the picker sits in. */
  chipClassName?: string;
}

/**
 * The skills a discussion can carry (KT-923), for the "new discussion" form and
 * the discussion settings: the project's own skills first — the native ones of
 * its repository included — then the ticked ones, then the catalog by category
 * behind "Voir les skills disponibles". The lists are read when the picker
 * opens (it is mounted only while open), so a skill just used, published or
 * attached elsewhere is here the next time, and one gone from the repository is
 * gone from the list.
 */
export function DiscussionSkillPicker({
  projectId,
  projects,
  catalog: initialCatalog,
  selectedIds,
  onToggle,
  t,
  chipClassName = 'disc-chip',
}: Props) {
  const [catalog, setCatalog] = useState<readonly Skill[]>(initialCatalog ?? []);
  const [usedSkills, setUsedSkills] = useState<readonly ProjectUsedSkill[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [query, setQuery] = useState('');
  const [availableOpen, setAvailableOpen] = useState(false);
  const latest = useRef(0);

  // Read on open, and again when the project changes: what the project uses
  // moves with it, and the section below already follows the new project from
  // the lists in hand.
  useEffect(() => {
    const request = ++latest.current;
    const read = <T,>(call: () => Promise<T>): Promise<T | null> =>
      Promise.resolve().then(call).catch(() => null);
    void Promise.all([read(() => skillsApi.list()), read(() => projectsApi.usedSkills())]).then(([skills, used]) => {
      if (request !== latest.current) return;
      // A read that failed leaves what was there: an empty answer is not a
      // reason to take the lists away.
      if (Array.isArray(skills)) setCatalog(skills);
      if (Array.isArray(used)) setUsedSkills(used);
      setLoaded(true);
    });
    return () => { latest.current += 1; };
  }, [projectId]);

  const project = useMemo(
    () => (projectId ? projects.find(candidate => candidate.id === projectId) ?? null : null),
    [projectId, projects],
  );
  const model = useMemo(
    () => skillPickerModel({ catalog, project, usedSkills, selectedIds, query, loaded }),
    [catalog, project, usedSkills, selectedIds, query, loaded],
  );

  const searching = query.trim().length > 0;
  const ticked = new Set(selectedIds);
  const availableShown = !model.foldAvailable || searching || availableOpen;
  const nothingShown = model.used.length === 0
    && model.ticked.length === 0
    && model.missing.length === 0
    && model.availableCount === 0;

  const chip = (entry: AutomationSkillEntry) => {
    const active = ticked.has(entry.id);
    const origin = entry.repository ? t('automation.skill.originRepository', entry.repository.root) : null;
    return (
      <button
        key={entry.id}
        type="button"
        className={chipClassName}
        data-active={active}
        data-color="accent"
        data-origin={entry.repository ? 'repository' : undefined}
        aria-pressed={active}
        onClick={() => onToggle(entry.id)}
        title={[entry.skill.description || entry.skill.name, origin].filter(Boolean).join(' — ')}
      >
        {active && <Check size={9} aria-hidden="true" />}
        {entry.skill.icon} {entry.skill.name}
        {origin && <small className="skill-picker-origin">{origin}</small>}
      </button>
    );
  };

  return (
    <div className="skill-picker" data-testid="skill-picker">
      <label className="skill-picker-search">
        <Search size={11} aria-hidden="true" />
        <input
          type="search"
          value={query}
          onChange={event => setQuery(event.target.value)}
          placeholder={t('disc.skillSearch')}
          aria-label={t('disc.skillSearch')}
        />
      </label>

      {model.used.length > 0 && (
        <section className="skill-picker-section" data-section="used" aria-label={t('disc.skillsUsedByProject')}>
          <h4 className="skill-picker-heading">
            {t('disc.skillsUsedByProject')} <span>{model.used.length}</span>
          </h4>
          <div className="skill-picker-chips">{model.used.map(chip)}</div>
        </section>
      )}

      {(model.ticked.length > 0 || model.missing.length > 0) && (
        <section className="skill-picker-section" data-section="ticked" aria-label={t('disc.skillsTicked')}>
          <h4 className="skill-picker-heading">
            {t('disc.skillsTicked')} <span>{model.ticked.length + model.missing.length}</span>
          </h4>
          <div className="skill-picker-chips">
            {model.ticked.map(chip)}
            {model.missing.map(id => (
              <button
                key={id}
                type="button"
                className={chipClassName}
                data-active="true"
                data-color="accent"
                data-missing="true"
                aria-pressed="true"
                onClick={() => onToggle(id)}
                title={t('disc.skillUnavailable')}
              >
                <AlertTriangle size={9} aria-hidden="true" /> {missingSkillLabel(id)}
              </button>
            ))}
          </div>
        </section>
      )}

      {model.availableCount > 0 && (
        <section className="skill-picker-section" data-section="available">
          {model.foldAvailable && (
            <button
              type="button"
              className="skill-picker-fold"
              aria-expanded={availableShown}
              onClick={() => setAvailableOpen(open => !open)}
            >
              <ChevronRight size={10} className="disc-chevron" data-expanded={availableShown} aria-hidden="true" />
              {t('automation.skill.availableToggle', model.availableCount)}
            </button>
          )}
          {availableShown && model.available.map(group => (
            <div key={group.category} className="skill-picker-group" data-category={group.category}>
              <h5 className="skill-picker-category">
                {t(`skills.${group.category.toLowerCase()}`)} <span>{group.entries.length}</span>
              </h5>
              <div className="skill-picker-chips">{group.entries.map(chip)}</div>
            </div>
          ))}
        </section>
      )}

      {searching && nothingShown && (
        <p className="skill-picker-empty" role="status">{t('disc.skillNoMatch', query.trim())}</p>
      )}
    </div>
  );
}
