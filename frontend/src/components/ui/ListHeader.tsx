import { HeaderFrame, type HeaderSlots } from './headerFrame';
import './ListHeader.css';

export type ListHeaderProps = HeaderSlots;

/** KT-1884 — the page header of a list page: the page's one `h1`, its count
 *  or scope on the line under it, the page actions (create, import) on the
 *  right. */
export function ListHeader(props: ListHeaderProps) {
  return <HeaderFrame kind="list" {...props} />;
}
