import {
  Fragment, useCallback, useEffect, useId, useLayoutEffect, useRef, useState,
  type KeyboardEvent, type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { MoreHorizontal } from 'lucide-react';
import { useAnchoredStyle } from './anchor';
import { Button, type ButtonSize, type ButtonVariant } from './Button';
import { IconButton, type NonEmptyLabel } from './IconButton';
import './Menu.css';

export interface MenuItem {
  id: string;
  label: string;
  /** An icon before the label, hidden from assistive technology. */
  icon?: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
}

export interface MenuProps<L extends string = string> {
  /** Name of the trigger, required: what the menu acts on ("Actions on this task"). */
  label: NonEmptyLabel<L>;
  items: readonly MenuItem[];
  /** The destructive action. A menu has at most one, and it is always the
   *  last entry, after a separator, in the danger colour: the API leaves no
   *  other place for it. */
  destructiveItem?: MenuItem;
  /** Trigger icon (default: three dots). */
  icon?: ReactNode;
  /** A trigger with the label written out, instead of an icon only. */
  showLabel?: boolean;
  /** Which edge of the trigger the menu lines up with (default end). */
  align?: 'start' | 'end';
  size?: ButtonSize;
  variant?: ButtonVariant;
  /** `data-testid` of the trigger. */
  triggerTestId?: string;
}

/** KT-1880 — a menu button (WAI-ARIA menu pattern). Enter, Space or Arrow
 *  Down opens it on the first item, Arrow Up on the last; the arrows, Home and
 *  End move between items, a letter jumps to the next item starting with it;
 *  Enter or Space chooses; Escape and Tab close it and give focus back to the
 *  trigger. A click outside closes it. */
export function Menu<L extends string>({
  label, items, destructiveItem, icon, showLabel = false, align = 'end', size = 'md', variant = 'ghost', triggerTestId,
}: MenuProps<L>) {
  const base = useId();
  const triggerId = `${base}-trigger`;
  const menuId = `${base}-menu`;
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [open, setOpen] = useState(false);
  /** Index in `entries` of the item to focus; -1 when there is none. */
  const [focus, setFocus] = useState(-1);
  const { style, placed } = useAnchoredStyle(triggerRef, menuRef, open, 'bottom', align);

  const entries = destructiveItem ? [...items, destructiveItem] : [...items];
  const enabled = entries.map((entry, i) => (entry.disabled ? -1 : i)).filter(i => i >= 0);

  const openAt = (at: 'first' | 'last') => {
    setFocus(enabled.length === 0 ? -1 : at === 'first' ? enabled[0] : enabled[enabled.length - 1]);
    setOpen(true);
  };
  const close = useCallback((returnFocus: boolean) => {
    setOpen(false);
    if (returnFocus) triggerRef.current?.focus();
  }, []);

  // Focus the asked item once the menu is placed: before that it is hidden to
  // be measured, and a browser refuses focus on a hidden element. Then scroll
  // it into view, inside the max-height the placement may have set.
  useLayoutEffect(() => {
    if (!open || !placed || focus < 0) return;
    const item = itemRefs.current[focus];
    item?.focus({ preventScroll: true });
    item?.scrollIntoView?.({ block: 'nearest' });
  }, [open, placed, focus]);

  // A press outside the trigger and the menu closes it, focus stays where it lands.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (menuRef.current?.contains(target) || triggerRef.current?.contains(target)) return;
      close(false);
    };
    document.addEventListener('pointerdown', onPointerDown);
    return () => document.removeEventListener('pointerdown', onPointerDown);
  }, [open, close]);

  const onTriggerKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); openAt('first'); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); openAt('last'); }
  };

  const onMenuKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const current = itemRefs.current.findIndex(el => el === document.activeElement);
    const pos = enabled.indexOf(current);
    const move = (to: number) => { e.preventDefault(); if (enabled.length) setFocus(enabled[to]); };
    switch (e.key) {
      case 'ArrowDown': move(pos < 0 ? 0 : (pos + 1) % enabled.length); break;
      case 'ArrowUp': move(pos < 0 ? enabled.length - 1 : (pos - 1 + enabled.length) % enabled.length); break;
      case 'Home': move(0); break;
      case 'End': move(enabled.length - 1); break;
      case 'Escape':
      case 'Tab':
        e.preventDefault();
        e.stopPropagation();
        close(true);
        break;
      default:
        if (e.key.length === 1 && /\S/.test(e.key) && !e.ctrlKey && !e.metaKey && !e.altKey) {
          const key = e.key.toLocaleLowerCase();
          const order = [...enabled.slice(pos + 1), ...enabled.slice(0, pos + 1)];
          const hit = order.find(i => entries[i].label.toLocaleLowerCase().startsWith(key));
          if (hit !== undefined) { e.preventDefault(); setFocus(hit); }
        }
    }
  };

  const choose = (entry: MenuItem) => {
    close(true);
    entry.onSelect();
  };

  const triggerProps = {
    id: triggerId,
    ref: triggerRef,
    size,
    variant,
    'aria-haspopup': 'menu' as const,
    'aria-expanded': open,
    'aria-controls': open ? menuId : undefined,
    onClick: () => (open ? close(false) : openAt('first')),
    onKeyDown: onTriggerKeyDown,
    'data-testid': triggerTestId,
  };

  return (
    <>
      {showLabel
        ? <Button {...triggerProps} icon={icon}>{label}</Button>
        : <IconButton {...triggerProps} label={label} icon={icon ?? <MoreHorizontal />} />}
      {open && createPortal(
        <div
          ref={menuRef}
          id={menuId}
          role="menu"
          aria-labelledby={triggerId}
          className="ui-menu"
          style={style}
          onKeyDown={onMenuKeyDown}
        >
          {entries.map((entry, i) => {
            const destructive = destructiveItem !== undefined && i === entries.length - 1;
            return (
              <Fragment key={entry.id}>
                {destructive && items.length > 0 && <div role="separator" className="ui-menu-separator" />}
                <button
                  ref={el => { itemRefs.current[i] = el; }}
                  type="button"
                  role="menuitem"
                  tabIndex={-1}
                  className="ui-menu-item"
                  data-destructive={destructive || undefined}
                  disabled={entry.disabled}
                  onClick={() => choose(entry)}
                >
                  {entry.icon && <span className="ui-menu-item-icon" aria-hidden="true">{entry.icon}</span>}
                  <span>{entry.label}</span>
                </button>
              </Fragment>
            );
          })}
        </div>,
        document.body,
      )}
    </>
  );
}
