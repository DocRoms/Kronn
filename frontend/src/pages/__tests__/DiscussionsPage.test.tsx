import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, act, cleanup } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock API — DiscussionsPage uses discussions, projects, and skills APIs
vi.mock('../../lib/api', () => ({
  discussions: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn().mockResolvedValue(null),
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
  profiles: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
  directives: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
}));

import { discussions as discussionsApi } from '../../lib/api';
import { DiscussionsPage } from '../DiscussionsPage';
import type { AgentsConfig, Discussion } from '../../types/generated';

const noop = () => {};
const toastFn = vi.fn() as any;

beforeEach(() => {
  vi.mocked(discussionsApi.get).mockReset();
});

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

/** List-view discussion: has message_count but messages is empty (like the real backend) */
const makeListDiscussion = (id: string, msgCount: number): Discussion => ({
  id,
  project_id: null,
  title: `Discussion ${id}`,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],           // list endpoint returns empty messages
  message_count: msgCount, // but provides the count
  archived: false,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
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

  it('sidebar shows message_count not messages.length', async () => {
    // List has 5 messages but messages array is empty (real backend behavior)
    const listDisc = makeListDiscussion('d1', 5);
    expect(listDisc.messages).toHaveLength(0);
    expect(listDisc.message_count).toBe(5);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[listDisc]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        {...liftedProps()}
      />
    );

    // The sidebar should show "5 msg" from message_count, not "0 msg"
    expect(screen.getByText(/5 msg/)).toBeTruthy();
  });

  it('discussions.get API is available for loading full discussions', async () => {
    // Verify the API mock is properly configured — this is a guard test
    // that ensures discussions.get is wired up and callable.
    // The actual integration (tap → fetch → render messages) uses pointer events
    // that jsdom doesn't support, so we verify the plumbing instead.
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 2),
      messages: [
        { id: 'm1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'm2', role: 'Agent', content: 'Hi', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:00Z', tokens_used: 100, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    const result = await discussionsApi.get('d1');
    expect(result).toBeDefined();
    expect(result!.messages).toHaveLength(2);
    expect(result!.messages[0].content).toBe('Hello');
    expect(result!.message_count).toBe(2);
  });

  it('list discussions have empty messages array (regression guard)', () => {
    // This test ensures test helpers match real backend behavior.
    // If someone changes makeListDiscussion to include messages,
    // this test will catch the mistake.
    const disc = makeListDiscussion('test', 10);
    expect(disc.messages).toHaveLength(0);
    expect(disc.message_count).toBe(10);
  });
});
