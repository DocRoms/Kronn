import type { ReactNode } from 'react';
import type { StatusTone } from './statusTones';
import './StatusBadge.css';

export interface StatusBadgeProps {
  tone: StatusTone;
  /** The status in words, always shown: the colour never carries it alone. */
  children: ReactNode;
  /** An icon instead of the dot, hidden from assistive technology. */
  icon?: ReactNode;
  className?: string;
}

/** KT-1880 — the state of a run, a task or a check, as a small label. */
export function StatusBadge({ tone, children, icon, className }: StatusBadgeProps) {
  return (
    <span className={className ? `ui-status-badge ${className}` : 'ui-status-badge'} data-tone={tone}>
      {icon
        ? <span className="ui-status-badge-icon" aria-hidden="true">{icon}</span>
        : <span className="ui-status-badge-dot" aria-hidden="true" />}
      <span>{children}</span>
    </span>
  );
}
