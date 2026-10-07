import { useRouteError } from 'react-router';
import { ErrorBoundary } from '../components/ErrorBoundary';

function Rethrow(): never {
  throw useRouteError();
}

/**
 * An error the router caught above every page zone. It is handed back to the
 * app's own boundary, so the user gets the usual error screen and its Reload
 * button instead of the router's developer page.
 */
export function RootRouteError() {
  return <ErrorBoundary><Rethrow /></ErrorBoundary>;
}
