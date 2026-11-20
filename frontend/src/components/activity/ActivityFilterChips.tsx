import { useT } from '../../lib/I18nContext';
import { ACTIVITY_FILTERS, type ActivityFilter } from '../../lib/activityFilters';

export interface ActivityFilterChipsProps {
  value: ActivityFilter;
  counts: Readonly<Record<ActivityFilter, number>>;
  onChange: (filter: ActivityFilter) => void;
}

/** All / Discussions / Automations / Kronn, each with how many it holds (KT-1684). */
export function ActivityFilterChips({ value, counts, onChange }: ActivityFilterChipsProps) {
  const { t } = useT();
  return (
    <div className="activity-chips" role="group" aria-label={t('activity.filters.label')}>
      {ACTIVITY_FILTERS.map(filter => (
        <button
          key={filter}
          type="button"
          className="activity-chip"
          data-testid={`activity-chip-${filter}`}
          aria-pressed={value === filter}
          onClick={() => onChange(filter)}
        >
          {t(`activity.filter.${filter}`)}
          <span className="activity-chip-count" data-testid={`activity-chip-count-${filter}`}>{counts[filter]}</span>
        </button>
      ))}
    </div>
  );
}
