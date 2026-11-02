// One side panel for the discussion and the Artifact pages: full window height
// (over the top navigation), a vertical rail of named views on its left, the
// subject's title, a « ⋯ » menu for actions and a close button in its header.
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent, MutableRefObject, ReactNode } from 'react';
import { MoreHorizontal, X } from 'lucide-react';
import './SidePanelShell.css';

export interface SidePanelRailItem {
  id: string;
  /** Short name shown under the icon. */
  label: string;
  /** Tooltip; defaults to `label`. The accessible name stays the visible label. */
  description?: string;
  icon: ReactNode;
  badge?: ReactNode;
}

export interface SidePanelMenuAction {
  id: string;
  label: string;
  icon: ReactNode;
  onSelect: () => void;
  danger?: boolean;
  disabled?: boolean;
}

export interface SidePanelShellProps {
  id?: string;
  testId?: string;
  ariaLabel: string;
  title: ReactNode;
  railLabel: string;
  items: SidePanelRailItem[];
  activeId: string;
  onSelect: (id: string) => void;
  onClose: () => void;
  closeLabel: string;
  closeTestId?: string;
  menuLabel?: string;
  menuActions?: SidePanelMenuAction[];
  /** Id given to the tab panel, for `aria-controls`. */
  bodyId?: string;
  /** Prefix of each rail tab's id and test id (`${tabIdPrefix}${item.id}`). */
  tabIdPrefix?: string;
  tabTestIdPrefix?: string;
  /** The page's own ref on the panel (outside-click checks). */
  panelRef?: MutableRefObject<HTMLElement | null>;
  className?: string;
  children: ReactNode;
}

export function SidePanelShell({
  id,
  testId,
  ariaLabel,
  title,
  railLabel,
  items,
  activeId,
  onSelect,
  onClose,
  closeLabel,
  closeTestId,
  menuLabel,
  menuActions = [],
  bodyId,
  tabIdPrefix,
  tabTestIdPrefix,
  panelRef,
  className,
  children,
}: SidePanelShellProps) {
  const autoId = useId();
  const prefix = tabIdPrefix ?? `side-panel-tab-${autoId}-`;
  const panelBodyId = bodyId ?? `side-panel-body-${autoId}`;
  const tabRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const localRef = useRef<HTMLElement | null>(null);
  const asideRef = panelRef ?? localRef;
  const [reserved, setReserved] = useState(0);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);

  // The panel floats over the navigation, so a spacer keeps the page content
  // from sliding under it (the panel used to sit in the page's flex row).
  useLayoutEffect(() => {
    const node = asideRef.current;
    if (!node) return undefined;
    const measure = () => setReserved(node.getBoundingClientRect().width);
    measure();
    if (typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, [asideRef]);

  useEffect(() => {
    if (!menuOpen) return undefined;
    const onPointerDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (menuRef.current?.contains(target) || menuButtonRef.current?.contains(target)) return;
      setMenuOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      // Closes the menu only, not the panel behind it.
      event.preventDefault();
      event.stopPropagation();
      setMenuOpen(false);
      menuButtonRef.current?.focus();
    };
    window.document.addEventListener('mousedown', onPointerDown);
    window.document.addEventListener('keydown', onKeyDown, true);
    return () => {
      window.document.removeEventListener('mousedown', onPointerDown);
      window.document.removeEventListener('keydown', onKeyDown, true);
    };
  }, [menuOpen]);

  useEffect(() => {
    if (menuOpen) menuRef.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
  }, [menuOpen]);

  const onRailKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const current = items.findIndex(item => item.id === activeId);
    let next = -1;
    if (event.key === 'ArrowDown') next = (current + 1) % items.length;
    else if (event.key === 'ArrowUp') next = (current - 1 + items.length) % items.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = items.length - 1;
    if (next < 0) return;
    event.preventDefault();
    onSelect(items[next].id);
    tabRefs.current[next]?.focus();
  };

  const onMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    event.preventDefault();
    const buttons = [...(menuRef.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])];
    const index = buttons.indexOf(window.document.activeElement as HTMLButtonElement);
    const step = event.key === 'ArrowDown' ? 1 : -1;
    buttons[(index + step + buttons.length) % buttons.length]?.focus();
  };

  return (
    <>
      <div className="side-panel-shell-spacer" style={{ width: reserved }} aria-hidden="true" />
      <aside
        id={id}
        ref={asideRef}
        className={['side-panel-shell', className].filter(Boolean).join(' ')}
        data-testid={testId}
        aria-label={ariaLabel}
      >
        <header className="side-panel-shell-header">
          <h2 className="side-panel-shell-title">{title}</h2>
          <div className="side-panel-shell-actions">
            {menuActions.length > 0 && menuLabel && (
              <div className="side-panel-shell-menu-wrap">
                <button
                  ref={menuButtonRef}
                  type="button"
                  className="side-panel-shell-icon"
                  aria-label={menuLabel}
                  title={menuLabel}
                  aria-haspopup="menu"
                  aria-expanded={menuOpen}
                  data-testid="side-panel-menu-btn"
                  onClick={() => setMenuOpen(open => !open)}
                >
                  <MoreHorizontal size={16} />
                </button>
                {menuOpen && (
                  <div
                    ref={menuRef}
                    className="side-panel-shell-menu"
                    role="menu"
                    aria-label={menuLabel}
                    onKeyDown={onMenuKeyDown}
                  >
                    {menuActions.map(action => (
                      <button
                        key={action.id}
                        type="button"
                        role="menuitem"
                        data-action={action.id}
                        data-danger={action.danger || undefined}
                        disabled={action.disabled}
                        onClick={() => {
                          setMenuOpen(false);
                          action.onSelect();
                        }}
                      >
                        {action.icon}
                        <span>{action.label}</span>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}
            <button
              type="button"
              className="side-panel-shell-icon"
              onClick={onClose}
              aria-label={closeLabel}
              title={closeLabel}
              data-testid={closeTestId}
            >
              <X size={16} />
            </button>
          </div>
        </header>
        <div className="side-panel-shell-main">
          <div
            className="side-panel-shell-rail"
            role="tablist"
            aria-orientation="vertical"
            aria-label={railLabel}
            onKeyDown={onRailKeyDown}
          >
            {items.map((item, index) => {
              const active = item.id === activeId;
              return (
                <button
                  key={item.id}
                  ref={node => { tabRefs.current[index] = node; }}
                  type="button"
                  role="tab"
                  id={`${prefix}${item.id}`}
                  data-testid={tabTestIdPrefix ? `${tabTestIdPrefix}${item.id}` : undefined}
                  data-panel={item.id}
                  data-active={active}
                  aria-selected={active}
                  aria-controls={panelBodyId}
                  title={item.description ?? item.label}
                  tabIndex={active ? 0 : -1}
                  className="side-panel-shell-tab"
                  onClick={() => onSelect(item.id)}
                >
                  <span className="side-panel-shell-tab-icon" aria-hidden="true">
                    {item.icon}
                    {item.badge != null && <span className="side-panel-shell-badge">{item.badge}</span>}
                  </span>
                  <span className="side-panel-shell-tab-label">{item.label}</span>
                </button>
              );
            })}
          </div>
          <div
            className="side-panel-shell-body"
            id={panelBodyId}
            role="tabpanel"
            aria-labelledby={`${prefix}${activeId}`}
          >
            {children}
          </div>
        </div>
      </aside>
    </>
  );
}
