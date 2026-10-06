import { describe, expect, it } from 'vitest';
import { effectiveTestEndpoint, isTestableEndpoint } from '../testEndpoint';

const ep = (method: string, path: string) => ({ method, path, description: '' });

// Same cases as `models::tests::probe_endpoint_*` on the backend: the form
// must show the endpoint the probe really calls.
describe('effectiveTestEndpoint', () => {
  it('prefers an identity GET over earlier ones', () => {
    expect(effectiveTestEndpoint([ep('POST', '/search'), ep('GET', '/users'), ep('GET', '/users/me/')], null))
      .toBe('/users/me/');
    expect(effectiveTestEndpoint([ep('GET', '/pages'), ep('get', '/v2/WhoAmI')], null)).toBe('/v2/WhoAmI');
  });

  it('falls back to the first testable GET', () => {
    expect(effectiveTestEndpoint(
      [ep('POST', '/me'), ep('GET', '/items/{id}'), ep('GET', '/items'), ep('GET', '/tags')],
      null,
    )).toBe('/items');
  });

  it('honours a valid selection and ignores a stale or unsafe one', () => {
    const endpoints = [ep('GET', '/users'), ep('GET', '/users/me'), ep('DELETE', '/cache')];
    expect(effectiveTestEndpoint(endpoints, ' /users ')).toBe('/users');
    expect(effectiveTestEndpoint(endpoints, '/cache')).toBe('/users/me');
    expect(effectiveTestEndpoint(endpoints, '/gone')).toBe('/users/me');
  });

  it('returns null without a testable GET, ignoring blank rows', () => {
    expect(effectiveTestEndpoint([], null)).toBeNull();
    expect(effectiveTestEndpoint([ep('GET', '  '), ep('POST', '/me'), ep('GET', '/users/{id}')], '/users/{id}'))
      .toBeNull();
  });
});

// Same table as `probe_endpoint_recognises_documented_identity_paths`.
describe('documented identity paths', () => {
  it.each([
    '/user', '/v1/users/me', '/1/members/me', '/v1.0/me', '/rest/api/3/myself', '/v0/meta/whoami',
    '/api/v10/users/@me', '/api/v2/users/me.json', '/oauth2/v3/userinfo', '/client/v4/user/tokens/verify',
    '/api/v1/validate', '/v2/account', '/scim/v2/Me',
  ])('%s wins over a plain list', path => {
    expect(effectiveTestEndpoint([ep('GET', '/list'), ep('GET', path)], null)).toBe(path);
  });

  it.each(['/3.0/ping', '/health', '/v1/models'])('%s never wins: it may be public', path => {
    expect(effectiveTestEndpoint([ep('GET', '/list'), ep('GET', path)], null)).toBe('/list');
  });
});

describe('isTestableEndpoint', () => {
  it('accepts only a GET without path parameters', () => {
    expect(isTestableEndpoint(ep(' get ', '/me'))).toBe(true);
    expect(isTestableEndpoint(ep('GET', '/users/{id}'))).toBe(false);
    expect(isTestableEndpoint(ep('PATCH', '/me'))).toBe(false);
  });
});
