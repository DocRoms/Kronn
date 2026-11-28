import { X } from 'lucide-react';
import { HeaderFrame, type HeaderSlots } from './headerFrame';
import { IconButton } from './IconButton';
import './DrawerHeader.css';

interface DrawerHeaderBase extends HeaderSlots {
  /** The layer's name (`h2`): give the panel `aria-labelledby={titleId}`. */
  titleId?: string;
}

/** With `onClose`, the close button must be named; it always comes last. */
type DrawerHeaderClose =
  | { onClose: () => void; closeLabel: string; closeTestId?: string }
  | { onClose?: undefined; closeLabel?: undefined; closeTestId?: undefined };

export type DrawerHeaderProps = DrawerHeaderBase & DrawerHeaderClose;

/** KT-1884 — the header of a side panel or a drawer: the layer's name (`h2`),
 *  its status badges, the actions on it, then its one close button, always
 *  the last control at the end edge. */
export function DrawerHeader({ onClose, closeLabel, closeTestId, ...slots }: DrawerHeaderProps) {
  const close = onClose && closeLabel ? (
    <IconButton
      label={closeLabel}
      icon={<X />}
      size={slots.compact ? 'sm' : 'md'}
      onClick={onClose}
      className="ui-drawer-header-close"
      data-testid={closeTestId}
    />
  ) : null;
  return <HeaderFrame kind="drawer" {...slots} trailing={close} />;
}
