import type { ReactNode } from 'react';
import { ArrowDown, ArrowRight, ArrowUp } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import type { StatDeltaDirection, StatDeltaTone, StatTileSize } from './dataKinds';
import './visuallyHidden.css';
import './StatTile.css';

export interface StatDelta {
  /** The change in words and figures: "+2 vs last week". */
  text: string;
  direction: StatDeltaDirection;
  /** Whether the change is good or bad; default neutral. */
  tone?: StatDeltaTone;
}

export interface StatTileProps {
  label: string;
  /** The figure; null when it is not known ("—"). */
  value: ReactNode | null;
  /** Why the value is not known, in a title and for assistive technology. */
  unknownReason?: string;
  unit?: ReactNode;
  /** Hidden from assistive technology. */
  icon?: ReactNode;
  /** One line under the value. */
  hint?: ReactNode;
  delta?: StatDelta;
  /** A placeholder of the value's height in place of the value. */
  loading?: boolean;
  /** Makes the tile a button (a filter, a drill-down). */
  onClick?: () => void;
  /** With onClick: the tile is a toggle, `aria-pressed`. */
  selected?: boolean;
  size?: StatTileSize;
  className?: string;
}

const DELTA_ICONS: Record<StatDeltaDirection, ReactNode> = {
  up: <ArrowUp />,
  down: <ArrowDown />,
  flat: <ArrowRight />,
};

/** KT-1886 — one figure with its label. A div, or a button when it can be
 *  clicked; only phrasing content inside, so both are valid. */
export function StatTile({
  label, value, unknownReason, unit, icon, hint, delta, loading = false, onClick, selected, size = 'md', className,
}: StatTileProps) {
  const { t } = useT();
  const unknown = value === null || value === undefined;
  const classes = ['ui-stat-tile', className].filter(Boolean).join(' ');

  const figure = loading ? (
    <>
      <span className="ui-stat-placeholder" aria-hidden="true" />
      <span className="ui-visually-hidden">{t('ui.loading.label')}</span>
    </>
  ) : unknown ? (
    <span className="ui-stat-value" data-unknown="true" title={unknownReason}>
      <span aria-hidden="true">—</span>
      <span className="ui-visually-hidden">{unknownReason ?? t('ui.stat.unknown')}</span>
    </span>
  ) : (
    <span className="ui-stat-value">
      {value}
      {unit !== undefined && <span className="ui-stat-unit">{unit}</span>}
    </span>
  );

  const content = (
    <>
      <span className="ui-stat-head">
        {icon && <span className="ui-stat-icon" aria-hidden="true">{icon}</span>}
        <span className="ui-stat-label">{label}</span>
      </span>
      {figure}
      {delta && !loading && (
        <span className="ui-stat-delta" data-direction={delta.direction} data-tone={delta.tone ?? 'neutral'}>
          <span className="ui-stat-delta-icon" aria-hidden="true">{DELTA_ICONS[delta.direction]}</span>
          <span className="ui-visually-hidden">{t(`ui.stat.${delta.direction}`)}</span>
          <span>{delta.text}</span>
        </span>
      )}
      {hint && <span className="ui-stat-hint">{hint}</span>}
    </>
  );

  const common = {
    className: classes,
    'data-size': size,
    'aria-busy': loading || undefined,
  };
  if (onClick) {
    return (
      <button type="button" {...common} data-selected={selected ? 'true' : undefined} aria-pressed={selected} onClick={onClick}>
        {content}
      </button>
    );
  }
  return <div {...common}>{content}</div>;
}

export interface StatGridProps {
  /** Names the group of tiles ("This week"); without it, a plain block. */
  label?: string;
  children: ReactNode;
  className?: string;
}

/** KT-1886 — tiles in columns that wrap by the grid's own width. */
export function StatGrid({ label, children, className }: StatGridProps) {
  return (
    <div
      className={className ? `ui-stat-grid ${className}` : 'ui-stat-grid'}
      role={label ? 'group' : undefined}
      aria-label={label}
    >
      {children}
    </div>
  );
}
