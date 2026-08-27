import { Star } from 'lucide-react';
import './FavoriteToggle.css';

interface FavoriteToggleProps {
  active: boolean;
  onToggle: () => void;
  activeLabel: string;
  inactiveLabel: string;
  /** Included in the accessible name as "{label} · {itemName}" so a screen
   *  reader can tell which row a toggle belongs to in a list — a bare
   *  "Add to favorites" repeated on every row isn't identifiable. */
  itemName?: string;
  size?: number;
  /** Layout-only escape hatch (spacing, ordering) for the surface embedding
   *  it — never use this to override color, size or focus styling; those
   *  are the one contract every caller shares. */
  className?: string;
}

/** Shared favorite/pin toggle — KT-464. One visual and interaction contract
 *  (hit area, aria-pressed, focus style, neutral→accent color) for
 *  Discussions, Automations and Pages row favorites, so callers never
 *  re-implement those details independently. */
export function FavoriteToggle({
  active, onToggle, activeLabel, inactiveLabel, itemName, size = 14, className,
}: FavoriteToggleProps) {
  const label = itemName ? `${active ? activeLabel : inactiveLabel} · ${itemName}` : (active ? activeLabel : inactiveLabel);
  return (
    <button
      type="button"
      className={`kr-favorite-toggle ${className ?? ''}`.trim()}
      data-active={active}
      aria-pressed={active}
      aria-label={label}
      title={active ? activeLabel : inactiveLabel}
      onClick={event => {
        // A row favorite always sits next to (or inside) a bigger clickable
        // row — never let the toggle also open/select that row.
        event.stopPropagation();
        onToggle();
      }}
    >
      <Star size={size} fill={active ? 'currentColor' : 'none'} />
    </button>
  );
}
