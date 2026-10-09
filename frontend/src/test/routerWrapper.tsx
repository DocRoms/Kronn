import type { ReactElement } from 'react';
import { afterEach, beforeAll } from 'vitest';
import { createBrowserRouter } from 'react-router';
import { RouterProvider } from 'react-router/dom';
import { dashboardRoutes } from '../routes/dashboardRoutes';
import { PRELOADED_ROUTES } from '../routes/lazyRoutes';

/**
 * Test harness for anything that renders inside the dashboard outlet.
 *
 * A real browser router, as in production: the dashboard reads the address
 * from `window.location` too, so both views must agree. Assert navigation on
 * `window.location.pathname`.
 *
 * Importing this module registers two hooks in the importing spec:
 * - route chunks are fetched once up front, so a page renders in the same
 *   commit as the shell instead of after a Suspense round-trip;
 * - every router is disposed after each test and the address reset to `/`.
 */
type TestRouter = ReturnType<typeof createBrowserRouter>;

const liveRouters: TestRouter[] = [];

beforeAll(async () => {
  await Promise.all(PRELOADED_ROUTES.map(route => route.preload()));
});

afterEach(() => {
  for (const router of liveRouters.splice(0)) router.dispose();
  window.history.replaceState(null, '', '/');
});

/**
 * `shell` as the root route of the real dashboard route table, opened at
 * `initialPath`. `historyState` is what the entry holds, as a reload keeps it.
 */
export function withDashboardRoutes(shell: ReactElement, initialPath = '/', historyState: unknown = null): ReactElement {
  window.history.replaceState(historyState, '', initialPath);
  const router = createBrowserRouter([{ path: '/', element: shell, children: dashboardRoutes }]);
  liveRouters.push(router);
  // As in `main.tsx`: synchronous router state updates.
  return <RouterProvider router={router} useTransitions={false} />;
}
