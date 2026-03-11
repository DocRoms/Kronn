import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock API — DiscussionsPage uses discussions and projects APIs
vi.mock('../../lib/api', () => ({
  discussions: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    create: vi.fn(),
    delete: vi.fn(),
    update: vi.fn(),
    sendMessage: vi.fn(),
    run: vi.fn(),
    stop: vi.fn(),
    _streamSSE: vi.fn(),
  },
  projects: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    scan: vi.fn(),
    create: vi.fn(),
    delete: vi.fn(),
  },
}));

import { DiscussionsPage } from '../DiscussionsPage';
import type { AgentsConfig } from '../../types/generated';

const noop = () => {};
const toastFn = vi.fn() as any;

const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

describe('DiscussionsPage', () => {
  it('renders without crashing with minimal props', () => {
    wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[]}
        configLanguage={null}
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
      />
    );
    expect(document.body.textContent).toBeDefined();
  });

  it('renders with agentAccess provided', () => {
    const agentAccess: AgentsConfig = {
      claude_code: { path: null, installed: true, version: null, full_access: true },
      codex: { path: null, installed: false, version: null, full_access: false },
      gemini_cli: { path: null, installed: false, version: null, full_access: false },
      kiro: { path: null, installed: false, version: null, full_access: false },
      vibe: { path: null, installed: false, version: null, full_access: false },
    };
    wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[]}
        configLanguage="en"
        agentAccess={agentAccess}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
      />
    );
    expect(document.body.textContent).toBeDefined();
  });

  it('renders with prefill prop', () => {
    wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[]}
        configLanguage={null}
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        prefill={{ projectId: 'p1', title: 'Test', prompt: 'Hello' }}
        onPrefillConsumed={noop}
      />
    );
    expect(document.body.textContent).toBeDefined();
  });
});
