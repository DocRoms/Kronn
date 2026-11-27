import { useId, useMemo, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';
import { useDialogFocus, focusableIn } from '../../hooks/useDialogFocus';
import { useT } from '../../lib/I18nContext';
import { IconButton } from './IconButton';
import './overlay.css';

/** sm = --kr-overlay-w-sm (480px), md = --kr-overlay-w-md (640px),
 *  lg = --kr-overlay-w-lg (960px). Under the first breakpoint every size
 *  fills the screen. */
export type OverlaySize = 'sm' | 'md' | 'lg';

/** The stacking layer: a drawer under a dialog, a nested dialog (a
 *  confirmation opened from a dialog or a drawer) above both. */
export type OverlayLayer = 'drawer' | 'modal' | 'nested';

/** What Dialog and Drawer take. */
export interface OverlayProps {
  open: boolean;
  /** Escape, the cross and a click on the scrim call it; the caller closes. */
  onClose: () => void;
  /** The visible heading, which is also the dialog's accessible name. */
  title: ReactNode;
  /** One line under the title, wired as the accessible description. */
  description?: ReactNode;
  children?: ReactNode;
  /** The actions, at the bottom (a Cancel, then the main action last). */
  footer?: ReactNode;
  size?: OverlaySize;
  /** alertdialog for a question that needs an answer before going on. */
  role?: 'dialog' | 'alertdialog';
  /** Focus on open; defaults to the first control of the content, then of
   *  the footer, then the cross. */
  initialFocus?: RefObject<HTMLElement | null>;
  /** Close on a click on the scrim (default true). */
  closeOnScrim?: boolean;
  layer?: OverlayLayer;
  className?: string;
  'data-testid'?: string;
}

/** KT-1881 — the frame Dialog and Drawer share: rendered in <body>, a scrim,
 *  a panel with role, aria-modal, a name from its heading and one cross at the
 *  top end. useDialogFocus gives the initial focus, keeps Tab inside, closes
 *  on Escape (one layer per press) and gives the focus back to the opener. */
export function OverlayFrame({
  kind, open, onClose, title, description, children, footer, size = 'md', role = 'dialog',
  initialFocus, closeOnScrim = true, layer, className, 'data-testid': testId,
}: OverlayProps & { kind: 'dialog' | 'drawer' }) {
  const { t } = useT();
  const base = useId();
  const titleId = `${base}-title`;
  const descriptionId = `${base}-description`;

  // Resolved when the layer opens, after its content is in the DOM.
  const firstFocus = useMemo<RefObject<HTMLElement | null>>(() => ({
    get current() {
      const panel = document.getElementById(titleId)?.closest<HTMLElement>('.ui-overlay-panel');
      if (initialFocus?.current) return initialFocus.current;
      if (!panel) return null;
      for (const part of ['.ui-overlay-body', '.ui-overlay-footer']) {
        const region = panel.querySelector<HTMLElement>(`:scope > ${part}`);
        const first = region ? focusableIn(region)[0] : undefined;
        if (first) return first;
      }
      return null;
    },
    set current(_value) { /* read-only: computed on open */ },
  }), [initialFocus, titleId]);

  const panelRef = useDialogFocus<HTMLElement>({ active: open, onEscape: onClose, initialFocus: firstFocus });

  if (!open) return null;
  const panelClass = ['ui-overlay-panel', `ui-${kind}`, className].filter(Boolean).join(' ');
  return createPortal(
    <div className="ui-overlay" data-kind={kind} data-layer={layer ?? (kind === 'drawer' ? 'drawer' : 'modal')}>
      <div className="ui-overlay-scrim" aria-hidden="true" onClick={closeOnScrim ? onClose : undefined} />
      <section
        ref={panelRef}
        className={panelClass}
        role={role}
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
        data-size={size}
        data-testid={testId}
        tabIndex={-1}
      >
        <header className="ui-overlay-header">
          <h2 id={titleId} className="ui-overlay-title">{title}</h2>
          <IconButton className="ui-overlay-close" label={t('ui.overlay.close')} icon={<X />} onClick={onClose} />
        </header>
        {description && <p id={descriptionId} className="ui-overlay-description">{description}</p>}
        <div className="ui-overlay-body">{children}</div>
        {footer && <footer className="ui-overlay-footer">{footer}</footer>}
      </section>
    </div>,
    document.body,
  );
}
