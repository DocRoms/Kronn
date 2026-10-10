import { useT } from '../lib/I18nContext';
import { skillBadgesOf, type SkillTraits } from '../lib/skillGroups';
import './SkillGroupBadges.css';

/** The badges a skill carries on the project card and on the Automation page alike. */
export function SkillGroupBadges({ traits }: { traits: SkillTraits }) {
  const { t } = useT();
  const badges = skillBadgesOf(traits);
  if (badges.length === 0) return null;
  return (
    <>
      {badges.map(badge => (
        <span key={badge} className="skill-group-badge" data-badge={badge} data-testid={`skill-group-badge-${badge}`}>
          {t(`skills.group.badge.${badge}`)}
        </span>
      ))}
    </>
  );
}
