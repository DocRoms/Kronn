// KT-1884 — where a panel inside `SidePanelShell` puts what its own header
// used to hold: the shell draws one header above its tabs, and gives each
// panel three places in it, `leading` (before the title: a way back),
// `meta` (on the line under the title, after the panel's name) and `actions`
// (before the « ⋯ » menu and the close button).
import { createContext } from 'react';

export interface SidePanelHeaderTargets {
  leading: HTMLElement | null;
  meta: HTMLElement | null;
  actions: HTMLElement | null;
}

/** Null outside a shell: a panel rendered on its own draws its own header. */
export const SidePanelHeaderContext = createContext<SidePanelHeaderTargets | null>(null);
