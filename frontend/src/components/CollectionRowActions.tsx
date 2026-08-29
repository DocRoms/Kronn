import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Check, Copy, MoreHorizontal } from 'lucide-react';
import { FavoriteToggle } from './FavoriteToggle';
import '../pages/DiscussionsPage.css';

export interface CollectionRowMenuAction {
  id: string;
  label: string;
  icon: ReactNode;
  onSelect: () => void | Promise<void>;
  danger?: boolean;
}

interface CollectionRowActionsProps {
  itemName: string;
  favorite: {
    active: boolean;
    onToggle: () => void;
    activeLabel: string;
    inactiveLabel: string;
  };
  menuLabel: string;
  copyId: string;
  copyLabel: string;
  actions?: CollectionRowMenuAction[];
}

/** Shared Discussion-style actions for every collection row. */
export function CollectionRowActions({
  itemName,
  favorite,
  menuLabel,
  copyId,
  copyLabel,
  actions = [],
}: CollectionRowActionsProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [menuPlacement, setMenuPlacement] = useState<'up' | 'down'>('down');
  const [copied, setCopied] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);
  const copiedTimerRef = useRef<number | null>(null);

  useEffect(() => () => {
    if (copiedTimerRef.current !== null) window.clearTimeout(copiedTimerRef.current);
  }, []);

  useEffect(() => {
    if (!menuOpen) return;
    const closeFromOutside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    const closeFromKeyboard = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setMenuOpen(false);
    };
    window.addEventListener('pointerdown', closeFromOutside);
    window.addEventListener('keydown', closeFromKeyboard);
    return () => {
      window.removeEventListener('pointerdown', closeFromOutside);
      window.removeEventListener('keydown', closeFromKeyboard);
    };
  }, [menuOpen]);

  const copyItemId = async () => {
    try {
      await navigator.clipboard.writeText(copyId);
      setCopied(true);
      if (copiedTimerRef.current !== null) window.clearTimeout(copiedTimerRef.current);
      copiedTimerRef.current = window.setTimeout(() => setCopied(false), 1200);
    } catch {
      // Keep the menu open so clipboard permission failures can be retried.
    }
  };

  return (
    <div className="disc-item-actions" ref={rootRef}>
      <FavoriteToggle
        active={favorite.active}
        onToggle={favorite.onToggle}
        activeLabel={favorite.activeLabel}
        inactiveLabel={favorite.inactiveLabel}
        itemName={itemName}
      />
      <button
        ref={menuButtonRef}
        type="button"
        className="disc-item-more-btn"
        onClick={event => {
          event.stopPropagation();
          if (!menuOpen) {
            const rect = menuButtonRef.current?.getBoundingClientRect();
            setMenuPlacement(rect && window.innerHeight - rect.bottom < 180 ? 'up' : 'down');
          }
          setMenuOpen(open => !open);
        }}
        aria-label={`${menuLabel} · ${itemName}`}
        aria-expanded={menuOpen}
        title={menuLabel}
      >
        <MoreHorizontal size={14} />
      </button>
      {menuOpen && (
        <div className="disc-item-action-menu" role="menu" data-placement={menuPlacement}>
          <button type="button" role="menuitem" data-copied={copied} onClick={() => void copyItemId()}>
            {copied ? <Check size={12} /> : <Copy size={12} />}
            {copyLabel}
          </button>
          {actions.map(action => (
            <button
              type="button"
              role="menuitem"
              key={action.id}
              className={action.danger ? 'disc-item-action-danger' : undefined}
              onClick={() => {
                setMenuOpen(false);
                void action.onSelect();
              }}
            >
              {action.icon}
              {action.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
