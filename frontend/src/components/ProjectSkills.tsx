import { projects as projectsApi } from '../lib/api';
import type { Skill, SkillCategory } from '../types/generated';
import { useT } from '../lib/I18nContext';

const CATEGORY_COLORS: Record<SkillCategory, string> = {
  Language: 'var(--kr-info)',
  Business: 'var(--kr-success)',
  Domain: 'var(--kr-accent-ink)',
};

interface ProjectSkillsProps {
  projectId: string;
  currentSkillIds: string[];
  allSkills: Skill[];
  onUpdate: () => void;
}

export function ProjectSkills({ projectId, currentSkillIds, allSkills, onUpdate }: ProjectSkillsProps) {
  const { t } = useT();

  if (allSkills.length === 0) {
    return (
      <div style={{ padding: '4px 0', fontSize: 11, color: 'var(--kr-text-dim)' }}>
        {t('projects.noSkills')}
      </div>
    );
  }

  const handleToggle = async (skillId: string) => {
    const next = currentSkillIds.includes(skillId)
      ? currentSkillIds.filter(id => id !== skillId)
      : [...currentSkillIds, skillId];
    await projectsApi.setDefaultSkills(projectId, next);
    onUpdate();
  };

  return (
    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
      {allSkills.map(skill => {
        const active = currentSkillIds.includes(skill.id);
        const catColor = CATEGORY_COLORS[skill.category] ?? 'var(--kr-text-faint)';
        return (
          <button
            key={skill.id}
            onClick={() => handleToggle(skill.id)}
            style={{
              display: 'inline-flex', alignItems: 'center', gap: 5,
              padding: '4px 10px', borderRadius: 6, fontSize: 11, fontFamily: 'inherit',
              cursor: 'pointer',
              border: `1px solid ${active ? `${catColor}33` : 'var(--kr-border)'}`,
              background: active ? `${catColor}0F` : 'var(--kr-bg-subtle)',
              color: active ? catColor : 'var(--kr-text-faint)',
              transition: 'all 0.15s ease',
            }}
          >
            <span>{skill.icon}</span>
            <span style={{ fontWeight: active ? 600 : 400 }}>{skill.name}</span>
            <span style={{
              fontSize: 9, padding: '0 4px', borderRadius: 3, fontWeight: 600,
              background: `${catColor}15`, color: catColor, opacity: 0.7,
            }}>
              {skill.category}
            </span>
          </button>
        );
      })}
    </div>
  );
}
