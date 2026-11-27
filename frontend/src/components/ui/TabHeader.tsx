import { HeaderFrame, type HeaderSlots } from './headerFrame';
import './TabHeader.css';

export type TabHeaderProps = HeaderSlots;

/** KT-1884 — the header of one tab of a detail, under its tab bar: the tab's
 *  subject (`h3`), its count, the actions that act on the whole tab. */
export function TabHeader(props: TabHeaderProps) {
  return <HeaderFrame kind="tab" {...props} />;
}
