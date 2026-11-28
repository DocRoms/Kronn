import { createContext } from 'react';

/** KT-1923 — which folds of Settings are open: the page holds it, so an
 *  address or a search hit that lands on a fold can open it. A module of its
 *  own, so the component files export components only (fast refresh). */
export interface SettingsFoldState {
  isOpen: (id: string) => boolean;
  setOpen: (id: string, open: boolean) => void;
}

export const SettingsFoldContext = createContext<SettingsFoldState | null>(null);
