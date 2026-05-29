import { describe, it, expect } from 'vitest';
import { pluginKind } from '../pluginKind';
import type { McpDefinition } from '../../types/generated';

// `transport` is a discriminated union at the type level; the tests only
// care about the two shapes the function branches on — a normal transport
// object vs the `"ApiOnly"` string sentinel — so we cast minimal stubs.
const stdio = { Stdio: { command: 'x', args: [] } } as unknown as McpDefinition['transport'];
const apiOnly = 'ApiOnly' as unknown as McpDefinition['transport'];

describe('pluginKind', () => {
  it('returns "cli" when the cli tag is present (wins over everything)', () => {
    expect(pluginKind({ transport: stdio, tags: ['cli'] })).toBe('cli');
  });

  it('cli tag beats api_spec AND ApiOnly transport', () => {
    expect(pluginKind({ transport: apiOnly, api_spec: {} as never, tags: ['cli'] })).toBe('cli');
  });

  it('returns "api" for the ApiOnly transport sentinel (no cli tag)', () => {
    expect(pluginKind({ transport: apiOnly })).toBe('api');
  });

  it('returns "hybrid" for an MCP transport that also carries api_spec', () => {
    expect(pluginKind({ transport: stdio, api_spec: {} as never })).toBe('hybrid');
  });

  it('returns "mcp" for a plain MCP transport with no api_spec, no cli tag', () => {
    expect(pluginKind({ transport: stdio })).toBe('mcp');
  });

  it('treats a null api_spec as no api_spec', () => {
    expect(pluginKind({ transport: stdio, api_spec: null })).toBe('mcp');
  });

  it('ignores non-cli tags', () => {
    expect(pluginKind({ transport: stdio, tags: ['featured', 'beta'] })).toBe('mcp');
  });

  it('tolerates a missing/undefined tags array', () => {
    expect(pluginKind({ transport: stdio, tags: undefined })).toBe('mcp');
  });
});
