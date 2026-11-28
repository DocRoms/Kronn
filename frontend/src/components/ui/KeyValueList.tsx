import type { ReactNode } from 'react';
import { useT } from '../../lib/I18nContext';
import { CopyButton } from './CopyButton';
import type { KeyValueLayout } from './dataKinds';
import './visuallyHidden.css';
import './KeyValueList.css';

export interface KeyValueItem {
  /** Stable key when two labels are the same; defaults to the label. */
  id?: string;
  label: string;
  /** null, undefined or '' is a missing value: "—". */
  value?: ReactNode;
  /** An id, a path, a hash: the mono font, wrapping anywhere. */
  mono?: boolean;
  /** Adds a CopyButton named after the label ("Copy {label}"). */
  copyable?: boolean;
  /** What the CopyButton copies when `value` is not plain text. */
  copyText?: string;
  /** One line, cut with an ellipsis, the full value in a title. */
  truncate?: boolean;
}

export interface KeyValueListProps {
  items: readonly KeyValueItem[];
  layout?: KeyValueLayout;
  className?: string;
}

const isMissing = (value: ReactNode) => value === null || value === undefined || value === '' || value === false;
const plainText = (value: ReactNode) => (typeof value === 'string' || typeof value === 'number' ? String(value) : undefined);

/** KT-1886 — labels and their values, as a description list. The frame
 *  around it is a size container (ui-kv): under 24rem of its own width the
 *  label goes above its value. */
export function KeyValueList({ items, layout = 'grid', className }: KeyValueListProps) {
  const { t } = useT();
  return (
    <div className={className ? `ui-kv-frame ${className}` : 'ui-kv-frame'}>
      <dl className="ui-kv" data-layout={layout}>
        {items.map(item => {
          const missing = isMissing(item.value);
          const text = item.copyText ?? plainText(item.value);
          return (
            <div key={item.id ?? item.label} className="ui-kv-item">
              <dt>{item.label}</dt>
              <dd
                data-missing={missing ? 'true' : undefined}
                data-mono={item.mono && !missing ? 'true' : undefined}
                data-truncate={item.truncate && !missing ? 'true' : undefined}
              >
                {missing ? (
                  <>
                    <span aria-hidden="true">—</span>
                    <span className="ui-visually-hidden">{t('ui.keyValue.missing')}</span>
                  </>
                ) : (
                  <span
                    className={item.truncate ? 'ui-kv-value ui-kv-truncate' : 'ui-kv-value'}
                    title={item.truncate ? text : undefined}
                  >
                    {item.value}
                  </span>
                )}
                {item.copyable && !missing && text !== undefined && (
                  <CopyButton text={text} label={t('ui.keyValue.copy', item.label)} size="sm" />
                )}
              </dd>
            </div>
          );
        })}
      </dl>
    </div>
  );
}
