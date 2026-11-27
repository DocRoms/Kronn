import { lazyPage } from '../lib/lazyPage';

// One chunk per route, so the dashboard shell stays small: a page is fetched
// on its first visit, or in the background once the browser is idle after
// start-up so that a first tab switch does not wait on the network.
export const ProjectsRoute = lazyPage(() => import('./ProjectsRoute').then(m => m.ProjectsRoute));
export const DiscussionsRoute = lazyPage(() => import('./DiscussionsRoute').then(m => m.DiscussionsRoute));
export const PlanningRoute = lazyPage(() => import('./PlanningRoute').then(m => m.PlanningRoute));
export const WorkflowsRoute = lazyPage(() => import('./WorkflowsRoute').then(m => m.WorkflowsRoute));
export const PagesRoute = lazyPage(() => import('./PagesRoute').then(m => m.PagesRoute));
export const PluginsRoute = lazyPage(() => import('./PluginsRoute').then(m => m.PluginsRoute));
export const SettingsRoute = lazyPage(() => import('./SettingsRoute').then(m => m.SettingsRoute));

// KT-1987 — the design system gallery, a sub-page of Configuration: its own
// chunk, fetched on its first visit only (never preloaded), so neither
// Configuration nor the start-up idle preload pulls the gallery in.
export const DesignSystemRoute = lazyPage(() => import('./DesignSystemRoute').then(m => m.DesignSystemRoute));

export const PRELOADED_ROUTES = [
  ProjectsRoute, DiscussionsRoute, PlanningRoute, WorkflowsRoute, PagesRoute, PluginsRoute, SettingsRoute,
];

// The dashboard shell and the whole-window views: one chunk each, fetched
// when the address asks for them.
export const DashboardLayout = lazyPage(() => import('./DashboardLayout').then(m => m.DashboardLayout));
export const StandaloneLivePageRoute = lazyPage(() => import('./StandaloneLivePageRoute').then(m => m.StandaloneLivePageRoute));
export const StandalonePagesMosaicRoute = lazyPage(() => import('./StandalonePagesMosaicRoute').then(m => m.StandalonePagesMosaicRoute));
export const StandaloneDiscussionsMosaicRoute = lazyPage(() => import('./StandaloneDiscussionsMosaicRoute').then(m => m.StandaloneDiscussionsMosaicRoute));
