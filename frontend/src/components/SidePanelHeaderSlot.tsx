// KT-1884 — a panel's header, as it shows inside or outside `SidePanelShell`.
// Inside, the panel draws no header of its own: its `leading`, `meta` and
// `actions` go into the shell's one header. Outside, it draws `standalone`,
// its own header.
import { useContext, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { hasSlot } from './ui/headerKinds';
import { SidePanelHeaderContext } from './sidePanelHeaderContext';

export interface SidePanelHeaderSlotProps {
  /** Before the shell's title, first in the tab order: a way back. */
  leading?: ReactNode;
  /** Shown after the panel's name, under the shell's title (a count, a progress, a path). */
  meta?: ReactNode;
  /** The panel's own actions, before the shell's menu and close button. */
  actions?: ReactNode;
  /** The panel's own header, drawn only outside a shell. */
  standalone: ReactNode;
}

export function SidePanelHeaderSlot({ leading, meta, actions, standalone }: SidePanelHeaderSlotProps) {
  const targets = useContext(SidePanelHeaderContext);
  if (!targets) return <>{standalone}</>;
  return (
    <>
      {hasSlot(leading) && targets.leading && createPortal(leading, targets.leading)}
      {hasSlot(meta) && targets.meta && createPortal(meta, targets.meta)}
      {hasSlot(actions) && targets.actions && createPortal(actions, targets.actions)}
    </>
  );
}
