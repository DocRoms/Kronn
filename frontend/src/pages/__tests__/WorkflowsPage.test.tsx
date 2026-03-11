import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { WorkflowsPage } from '../WorkflowsPage';
import type { AgentsConfig } from '../../types/generated';

// Mock API — WorkflowsPage calls workflowsApi.list() on mount
vi.mock('../../lib/api', () => ({
  workflows: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    create: vi.fn(),
    delete: vi.fn(),
    trigger: vi.fn(),
    runs: vi.fn(),
    deleteRun: vi.fn(),
    deleteAllRuns: vi.fn(),
  },
}));

const restrictedConfig: AgentsConfig = {
  claude_code: { path: null, installed: true, version: null, full_access: false },
  codex: { path: null, installed: true, version: null, full_access: false },
  gemini_cli: { path: null, installed: true, version: null, full_access: false },
  kiro: { path: null, installed: false, version: null, full_access: false },
  vibe: { path: null, installed: false, version: null, full_access: false },
};

const fullConfig: AgentsConfig = {
  claude_code: { path: null, installed: true, version: null, full_access: true },
  codex: { path: null, installed: true, version: null, full_access: true },
  gemini_cli: { path: null, installed: true, version: null, full_access: true },
  kiro: { path: null, installed: false, version: null, full_access: true },
  vibe: { path: null, installed: false, version: null, full_access: true },
};

const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

describe('WorkflowsPage', () => {
  it('renders without agentAccess (undefined)', () => {
    wrap(<WorkflowsPage projects={[]} />);
    expect(screen.getByText('Workflows')).toBeDefined();
  });

  it('renders with restricted agentAccess without errors', () => {
    wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode', 'Codex']}
        agentAccess={restrictedConfig}
      />
    );
    expect(screen.getByText('Workflows')).toBeDefined();
  });

  it('renders with full access agentAccess without errors', () => {
    wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode']}
        agentAccess={fullConfig}
      />
    );
    expect(screen.getByText('Workflows')).toBeDefined();
  });
});
