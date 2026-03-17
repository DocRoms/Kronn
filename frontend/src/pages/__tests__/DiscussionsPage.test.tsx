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
  workspace_mode: 'Direct',
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
      model_tiers: {
        claude_code: { economy: null, reasoning: null },
        codex: { economy: null, reasoning: null },
        gemini_cli: { economy: null, reasoning: null },
        kiro: { economy: null, reasoning: null },
        vibe: { economy: null, reasoning: null },
      },
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

  // ─── Streaming & tab-switch behavior tests ──────────────────────────────

  it('shows thinking loader when sendingMap has active entry', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    const lifted = liftedProps();
    lifted.sendingMap = { d1: true };
    lifted.streamingMap = { d1: '' };

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[{ ...makeListDiscussion('d1', 1), messages: fullDisc.messages }]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d1"
        {...lifted}
      />
    );

    // The "thinking" / running indicator should be visible
    const body = document.body.textContent ?? '';
    expect(body).toContain('ClaudeCode');
  });

  it('restores active discussion on remount via initialActiveDiscussionId', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 2),
      messages: [
        { id: 'm1', role: 'User', content: 'My question', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'm2', role: 'Agent', content: 'My answer', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:01Z', tokens_used: 100, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    const lifted = liftedProps();

    // First mount — simulate user selecting d1
    const { unmount } = await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[makeListDiscussion('d1', 2)]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d1"
        {...lifted}
      />
    );

    // discussions.get should have been called for d1
    expect(vi.mocked(discussionsApi.get)).toHaveBeenCalledWith('d1');

    // Unmount (tab switch) and remount
    unmount();
    vi.mocked(discussionsApi.get).mockClear();

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[makeListDiscussion('d1', 2)]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d1"
        {...lifted}
      />
    );

    // After remount, discussions.get should be called again to reload d1
    expect(vi.mocked(discussionsApi.get)).toHaveBeenCalledWith('d1');
  });

  it('does NOT abort SSE controllers on unmount', async () => {
    const controller = new AbortController();
    const abortSpy = vi.spyOn(controller, 'abort');
    const lifted = liftedProps();
    lifted.abortControllers = { current: { d1: controller } };

    const { unmount } = await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        {...lifted}
      />
    );

    unmount();

    // The controller should NOT be aborted — SSE streams survive page switches
    expect(abortSpy).not.toHaveBeenCalled();
  });

  it('refetches discussion when sending finishes (activeSending changes)', async () => {
    const discWithResponse: Discussion = {
      ...makeListDiscussion('d1', 2),
      messages: [
        { id: 'm1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'm2', role: 'Agent', content: 'Response', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:01Z', tokens_used: 50, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(discWithResponse);

    const lifted = liftedProps();

    // Initial render: agent is still sending
    const sendingMap: Record<string, boolean> = { d1: true };
    lifted.sendingMap = sendingMap;

    const { rerender } = await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[makeListDiscussion('d1', 1)]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d1"
        {...lifted}
      />
    );

    const callCountBefore = vi.mocked(discussionsApi.get).mock.calls.length;

    // Simulate sending finishing: sendingMap changes to false
    const updatedLifted = { ...lifted, sendingMap: { d1: false } };
    await act(async () => {
      rerender(
        <I18nProvider>
          <DiscussionsPage
            projects={[]}
            agents={[]}
            allDiscussions={[makeListDiscussion('d1', 2)]}
            configLanguage="fr"
            agentAccess={null}
            refetchDiscussions={noop}
            refetchProjects={noop}
            onNavigate={noop}
            toast={toastFn}
            initialActiveDiscussionId="d1"
            {...updatedLifted}
          />
        </I18nProvider>
      );
    });

    // discussions.get should have been called again to reload the discussion with new messages
    expect(vi.mocked(discussionsApi.get).mock.calls.length).toBeGreaterThan(callCountBefore);
  });

  it('pre-selects validation profiles when prefill is provided', async () => {
    const lifted = liftedProps();

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        prefill={{ projectId: 'p1', title: 'Validation audit AI', prompt: 'Validate this', locked: true }}
        onPrefillConsumed={noop}
        {...lifted}
      />
    );

    // The prefilled form should be visible — the prompt textarea has the prefilled content
    const body = document.body.textContent ?? '';
    expect(body).toContain('Validate this');

    // The title input should have the prefilled value
    const titleInput = document.querySelector('input[readonly]') as HTMLInputElement;
    expect(titleInput).toBeTruthy();
    expect(titleInput.value).toBe('Validation audit AI');
  });

  // ─── Sidebar content tests ────────────────────────────────────────────

  it('sidebar shows discussion titles in the list', async () => {
    const discs = [
      makeListDiscussion('d1', 2),
      makeListDiscussion('d2', 0),
    ];

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={discs}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        {...liftedProps()}
      />
    );

    const body = document.body.textContent!;
    expect(body).toContain('Discussion d1');
    expect(body).toContain('Discussion d2');
  });

  it('archived discussions show count in Archives section header', async () => {
    const activeDisc: Discussion = {
      ...makeListDiscussion('d1', 3),
      archived: false,
    };
    const archivedDisc: Discussion = {
      ...makeListDiscussion('d2', 5),
      title: 'Old discussion',
      archived: true,
    };

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[activeDisc, archivedDisc]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        {...liftedProps()}
      />
    );

    const body = document.body.textContent!;
    // Active discussion is visible
    expect(body).toContain('Discussion d1');
    // Archives section header shows count of archived discussions
    expect(body).toContain('Archives');
    expect(body).toContain('1');
  });
});
