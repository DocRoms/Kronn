import type { RouteObject } from 'react-router';
import { STANDALONE_PATHS } from '../lib/routes';
import { dashboardRoutes } from './dashboardRoutes';
import { FullscreenShell } from './FullscreenShell';
import {
  DashboardLayout, StandaloneDiscussionsMosaicRoute, StandaloneLivePageRoute, StandalonePagesMosaicRoute,
} from './lazyRoutes';

/**
 * What the app root renders in its outlet once setup is complete: the
 * dashboard shell around its pages, or a view that takes the whole window.
 * A pure route table.
 */
export const appRoutes: RouteObject[] = [
  {
    element: <FullscreenShell><DashboardLayout /></FullscreenShell>,
    children: dashboardRoutes,
  },
  {
    path: `${STANDALONE_PATHS.page}/:pageId`,
    element: <FullscreenShell><StandaloneLivePageRoute /></FullscreenShell>,
  },
  {
    path: STANDALONE_PATHS.pagesMosaic,
    element: <FullscreenShell><StandalonePagesMosaicRoute /></FullscreenShell>,
  },
  {
    path: STANDALONE_PATHS.discussionsMosaic,
    element: <FullscreenShell><StandaloneDiscussionsMosaicRoute /></FullscreenShell>,
  },
];
