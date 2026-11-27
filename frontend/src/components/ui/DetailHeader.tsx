import { HeaderFrame, type HeaderSlots } from './headerFrame';
import './DetailHeader.css';

export type DetailHeaderProps = HeaderSlots;

/** KT-1884 — the header of the object a list opens: its name (`h2`), its id
 *  as `meta`, its state as `status`, what it is on the line under it, the
 *  actions on it on the right. `DetailHeaderShell` (the discussion and the
 *  artifact pages) moves to it with the other details in lots 2-5. */
export function DetailHeader(props: DetailHeaderProps) {
  return <HeaderFrame kind="detail" {...props} />;
}
