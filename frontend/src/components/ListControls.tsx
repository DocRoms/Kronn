import { ArrowDownAZ, ArrowDownZA, ArrowUpDown, Filter } from 'lucide-react';
import './ListControls.css';

export interface ListControlOption<T extends string> {
  value: T;
  label: string;
  disabled?: boolean;
}

interface ListControlsProps<TFilter extends string, TSort extends string> {
  filterLabel?: string;
  filterAriaLabel?: string;
  filterValue?: TFilter;
  filterOptions?: Array<ListControlOption<TFilter>>;
  onFilterChange?: (value: TFilter) => void;
  sortLabel: string;
  sortAriaLabel?: string;
  sortValue: TSort;
  sortOptions: Array<ListControlOption<TSort>>;
  onSortChange: (value: TSort) => void;
  reversed: boolean;
  onToggleDirection: () => void;
  directionLabel: string;
  className?: string;
}

/**
 * Shared compact list toolbar used by Automation and Plugins.
 * Separating the sort criterion from its direction avoids duplicating
 * "name A→Z / name Z→A" options and keeps filtering visually lightweight.
 */
export function ListControls<TFilter extends string, TSort extends string>({
  filterLabel,
  filterAriaLabel,
  filterValue,
  filterOptions,
  onFilterChange,
  sortLabel,
  sortAriaLabel,
  sortValue,
  sortOptions,
  onSortChange,
  reversed,
  onToggleDirection,
  directionLabel,
  className,
}: ListControlsProps<TFilter, TSort>) {
  const showFilter = filterLabel != null
    && filterValue != null
    && filterOptions != null
    && onFilterChange != null;

  return (
    <div className={`list-controls${className ? ` ${className}` : ''}`}>
      {showFilter && (
        <label className="list-control">
          <Filter size={12} />
          <span>{filterLabel}</span>
          <select
            value={filterValue}
            onChange={event => onFilterChange(event.target.value as TFilter)}
            aria-label={filterAriaLabel ?? filterLabel}
          >
            {filterOptions.map(option => (
              <option key={option.value} value={option.value} disabled={option.disabled}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
      )}

      <label className="list-control">
        <ArrowUpDown size={12} />
        <span>{sortLabel}</span>
        <select
          value={sortValue}
          onChange={event => onSortChange(event.target.value as TSort)}
          aria-label={sortAriaLabel ?? sortLabel}
        >
          {sortOptions.map(option => (
            <option key={option.value} value={option.value} disabled={option.disabled}>
              {option.label}
            </option>
          ))}
        </select>
      </label>

      <button
        type="button"
        className="list-direction-btn"
        data-reversed={reversed}
        onClick={onToggleDirection}
        title={directionLabel}
        aria-label={directionLabel}
        aria-pressed={reversed}
      >
        {reversed ? <ArrowDownZA size={15} /> : <ArrowDownAZ size={15} />}
      </button>
    </div>
  );
}
