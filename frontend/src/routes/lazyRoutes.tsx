// Route-level components kept OUT of `router.tsx` on purpose: a module that
// exports a non-component (the `router` object) while also declaring
// components is not a valid fast-refresh boundary, so editing it would force
// a full reload instead of a hot update. Keeping every component here — and
// only components in the exports — lets `router.tsx` stay a pure route table.
//
// The routes stay lazy-loaded so the initial Dashboard chunk stays under
// 500 KB: each page is its own chunk, only fetched when the user first
// visits it. Dropped the Dashboard chunk from 949 KB → ~430 KB, at the cost
// of a one-time ~100 ms fetch on first tab switch.

import { lazy, Suspense } from 'react';
import { ErrorBoundary } from '../components/ErrorBoundary';

const LOADING_LABELS: Record<string, string> = {
  fr: 'Chargement…',
  en: 'Loading…',
  es: 'Cargando…',
};

function getLoadingLabel(): string {
  try {
    const stored = localStorage.getItem('kronn:ui-locale');
    if (stored && stored in LOADING_LABELS) return LOADING_LABELS[stored];
  } catch { /* ignore */ }
  return LOADING_LABELS.fr;
}

export function PageFallback() {
  return <div className="route-fallback">{getLoadingLabel()}</div>;
}

export function LazyRoute({ Component }: { Component: React.LazyExoticComponent<React.ComponentType> }) {
  return (
    <ErrorBoundary mode="zone" label="Route">
      <Suspense fallback={<PageFallback />}><Component /></Suspense>
    </ErrorBoundary>
  );
}

export const LazyProjectsRoute = lazy(() => import('./ProjectsRoute').then(m => ({ default: m.ProjectsRoute })));
export const LazyDiscussionsRoute = lazy(() => import('./DiscussionsRoute').then(m => ({ default: m.DiscussionsRoute })));
export const LazyPlanningRoute = lazy(() => import('./PlanningRoute').then(m => ({ default: m.PlanningRoute })));
export const LazyPluginsRoute = lazy(() => import('./PluginsRoute').then(m => ({ default: m.PluginsRoute })));
export const LazyWorkflowsRoute = lazy(() => import('./WorkflowsRoute').then(m => ({ default: m.WorkflowsRoute })));
export const LazySettingsRoute = lazy(() => import('./SettingsRoute').then(m => ({ default: m.SettingsRoute })));
