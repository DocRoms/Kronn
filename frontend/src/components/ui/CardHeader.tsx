import { HeaderFrame, type HeaderSlots } from './headerFrame';
import './CardHeader.css';

export interface CardHeaderProps extends HeaderSlots {
  /** The card's name (`h2`): give the card `aria-labelledby={titleId}`. */
  titleId?: string;
}

/** KT-1884 — the header of one card: its name, its state as `StatusBadge`s in
 *  `status`, a line under it, a menu or one action on the right. */
export function CardHeader(props: CardHeaderProps) {
  return <HeaderFrame kind="card" {...props} />;
}
