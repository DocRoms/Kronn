import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, act, cleanup } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock API — DiscussionsPage uses discussions, projects, and skills APIs
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
  skills: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
}));

import { DiscussionsPage } from '../DiscussionsPage';
import type { AgentsConfig } from '../../types/generated';

const noop = () => {};
const toastFn = vi.fn() as any;

afterEach(cleanup);

const wrap = async (ui: React.ReactElement) => {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(<I18nProvider>{ui}</I18nProvider>);
  });
  return result!;
};

// Shared lifted props (mimic Dashboard)
const liftedProps = () => ({
  sendingMap: {},
  setSendingMap: vi.fn(),
  streamingMap: {},
  setStreamingMap: vi.fn(),
  abortControllers: { current: {} } as React.MutableRefObject<Record<string, AbortController>>,
  cleanupStream: vi.fn(),
  markDiscussionSeen: vi.fn(),
  onActiveDiscussionChange: vi.fn(),
  lastSeenMsgCount: {},
});

describe('DiscussionsPage', () => {
  it('renders without crashing with minimal props', async () => {
    await wrap(
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
        {...liftedProps()}
      />
    );
    expect(document.body.textContent).toBeDefined();
  });

  it('renders with agentAccess provided', async () => {
    const agentAccess: AgentsConfig = {
      claude_code: { path: null, installed: true, version: null, full_access: true },
      codex: { path: null, installed: false, version: null, full_access: false },
      gemini_cli: { path: null, installed: false, version: null, full_access: false },
      kiro: { path: null, installed: false, version: null, full_access: false },
      vibe: { path: null, installed: false, version: null, full_access: false },
    };
    await wrap(
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
        {...liftedProps()}
      />
    );
    expect(document.body.textContent).toBeDefined();
  });

  it('renders with prefill prop', async () => {
    await wrap(
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
        {...liftedProps()}
      />
    );
    expect(document.body.textContent).toBeDefined();
  });
});
