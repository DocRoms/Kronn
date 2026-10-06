import type { ApiEndpoint } from '../../types/generated';

// Mirror of `ApiSpec::probe_endpoint` (backend/src/models/mcp.rs): the form
// shows the endpoint the "Test" button will really call.
const IDENTITY_SEGMENTS = new Set([
  'me', 'whoami', 'self', 'myself', 'user', 'userinfo', 'viewer', 'account', 'profile',
  'current', 'verify', 'validate',
]);

/** A `GET` the test can call as-is: no `{param}` left to fill. */
export function isTestableEndpoint(endpoint: ApiEndpoint): boolean {
  return endpoint.method.trim().toUpperCase() === 'GET' && !endpoint.path.includes('{');
}

/** Last path segment, lower-cased, without a leading `@` or a `.json`/`.xml`. */
function identitySegment(path: string): string {
  const segments = path.trim().replace(/\/+$/, '').split('/');
  return (segments[segments.length - 1] ?? '').replace(/^@+/, '').toLowerCase().replace(/\.(json|xml)$/, '');
}

function isIdentityEndpoint(endpoint: ApiEndpoint): boolean {
  return IDENTITY_SEGMENTS.has(identitySegment(endpoint.path));
}

/** The selected endpoint when it still qualifies, otherwise the most
 *  identity-like testable endpoint, then the first testable one. */
export function effectiveTestEndpoint(endpoints: ApiEndpoint[], selected: string | null | undefined): string | null {
  const testable = endpoints
    .map(e => ({ ...e, path: e.path.trim() }))
    .filter(e => e.path !== '' && isTestableEndpoint(e));
  const chosen = selected?.trim();
  if (chosen && testable.some(e => e.path === chosen)) return chosen;
  return (testable.find(isIdentityEndpoint) ?? testable[0])?.path ?? null;
}
