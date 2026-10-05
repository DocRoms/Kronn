import { Braces } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { isVariabilizedSkill } from '../lib/automationSkills';
import type { Skill } from '../types/generated';
import './SkillVariablesBadge.css';

/** The same badge in every skill list; its hover names the `$` placeholders. */
export function SkillVariablesBadge({ skill }: { skill: Pick<Skill, 'arguments'> }) {
  const { t } = useT();
  if (!isVariabilizedSkill(skill)) return null;
  const placeholders = (skill.arguments ?? []).map(name => `$${name}`).join(' ');
  return (
    <span
      className="skill-variables-badge"
      data-testid="skill-variables-badge"
      title={t('skills.variabilizedHint', placeholders)}
    >
      <Braces size={9} aria-hidden="true" />
      {t('skills.variabilized')}
    </span>
  );
}
