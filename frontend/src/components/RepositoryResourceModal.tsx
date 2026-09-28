import { useEffect, useId, useRef, type ReactNode } from 'react';
import { X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import './RepositoryResourceSheets.css';

interface Props {
  title: string;
  subtitle?: ReactNode;
  size?: 'dialog' | 'sheet';
  testId?: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
}

/** Dialog shell: Escape and a backdrop click close it; full screen on a phone. */
export function RepositoryResourceModal({ title, subtitle, size = 'sheet', testId, onClose, children, footer }: Props) {
  const { t } = useT();
  const titleId = useId();
  const closeRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);

  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    closeRef.current?.focus();
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onCloseRef.current();
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => {
      window.removeEventListener('keydown', closeOnEscape);
      previous?.focus?.();
    };
  }, []);

  return (
    <div className="rr-modal-backdrop" onClick={onClose}>
      <div
        className="rr-modal"
        data-size={size}
        data-testid={testId}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={event => event.stopPropagation()}
      >
        <header className="rr-modal-header">
          <div>
            <h2 id={titleId}>{title}</h2>
            {subtitle && <p>{subtitle}</p>}
          </div>
          <button ref={closeRef} type="button" className="rr-icon-button" onClick={onClose} aria-label={t('common.close')}>
            <X size={16} aria-hidden="true" />
          </button>
        </header>
        <div className="rr-modal-body">{children}</div>
        {footer && <footer className="rr-modal-footer">{footer}</footer>}
      </div>
    </div>
  );
}
