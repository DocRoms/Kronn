import { createBrowserRouter } from 'react-router';
import { App } from './App';
import { appRoutes } from './routes/appRoutes';
import { RootRouteError } from './routes/RootRouteError';

/**
 * `App` is the root route: it gates on setup, then renders the dashboard
 * shell or a whole-window view, whichever the address names.
 *
 * A factory rather than a module-level router: creating one starts listening
 * to the history, which must not happen before the app decides to boot here.
 */
export function createAppRouter() {
  return createBrowserRouter([
    { path: '/', Component: App, ErrorBoundary: RootRouteError, children: appRoutes },
  ]);
}
