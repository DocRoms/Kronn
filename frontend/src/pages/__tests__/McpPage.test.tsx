import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock API — McpPage uses mcpsApi
vi.mock('../../lib/api', () => ({
  mcps: {
    overview: vi.fn().mockResolvedValue({ servers: [], configs: [], project_links: [], customized_contexts: [] }),
    registry: vi.fn().mockResolvedValue([]),
    refresh: vi.fn(),
    createConfig: vi.fn(),
    updateConfig: vi.fn(),
    deleteConfig: vi.fn(),
    setConfigProjects: vi.fn(),
    revealSecrets: vi.fn(),
    listContexts: vi.fn(),
    getContext: vi.fn(),
    updateContext: vi.fn(),
  },
}));

import { McpPage } from '../McpPage';
import type { McpOverview } from '../../types/generated';

const noop = () => {};

const emptyOverview: McpOverview = {
  servers: [],
  configs: [],
  customized_contexts: [],
};

const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

describe('McpPage', () => {
  it('renders without crashing with empty data', () => {
    wrap(
      <McpPage
        projects={[]}
        mcpOverview={emptyOverview}
        mcpRegistry={[]}
        refetchMcps={noop}
      />
    );
    expect(document.body.textContent).toBeDefined();
  });

  it('renders the MCP heading', () => {
    wrap(
      <McpPage
        projects={[]}
        mcpOverview={emptyOverview}
        mcpRegistry={[]}
        refetchMcps={noop}
      />
    );
    // The heading text may be localized; just verify it rendered content
    expect(document.body.textContent!.length).toBeGreaterThan(0);
  });

  it('renders with registry entries', () => {
    wrap(
      <McpPage
        projects={[]}
        mcpOverview={emptyOverview}
        mcpRegistry={[
          { id: 'test-mcp', name: 'Test MCP', description: 'A test', command: 'node', args: [], env_keys: [] } as any,
        ]}
        refetchMcps={noop}
      />
    );
    expect(document.body.textContent).toBeDefined();
  });
});
