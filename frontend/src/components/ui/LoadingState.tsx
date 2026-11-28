import { useEffect, useLayoutEffect, useState } from 'react';
import { useT } from '../../lib/I18nContext';
import { KronnMark } from '../KronnMark';
import { Button } from './Button';
import {
  bootMessage,
  bootScreenActive,
  holdBootScreen,
  onBootScreenFinished,
  setBootPhase,
  type BootPhase,
} from '../../lib/bootScreen';
import type { LoadingSize, SkeletonKind } from './feedbackKinds';
import './visuallyHidden.css';
import './LoadingState.css';

/** The edge of the Kronn mark at md and lg, in px (KronnMark draws an svg). */
const MARK_SIZE: Record<Exclude<LoadingSize, 'sm'>, number> = { md: 48, lg: 100 };

export interface LoadingStateProps {
  size?: LoadingSize;
  phase?: BootPhase;
  /** The sentence beside or under the mark; defaults to the phase's. */
  message?: string;
  onRetry?: () => void;
  /** Full window (start-up) rather than the page area; implies lg. */
  fullscreen?: boolean;
  /** md and lg wait behind the cold-start screen (default). False for a
   *  loader shown as content, such as a gallery demo: it draws at once and
   *  never holds that screen, which would otherwise stay up while it is
   *  mounted. */
  holdsBootScreen?: boolean;
  className?: string;
}

/**
 * KT-1883 — Kronn's one loading visual, in three sizes. md and lg are the mark
 * with a plain sentence under it; during a cold start they draw nothing and
 * keep the screen index.html painted, only changing its sentence, so loaders
 * never hand over to one another on screen. sm is inline and never waits for
 * that screen, nor does a loader shown as content (`holdsBootScreen={false}`).
 */
export function LoadingState({
  size = 'md', phase = 'opening', message, onRetry, fullscreen = false, holdsBootScreen = true, className,
}: LoadingStateProps) {
  const shown: LoadingSize = fullscreen ? 'lg' : size;
  // Whether the cold-start screen is still up, followed until it goes; the
  // hold itself is derived from the current props, so a loader that turns sm
  // or stops holding releases the screen and draws itself at once.
  const [bootUp, setBootUp] = useState(bootScreenActive);
  const deferToBoot = bootUp && shown !== 'sm' && holdsBootScreen;

  useLayoutEffect(() => {
    if (!bootUp) return;
    return onBootScreenFinished(() => setBootUp(false));
  }, [bootUp]);

  useLayoutEffect(() => {
    if (!deferToBoot) return;
    return holdBootScreen();
  }, [deferToBoot]);

  useEffect(() => {
    if (deferToBoot) setBootPhase(phase, onRetry);
  }, [deferToBoot, phase, onRetry]);

  if (deferToBoot) return null;
  const classes = ['ui-loading-state', className].filter(Boolean).join(' ');
  const text = message ?? bootMessage(phase);
  if (shown === 'sm') {
    return (
      <span className={classes} data-size="sm" role="status" aria-live="polite">
        <span className="ui-loading-ring" aria-hidden="true" />
        <span className="ui-loading-text">{text}</span>
      </span>
    );
  }
  return (
    <div
      className={classes}
      data-size={shown}
      data-fullscreen={fullscreen || undefined}
      role="status"
      aria-live="polite"
    >
      <KronnMark size={MARK_SIZE[shown]} className="ui-loading-mark" />
      <span className="ui-loading-text">{text}</span>
      {onRetry && <Button onClick={onRetry}>{bootMessage('retry')}</Button>}
    </div>
  );
}

/** Keeps the cold-start screen up while `active`, without drawing anything. */
export function BootHold({ active, phase = 'opening' }: { active: boolean; phase?: BootPhase }) {
  useLayoutEffect(() => {
    if (!active || !bootScreenActive()) return;
    return holdBootScreen();
  }, [active]);
  useEffect(() => {
    if (active) setBootPhase(phase);
  }, [active, phase]);
  return null;
}

export interface SkeletonProps {
  kind: SkeletonKind;
  /** Rows of a list skeleton (default 4). */
  rows?: number;
  /** What is loading, for assistive technology; defaults to "Loading…". */
  label?: string;
  className?: string;
}

/** Line widths, in percent: varied so the shape reads as text, not as bars. */
const LIST_WIDTHS: readonly [number, number][] = [[60, 35], [45, 25], [70, 40], [55, 30]];
const DETAIL_WIDTHS: readonly number[] = [100, 92, 96, 70];

/** KT-1883 — the shape of a list or a detail pane while its content loads, so
 *  the layout does not jump when it arrives. One status for assistive
 *  technology, its hidden label; the shapes are hidden from it. The status is
 *  not marked busy: aria-busy would hold its announcement until it stops
 *  being busy, and the skeleton unmounts before that. */
export function Skeleton({ kind, rows = 4, label, className }: SkeletonProps) {
  const { t } = useT();
  return (
    <div
      className={className ? `ui-skeleton ${className}` : 'ui-skeleton'}
      data-kind={kind}
      role="status"
    >
      <span className="ui-visually-hidden">{label ?? t('ui.loading.label')}</span>
      {kind === 'list' ? (
        <ul className="ui-skeleton-list" aria-hidden="true">
          {Array.from({ length: rows }, (_, i) => {
            const [first, second] = LIST_WIDTHS[i % LIST_WIDTHS.length];
            return (
              <li key={i} className="ui-skeleton-row">
                <span className="ui-skeleton-line" data-w={first} />
                <span className="ui-skeleton-line" data-w={second} data-line="sub" />
              </li>
            );
          })}
        </ul>
      ) : (
        <div className="ui-skeleton-detail" aria-hidden="true">
          <span className="ui-skeleton-line" data-w={40} data-line="title" />
          <span className="ui-skeleton-line" data-w={25} data-line="sub" />
          <div className="ui-skeleton-paragraph">
            {DETAIL_WIDTHS.map(w => <span key={w} className="ui-skeleton-line" data-w={w} />)}
          </div>
          <span className="ui-skeleton-block" />
        </div>
      )}
    </div>
  );
}
