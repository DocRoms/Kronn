import { Navigate, useLocation } from 'react-router';
import { DEFAULT_PAGE, PAGE_PATHS } from '../lib/routes';

/**
 * Bare and unknown addresses land on the default page. The hash survives the
 * redirect: `#…` deep links are resolved by the app shell, whatever the path.
 */
export function HomeRedirect() {
  const { search, hash } = useLocation();
  return <Navigate to={{ pathname: PAGE_PATHS[DEFAULT_PAGE], search, hash }} replace />;
}
