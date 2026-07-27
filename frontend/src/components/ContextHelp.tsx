import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { CircleHelp, X } from 'lucide-react';
import './ContextHelp.css';

interface ContextHelpProps {
  title: string;
  children: ReactNode;
  align?: 'start' | 'end';
}

/**
 * Small, reusable explanation panel for product concepts that are difficult to
 * infer from a label alone. It stays opt-in so help does not crowd experienced
 * users, while remaining keyboard accessible and dismissible.
 */
export function ContextHelp({
  title,
  children,
  align = 'start',
}: ContextHelpProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const closeAndRestoreFocus = useCallback(() => {
    setOpen(false);
    requestAnimationFrame(() => triggerRef.current?.focus());
  }, []);

  useEffect(() => {
    if (!open) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        closeAndRestoreFocus();
      }
    };
    const closeOnOutsideClick = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('keydown', closeOnEscape);
    document.addEventListener('pointerdown', closeOnOutsideClick);
    return () => {
      document.removeEventListener('keydown', closeOnEscape);
      document.removeEventListener('pointerdown', closeOnOutsideClick);
    };
  }, [open, closeAndRestoreFocus]);

  return (
    <div className="kr-context-help" data-align={align} ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        className="kr-context-help-trigger"
        title={title}
        aria-label={title}
        aria-expanded={open}
        aria-controls={open ? `${titleId}-panel` : undefined}
        onClick={() => setOpen(current => !current)}
      >
        <CircleHelp size={14} />
      </button>
      {open && (
        <section
          id={`${titleId}-panel`}
          className="kr-context-help-panel"
          role="dialog"
          aria-labelledby={titleId}
        >
          <header>
            <strong id={titleId}><CircleHelp size={13} /> {title}</strong>
            <button
              type="button"
              onClick={closeAndRestoreFocus}
              aria-label={`× ${title}`}
            >
              <X size={13} />
            </button>
          </header>
          <div className="kr-context-help-content">{children}</div>
        </section>
      )}
    </div>
  );
}
