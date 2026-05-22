// 0.8.6 — Layout density picker.
//
// 3 modes that drive the `.dash-main` max-width via `data-density` on
// the root HTML element (mirrors how `data-theme` flips the palette) :
//
//   - `small`  (1000px)             — focused, ideal for 13-15" screens
//   - `medium` (default, 1400px)    — sweet spot for 24-27" desktops
//   - `large`  (full width, no cap) — ultrawide / dev workstation
//
// Default switched from `small` to `medium` in 0.8.6 — most installs run
// on 24"+ desktops where the legacy 1000px cap left too much dead space
// on the sides. Users on smaller screens can opt back to `small` and the
// pick is persisted ; first-launch on a fresh machine lands on `medium`.
//
// Persisted to localStorage only — no backend persistence today because
// this is a per-machine preference (the same user on a 13" laptop and a
// 32" desktop probably wants different densities).

import { createContext, useContext, useState, useEffect, useCallback, type ReactNode } from 'react';

export type LayoutDensity = 'small' | 'medium' | 'large';

const STORAGE_KEY = 'kronn:layoutDensity';
const DEFAULT_DENSITY: LayoutDensity = 'medium';

function isValidDensity(v: unknown): v is LayoutDensity {
  return v === 'small' || v === 'medium' || v === 'large';
}

function loadInitial(): LayoutDensity {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (isValidDensity(stored)) return stored;
  } catch { /* private mode / quota — fall through */ }
  return DEFAULT_DENSITY;
}

function applyDensity(density: LayoutDensity) {
  document.documentElement.setAttribute('data-density', density);
}

interface LayoutDensityContextValue {
  density: LayoutDensity;
  setDensity: (d: LayoutDensity) => void;
}

const LayoutDensityContext = createContext<LayoutDensityContextValue>({
  density: DEFAULT_DENSITY,
  setDensity: () => {},
});

export function LayoutDensityProvider({ children }: { children: ReactNode }) {
  const [density, setDensityState] = useState<LayoutDensity>(loadInitial);

  const setDensity = useCallback((d: LayoutDensity) => {
    try { localStorage.setItem(STORAGE_KEY, d); } catch { /* noop */ }
    setDensityState(d);
  }, []);

  useEffect(() => {
    applyDensity(density);
  }, [density]);

  return (
    <LayoutDensityContext.Provider value={{ density, setDensity }}>
      {children}
    </LayoutDensityContext.Provider>
  );
}

export function useLayoutDensity() {
  return useContext(LayoutDensityContext);
}
