// Helpers for the ApiCall step card to know which query / header keys are
// already wired by the plugin's auth config — and therefore must NOT be
// editable by the user, NOT be suggested by the AI helper, and NOT be
// duplicated when an agent does suggest them anyway.
//
// Source of truth: the `ApiSpec.auth` field on the selected `McpServer`.
// Backend `core::oauth2_cache` + `api_call_executor::resolve_auth` inject
// these at request build time using the env value the user entered when
// they configured the plugin (Settings → APIs).

import type { ApiAuthKind, ApiSpec, McpServer } from '../../types/generated';

/** Describes a single auth-managed slot (one query param OR one header). */
export interface AuthSlot {
  kind: 'query' | 'header';
  name: string;
  /** Env var read at runtime — surfaced to the user as a hint
   *  ("clé `CHARTBEAT_KEY` configurée dans Kronn"). */
  envKey: string;
}

/** Inspect the plugin's auth scheme and return the list of slots that the
 *  backend will fill at request time. Empty array when auth is `None`. */
export function authSlots(spec: ApiSpec | null | undefined): AuthSlot[] {
  if (!spec) return [];
  const auth: ApiAuthKind = spec.auth;
  if (auth === 'None') return [];
  if ('ApiKeyQuery' in auth) {
    return [{ kind: 'query', name: auth.ApiKeyQuery.param_name, envKey: auth.ApiKeyQuery.env_key }];
  }
  if ('ApiKeyHeader' in auth) {
    return [{ kind: 'header', name: auth.ApiKeyHeader.header_name, envKey: auth.ApiKeyHeader.env_key }];
  }
  if ('Bearer' in auth) {
    return [{ kind: 'header', name: 'Authorization', envKey: auth.Bearer.env_key }];
  }
  if ('Basic' in auth) {
    // HTTP Basic = Authorization: Basic <base64(user:password)>. The
    // wire only carries one Authorization header, but two env keys are
    // involved. We expose them as one slot keyed on Authorization with
    // a synthesised env-key label so the user understands BOTH the
    // username + token come from their plugin config (not editable in
    // the step). The +token suffix tells the wizard "this is a Basic
    // auth — display the pair pedagogically".
    return [{
      kind: 'header',
      name: 'Authorization',
      envKey: `${auth.Basic.user_env}+${auth.Basic.password_env}`,
    }];
  }
  if ('OAuth2ClientCredentials' in auth) {
    return [{ kind: 'header', name: 'Authorization', envKey: `${auth.OAuth2ClientCredentials.client_id_env}+secret` }];
  }
  return [];
}

/** Convenience wrapper. */
export function authSlotsForServer(server: McpServer | null): AuthSlot[] {
  return authSlots(server?.api_spec ?? null);
}

/** Return query-param names whose values are auto-injected and therefore
 *  must NOT be editable / suggestible. */
export function managedQueryNames(server: McpServer | null): Set<string> {
  return new Set(authSlotsForServer(server).filter(s => s.kind === 'query').map(s => s.name));
}

/** Return header names whose values are auto-injected. Comparison is done
 *  case-insensitively at the call site (`Authorization` vs `authorization`). */
export function managedHeaderNames(server: McpServer | null): Set<string> {
  return new Set(
    authSlotsForServer(server).filter(s => s.kind === 'header').map(s => s.name.toLowerCase()),
  );
}

/** Strip any auth-managed key from a record. Used both as a defensive guard
 *  in `applyToStep` (silent drop of `apikey: 'VOTRE_API_KEY'`-style hallucinated
 *  suggestions) and to clean up legacy steps that were pre-AI-helper. */
export function stripManagedQuery(
  query: Record<string, string> | null | undefined,
  managed: Set<string>,
): Record<string, string> | null {
  if (!query) return null;
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(query)) {
    if (!managed.has(k)) out[k] = v;
  }
  return Object.keys(out).length > 0 ? out : null;
}

/** Same for headers, case-insensitive. */
export function stripManagedHeaders(
  headers: Record<string, string> | null | undefined,
  managed: Set<string>,
): Record<string, string> | null {
  if (!headers) return null;
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(headers)) {
    if (!managed.has(k.toLowerCase())) out[k] = v;
  }
  return Object.keys(out).length > 0 ? out : null;
}
