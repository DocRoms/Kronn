import { HeaderFrame, type HeaderSlots } from './headerFrame';
import './SectionHeader.css';

export type SectionHeaderProps = HeaderSlots;

/** KT-1884 — the header of a group of content inside a detail, a tab or a
 *  drawer: its name (`h3`), a hint under it, one or two actions. */
export function SectionHeader(props: SectionHeaderProps) {
  return <HeaderFrame kind="section" {...props} />;
}
