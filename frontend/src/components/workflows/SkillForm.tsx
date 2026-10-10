import { useState } from 'react';
import { Save, X } from 'lucide-react';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import { useT } from '../../lib/I18nContext';
import { userError } from '../../lib/userError';
import { skillBody } from '../../lib/automationSkills';
import type { CreateSkillRequest, Project, Skill, SkillCategory } from '../../types/generated';

interface SkillFormProps {
  editSkill?: Skill;
  projects: Project[];
  /** Project picked for a new skill, e.g. the Automation page's project filter. */
  initialProjectId?: string | null;
  onSave: (request: CreateSkillRequest) => Promise<void> | void;
  onCancel: () => void;
}

const CATEGORIES: SkillCategory[] = ['Language', 'Domain', 'Business'];

export function SkillForm({ editSkill, projects, initialProjectId, onSave, onCancel }: SkillFormProps) {
  const { t } = useT();
  const [name, setName] = useState(editSkill?.name ?? '');
  const [icon, setIcon] = useState(editSkill?.icon ?? '⭐');
  const [description, setDescription] = useState(editSkill?.description ?? '');
  const [category, setCategory] = useState<SkillCategory>(editSkill?.category ?? 'Domain');
  const [projectId, setProjectId] = useState(editSkill ? (editSkill.project_id ?? '') : (initialProjectId ?? ''));
  const [content, setContent] = useState(editSkill ? skillBody(editSkill.content) : '');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  // A ref-backed guard: two synchronous clicks must not send two creations.
  const save = useAsyncGuard(async () => {
    if (!name.trim() || !content.trim()) return;
    setSaving(true);
    setSaveError(null);
    try {
      await onSave({
        name: name.trim(),
        icon: icon.trim() || '⭐',
        description: description.trim(),
        category,
        content,
        // Sent as they are: the editor does not show them, an update must not drop them.
        license: editSkill?.license ?? null,
        allowed_tools: editSkill?.allowed_tools ?? null,
        project_id: projectId || null,
      });
    } catch (error) {
      setSaveError(userError(error));
    } finally {
      setSaving(false);
    }
  });

  return (
    <section className="qp-form qe-form" aria-label={editSkill ? t('skills.editTitle') : t('skills.newTitle')}>
      <div className="qp-form-header">
        <div>
          <h2>{editSkill ? t('skills.editTitle') : t('skills.newTitle')}</h2>
          <p>{t('skills.formHint')}</p>
        </div>
        <button className="wf-icon-btn" onClick={onCancel} aria-label={t('common.close')}>
          <X size={15} />
        </button>
      </div>

      <div className="qe-form-grid">
        <label>
          <span>{t('skills.icon')}</span>
          <input className="wf-input" value={icon} onChange={event => setIcon(event.target.value)} maxLength={8} />
        </label>
        <label className="qe-form-wide">
          <span>{t('skills.name')} *</span>
          <input className="wf-input" value={name} onChange={event => setName(event.target.value)} autoFocus />
        </label>
        <label className="qe-form-wide">
          <span>{t('skills.description')}</span>
          <input
            className="wf-input"
            value={description}
            onChange={event => setDescription(event.target.value)}
            placeholder={t('skills.descriptionPlaceholder')}
          />
        </label>
        <label>
          <span>{t('skills.category')}</span>
          <select className="wf-select" value={category} onChange={event => setCategory(event.target.value as SkillCategory)}>
            {CATEGORIES.map(value => (
              <option key={value} value={value}>{t(`skills.${value.toLowerCase()}`)}</option>
            ))}
          </select>
        </label>
        <label>
          <span>{t('skills.project')}</span>
          <select className="wf-select" value={projectId} onChange={event => setProjectId(event.target.value)}>
            <option value="">{t('skills.noProject')}</option>
            {projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}
          </select>
          <small>{projectId ? t('skills.projectHint') : t('skills.noProjectHint')}</small>
        </label>
        <label className="qe-form-wide">
          <span>{t('skills.content')} *</span>
          <textarea
            className="wf-textarea qe-args"
            value={content}
            onChange={event => setContent(event.target.value)}
            placeholder={t('skills.contentPlaceholder')}
          />
        </label>
      </div>

      {saveError && <div className="wf-apicall-error mb-2" role="alert">{saveError}</div>}
      <div className="qp-form-actions">
        <button className="wf-small-btn" type="button" onClick={onCancel}>{t('common.cancel')}</button>
        <button
          className="wf-create-btn"
          type="button"
          onClick={() => void save()}
          disabled={!name.trim() || !content.trim() || saving}
        >
          <Save size={13} /> {editSkill ? t('skills.saveChanges') : t('skills.add')}
        </button>
      </div>
    </section>
  );
}
