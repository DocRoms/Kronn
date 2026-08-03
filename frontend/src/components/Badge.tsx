import type { ReactNode } from 'react';
import './Badge.css';

/** Semantic tones — map to a `--kr-*` colour driving both border + text
 *  (the badge is outline-only, so `currentColor` carries the whole look). */
export type BadgeTone = 'neutral' | 'faint' | 'accent' | 'success' | 'warning' | 'purple';

/**
 * One small outline tag, used across the app for statuses, levels, kinds, modes…
 * Consolidates the family of near-identical `.mentor-*-tag` styles into a single
 * component so they never drift. Deliberately NOT uppercase (the page had too
 * many caps); reserve uppercase for true section headers.
 */
export function Badge({
  tone = 'neutral',
  icon,
  children,
  className,
  title,
}: {
  tone?: BadgeTone;
  /** Optional leading glyph (inherits the tone colour via `currentColor`). */
  icon?: ReactNode;
  children: ReactNode;
  /** Extra class for positioning only (e.g. `margin-left:auto`) — not styling. */
  className?: string;
  title?: string;
}) {
  return (
    <span className={`kr-badge kr-badge-${tone}${className ? ` ${className}` : ''}`} title={title}>
      {icon}
      {children}
    </span>
  );
}
