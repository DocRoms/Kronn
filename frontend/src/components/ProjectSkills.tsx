import { useRef } from 'react';
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
  // Hooks must run unconditionally on every render — declare the
  // re-entry guard ref BEFORE any early-return branch.
  const togglingRef = useRef(false);

  if (allSkills.length === 0) {
    return (
      <div style={{ padding: '4px 0', fontSize: 11, color: 'var(--kr-text-dim)' }}>
        {t('projects.noSkills')}
      </div>
    );
  }

  // Synchronous re-entry guard. Two fast clicks on the same skill chip
  // would both read `currentSkillIds` from a stale closure (React hasn't
  // re-rendered between the two clicks), fire two
  // `setDefaultSkills` POSTs and lose the optimistic toggle. The ref
  // blocks the second invocation — the user has to wait one round-trip.
  const handleToggle = async (skillId: string) => {
    if (togglingRef.current) return;
    togglingRef.current = true;
    try {
      const next = currentSkillIds.includes(skillId)
        ? currentSkillIds.filter(id => id !== skillId)
        : [...currentSkillIds, skillId];
      await projectsApi.setDefaultSkills(projectId, next);
      onUpdate();
    } finally {
      togglingRef.current = false;
    }
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
            {skill.external && (
              <span
                style={{
                  fontSize: 9, padding: '0 4px', borderRadius: 3, fontWeight: 600,
                  background: 'rgba(var(--kr-info-rgb, 96 165 250), 0.15)',
                  color: 'var(--kr-info, rgb(96, 165, 250))',
                  opacity: 0.85,
                }}
                title={skill.source_url ? `Vendored from ${skill.source_url}` : 'Vendored from a third-party project'}
              >
                🔗 External
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}
