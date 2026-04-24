// Tests for apiCallAuth — pure logic that drives the read-only "Auth
// — gérée par Kronn" panel above the query editor. The right slot
// shape per ApiAuthKind variant is the contract every plugin spec
// relies on; if a future refactor accidentally drops a variant, the
// wizard would silently render an empty Auth panel and the user would
// have no signal that their token is plumbed through.

import { describe, it, expect } from 'vitest';
import {
  authSlots,
  authSlotsForServer,
  managedQueryNames,
  managedHeaderNames,
  stripManagedQuery,
  stripManagedHeaders,
} from '../apiCallAuth';
import type { ApiSpec } from '../../../types/generated';

const mkSpec = (auth: ApiSpec['auth']): ApiSpec => ({
  base_url: 'https://api.example.com',
  auth,
  endpoints: [],
  docs_url: null,
  config_keys: [],
});

describe('authSlots — per-variant slot shape', () => {
  it('ApiKeyQuery → one query slot', () => {
    const slots = authSlots(mkSpec({ ApiKeyQuery: { param_name: 'apikey', env_key: 'KEY' } }));
    expect(slots).toEqual([{ kind: 'query', name: 'apikey', envKey: 'KEY' }]);
  });

  it('ApiKeyHeader → one header slot', () => {
    const slots = authSlots(mkSpec({ ApiKeyHeader: { header_name: 'X-API-Key', env_key: 'KEY' } }));
    expect(slots).toEqual([{ kind: 'header', name: 'X-API-Key', envKey: 'KEY' }]);
  });

  it('Bearer → one Authorization header slot', () => {
    const slots = authSlots(mkSpec({ Bearer: { env_key: 'GH_TOKEN' } }));
    expect(slots).toEqual([{ kind: 'header', name: 'Authorization', envKey: 'GH_TOKEN' }]);
  });

  it('Basic → one Authorization header slot referencing the user+password env pair', () => {
    // Jira Cloud shape — the wizard renders this as a single
    // `Authorization ••••••••` row but the synthesised env-key label
    // (`JIRA_USERNAME+JIRA_API_TOKEN`) tells the user BOTH halves come
    // from the encrypted plugin config, never editable per-step.
    const slots = authSlots(mkSpec({
      Basic: { user_env: 'JIRA_USERNAME', password_env: 'JIRA_API_TOKEN' },
    }));
    expect(slots).toEqual([{
      kind: 'header',
      name: 'Authorization',
      envKey: 'JIRA_USERNAME+JIRA_API_TOKEN',
    }]);
  });

  it('OAuth2 → one Authorization header slot referencing the client id', () => {
    const slots = authSlots(mkSpec({
      OAuth2ClientCredentials: {
        token_url: 'https://x/oauth/token',
        client_id_env: 'CID',
        client_secret_env: 'SECRET',
        scope: 'read',
      },
    }));
    expect(slots[0].kind).toBe('header');
    expect(slots[0].name).toBe('Authorization');
    expect(slots[0].envKey).toContain('CID');
  });

  it('None → empty list', () => {
    expect(authSlots(mkSpec('None'))).toEqual([]);
  });

  it('null spec → empty list (graceful degradation when spec missing)', () => {
    expect(authSlots(null)).toEqual([]);
  });
});

describe('authSlotsForServer + managed* helpers', () => {
  it('managedQueryNames returns the param_name for ApiKeyQuery', () => {
    const server = { id: 's', name: 'x', description: '', transport: 'ApiOnly' as const, source: 'Registry' as const,
      api_spec: mkSpec({ ApiKeyQuery: { param_name: 'apikey', env_key: 'K' } }) };
    expect(managedQueryNames(server).has('apikey')).toBe(true);
  });

  it('managedHeaderNames lowercases (Authorization vs authorization)', () => {
    const server = { id: 's', name: 'x', description: '', transport: 'ApiOnly' as const, source: 'Registry' as const,
      api_spec: mkSpec({ Bearer: { env_key: 'T' } }) };
    expect(managedHeaderNames(server).has('authorization')).toBe(true);
  });

  it('null server → empty managed sets', () => {
    expect(authSlotsForServer(null)).toEqual([]);
    expect(managedQueryNames(null).size).toBe(0);
  });
});

describe('stripManagedQuery / stripManagedHeaders', () => {
  it('drops auth-managed query keys from a suggestion', () => {
    const out = stripManagedQuery(
      { host: 'fr.euronews.com', apikey: 'VOTRE_KEY' },
      new Set(['apikey']),
    );
    expect(out).toEqual({ host: 'fr.euronews.com' });
  });

  it('returns null when stripping leaves no entry (cleanup)', () => {
    const out = stripManagedQuery({ apikey: 'X' }, new Set(['apikey']));
    expect(out).toBeNull();
  });

  it('case-insensitive header strip (Authorization / authorization)', () => {
    const out = stripManagedHeaders(
      { 'X-Trace': 'yes', authorization: 'Bearer evil' },
      new Set(['authorization']),
    );
    expect(out).toEqual({ 'X-Trace': 'yes' });
  });
});
