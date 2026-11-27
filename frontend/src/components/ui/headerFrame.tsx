// KT-1884 — the one frame the six header kinds share: identity on the left
// (leading, title, meta, status, description), actions on the right. A kind
// fixes the heading level and the sizes; the caller fills the slots.
//
// The frame is plain content: never sticky, never fixed, never over the page.
// Where it sits (top of a page, of a drawer, of a card) is the caller's layout.
import type { ReactNode } from 'react';
import { HEADER_LEVELS, hasSlot, type HeaderKind } from './headerKinds';
import './headerFrame.css';

export interface HeaderSlots {
  /** The heading text: the one heading the header renders. */
  title: ReactNode;
  /** Id of the heading, for an `aria-labelledby` on what the header names. */
  titleId?: string;
  /** Before the title: an icon, an avatar, a back button. */
  leading?: ReactNode;
  /** Right after the title, on its line: a copyable id, a count. */
  meta?: ReactNode;
  /** Status badges (`StatusBadge`), after the title and its meta. */
  status?: ReactNode;
  /** The line under the title: what the object is, a progress, a path. */
  description?: ReactNode;
  /** The actions, at the end edge. */
  actions?: ReactNode;
  /** The compact form: smaller title, tighter padding, the description on one line. */
  compact?: boolean;
  className?: string;
  testId?: string;
}

interface HeaderFrameProps extends HeaderSlots {
  kind: HeaderKind;
  /** Placed after `actions`, always last (the drawer's close button). */
  trailing?: ReactNode;
}

export function HeaderFrame({
  kind, title, titleId, leading, meta, status, description, actions, trailing, compact = false, className, testId,
}: HeaderFrameProps) {
  const Heading = `h${HEADER_LEVELS[kind]}` as 'h1' | 'h2' | 'h3';
  const hasActions = hasSlot(actions) || hasSlot(trailing);
  return (
    <div
      className={['ui-header', `ui-${kind}-header`, className].filter(Boolean).join(' ')}
      data-kind={kind}
      data-compact={compact || undefined}
      data-testid={testId}
    >
      <div className="ui-header-identity">
        {hasSlot(leading) && <div className="ui-header-leading">{leading}</div>}
        <div className="ui-header-text">
          <div className="ui-header-title-row">
            <Heading id={titleId} className="ui-header-title">{title}</Heading>
            {hasSlot(meta) && <div className="ui-header-meta">{meta}</div>}
            {hasSlot(status) && <div className="ui-header-status">{status}</div>}
          </div>
          {hasSlot(description) && <div className="ui-header-description">{description}</div>}
        </div>
      </div>
      {hasActions && (
        <div className="ui-header-actions">
          {actions}
          {trailing}
        </div>
      )}
    </div>
  );
}
