import { createContext, useContext } from 'react';

export type LayoutDensity = 'small' | 'medium' | 'large';

export interface LayoutDensityContextValue {
  density: LayoutDensity;
  setDensity: (density: LayoutDensity) => void;
}

export const DEFAULT_DENSITY: LayoutDensity = 'medium';

export const LayoutDensityContext = createContext<LayoutDensityContextValue>({
  density: DEFAULT_DENSITY,
  setDensity: () => {},
});

export function useLayoutDensity() {
  return useContext(LayoutDensityContext);
}
