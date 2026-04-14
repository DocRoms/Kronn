import { projects as projectsApi } from '../lib/api';
import type { Skill, SkillCategory } from '../types/generated';
import { useT } from '../lib/I18nContext';

const CATEGORY_COLORS: Record<SkillCategory, string> = {
  Language: '#60a5fa',
  Business: '#34d399',
  Domain: '#c8ff00',
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
        const catColor = CATEGORY_COLORS[skill.category] ?? '#888';
        return (
          <button
            key={skill.id}
            onClick={() => handleToggle(skill.id)}
            style={{
              display: 'inline-flex', alignItems: 'center', gap: 5,
              padding: '4px 10px', borderRadius: 6, fontSize: 11, fontFamily: 'inherit',
              cursor: 'pointer',
              border: `1px solid ${active ? `${catColor}33` : 'rgba(255,255,255,0.08)'}`,
              background: active ? `${catColor}0F` : 'rgba(255,255,255,0.02)',
              color: active ? catColor : 'rgba(255,255,255,0.4)',
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
