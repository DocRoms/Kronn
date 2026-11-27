// One side panel for the discussion and the Artifact pages: full window height
// (over the top navigation), a vertical rail of named views on its left, and
// ONE header above it (KT-1884, a `DrawerHeader`): the view's way back if it
// has one, the subject's title, the active view's name and meta under it, the
// view's own actions, a « ⋯ » menu and the close button. A view inside the
// shell draws no header of its own: it hands its leading control, meta and
// actions to this one (`SidePanelHeaderSlot`).
import { useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent, MutableRefObject, ReactNode } from 'react';
import { DrawerHeader } from './ui/DrawerHeader';
import { Menu, type MenuItem } from './ui/Menu';
import { SidePanelHeaderContext } from './sidePanelHeaderContext';
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
  const [leadingTarget, setLeadingTarget] = useState<HTMLElement | null>(null);
  const [metaTarget, setMetaTarget] = useState<HTMLElement | null>(null);
  const [actionsTarget, setActionsTarget] = useState<HTMLElement | null>(null);
  const headerTargets = useMemo(
    () => ({ leading: leadingTarget, meta: metaTarget, actions: actionsTarget }),
    [leadingTarget, metaTarget, actionsTarget],
  );

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

  // The menu is the shared one: it renders in <body>, above the panel and
  // whatever the active view draws, so its entries are never under a header.
  const toItem = (action: SidePanelMenuAction): MenuItem => ({
    id: action.id,
    label: action.label,
    icon: action.icon,
    disabled: action.disabled,
    onSelect: action.onSelect,
  });
  const destructive = menuActions.find(action => action.danger);
  const activeItem = items.find(item => item.id === activeId);

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
        <DrawerHeader
          compact
          className="side-panel-shell-header"
          title={title}
          leading={<span ref={setLeadingTarget} className="side-panel-shell-view-leading" />}
          description={(
            <>
              {activeItem && <span className="side-panel-shell-view">{activeItem.label}</span>}
              <span ref={setMetaTarget} className="side-panel-shell-meta" />
            </>
          )}
          actions={(
            <>
              <span ref={setActionsTarget} className="side-panel-shell-view-actions" />
              {menuActions.length > 0 && menuLabel && (
                <Menu
                  label={menuLabel}
                  size="sm"
                  triggerTestId="side-panel-menu-btn"
                  items={menuActions.filter(action => action !== destructive).map(toItem)}
                  destructiveItem={destructive ? toItem(destructive) : undefined}
                />
              )}
            </>
          )}
          onClose={onClose}
          closeLabel={closeLabel}
          closeTestId={closeTestId}
        />
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
            <SidePanelHeaderContext.Provider value={headerTargets}>
              {children}
            </SidePanelHeaderContext.Provider>
          </div>
        </div>
      </aside>
    </>
  );
}
