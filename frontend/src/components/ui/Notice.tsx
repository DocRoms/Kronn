import type { ReactNode } from 'react';
import { CircleAlert, CircleCheck, Info, TriangleAlert, X } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { Button } from './Button';
import { IconButton } from './IconButton';
import type { NoticeTone } from './feedbackKinds';
import './Notice.css';

const ICONS: Record<NoticeTone, ReactNode> = {
  info: <Info />,
  warning: <TriangleAlert />,
  danger: <CircleAlert />,
  success: <CircleCheck />,
};

export interface NoticeAction {
  label: string;
  onClick: () => void;
}

export interface NoticeProps {
  tone: NoticeTone;
  /** A short first line in bold. */
  title?: ReactNode;
  /** What happened or what to know, in words: the colour never carries it alone. */
  children: ReactNode;
  /** One action that answers the notice ("Retry", "Open the settings"). */
  action?: NoticeAction;
  /** Makes the notice dismissible: a cross named "Dismiss" calls it. */
  onDismiss?: () => void;
  className?: string;
}

/** KT-1883 — a message inside the page it concerns, in its one notice slot
 *  (under the page header, above the content). The tone is on the icon, the
 *  border and the tint; the text stays in a text colour. A danger notice is an
 *  alert (read at once), the others a polite status. */
export function Notice({ tone, title, children, action, onDismiss, className }: NoticeProps) {
  const { t } = useT();
  return (
    <div
      className={className ? `ui-notice ${className}` : 'ui-notice'}
      data-tone={tone}
      role={tone === 'danger' ? 'alert' : 'status'}
    >
      <span className="ui-notice-icon" aria-hidden="true">{ICONS[tone]}</span>
      <div className="ui-notice-body">
        {title && <p className="ui-notice-title">{title}</p>}
        <div className="ui-notice-message">{children}</div>
        {action && (
          <div className="ui-notice-actions">
            <Button size="sm" onClick={action.onClick}>{action.label}</Button>
          </div>
        )}
      </div>
      {onDismiss && (
        <IconButton className="ui-notice-dismiss" size="sm" label={t('ui.notice.dismiss')} icon={<X />} onClick={onDismiss} />
      )}
    </div>
  );
}
