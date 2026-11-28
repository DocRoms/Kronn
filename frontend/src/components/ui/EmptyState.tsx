import type { ReactNode } from 'react';
import { Inbox, MousePointerClick, SearchX } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { Button } from './Button';
import type { EmptyStateKind } from './feedbackKinds';
import './EmptyState.css';

interface EmptyStateBase {
  /** What is empty, in one short line ("No workflow yet"). */
  title: ReactNode;
  /** What the list holds once filled, or why it is empty. */
  description?: ReactNode;
  /** Replaces the default icon of the kind; hidden from assistive technology. */
  icon?: ReactNode;
  className?: string;
}

export interface EmptyStateAction {
  /** The verb and the object: "Create a workflow", never "OK" or "New". */
  label: string;
  onClick: () => void;
  icon?: ReactNode;
}

/** First use: the action that creates the first item is required. */
export interface FirstUseEmptyState extends EmptyStateBase {
  kind: 'firstUse';
  action: EmptyStateAction;
}

/** No result: the button that clears the search or the filters is required. */
export interface NoResultEmptyState extends EmptyStateBase {
  kind: 'noResult';
  onClear: () => void;
  /** Defaults to "Clear the filters". */
  clearLabel?: string;
}

/** Nothing selected: no action, the list next to it is the way out. */
export interface NothingSelectedEmptyState extends EmptyStateBase {
  kind: 'nothingSelected';
}

export type EmptyStateProps = FirstUseEmptyState | NoResultEmptyState | NothingSelectedEmptyState;

const ICONS: Record<EmptyStateKind, ReactNode> = {
  firstUse: <Inbox />,
  noResult: <SearchX />,
  nothingSelected: <MousePointerClick />,
};

/** KT-1883 — what a list, a page or a detail pane shows when it has nothing
 *  to show, in one of three cases, each with its way out. */
export function EmptyState(props: EmptyStateProps) {
  const { t } = useT();
  const { kind, title, description, icon, className } = props;
  let action: ReactNode = null;
  if (props.kind === 'firstUse') {
    action = <Button variant="primary" icon={props.action.icon} onClick={props.action.onClick}>{props.action.label}</Button>;
  } else if (props.kind === 'noResult') {
    action = <Button onClick={props.onClear}>{props.clearLabel ?? t('ui.emptyState.clear')}</Button>;
  }
  return (
    <div className={className ? `ui-empty-state ${className}` : 'ui-empty-state'} data-kind={kind}>
      <span className="ui-empty-state-icon" aria-hidden="true">{icon ?? ICONS[kind]}</span>
      <p className="ui-empty-state-title">{title}</p>
      {description && <p className="ui-empty-state-description">{description}</p>}
      {action && <div className="ui-empty-state-action">{action}</div>}
    </div>
  );
}
