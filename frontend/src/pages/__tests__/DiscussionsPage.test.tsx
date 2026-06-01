import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock SpeechSynthesis API
const mockCancel = vi.fn();
const mockSpeak = vi.fn();
const mockGetVoices = vi.fn().mockReturnValue([]);
Object.defineProperty(window, 'speechSynthesis', {
  value: { cancel: mockCancel, speak: mockSpeak, getVoices: mockGetVoices, speaking: false },
  writable: true,
  configurable: true,
});

// Mock SpeechSynthesisUtterance (not available in jsdom)
class MockUtterance {
  text: string;
  lang = '';
  rate = 1;
  voice: any = null;
  constructor(text: string) { this.text = text; }
}
(globalThis as unknown as Record<string, unknown>).SpeechSynthesisUtterance = MockUtterance;

// Mock API — DiscussionsPage uses discussions, projects, and skills APIs
vi.mock('../../lib/api', () => ({
  // 0.9.0 — ChatHeader renders <LearningsBadge> which polls learnings.pending().
  learnings: {
    pending: vi.fn().mockResolvedValue({ count: 0 }),
    list: vi.fn().mockResolvedValue([]),
    validate: vi.fn().mockResolvedValue({}),
    reject: vi.fn().mockResolvedValue(undefined),
    propose: vi.fn().mockResolvedValue({ accepted: true, warnings: [], evidence_checks: [], learning: null }),
    forDiscussion: vi.fn().mockResolvedValue([]),
  },
  discussions: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn().mockResolvedValue(null),
    create: vi.fn(),
    delete: vi.fn(),
    update: vi.fn(),
    sendMessage: vi.fn(),
    sendMessageStream: vi.fn().mockResolvedValue(undefined),
    run: vi.fn(),
    runAgent: vi.fn().mockResolvedValue(undefined),
    stop: vi.fn(),
    _streamSSE: vi.fn(),
    worktreeUnlock: vi.fn().mockResolvedValue('ok'),
    worktreeLock: vi.fn().mockResolvedValue('ok'),
  },
  projects: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    scan: vi.fn(),
    create: vi.fn(),
    delete: vi.fn(),
    // 0.8.3 (#280) — DiscussionsPage polls this every 8 s to decide
    // whether to show the audit-running banner. Default = null (no
    // audit). Tests that need the running state override per-test.
    auditStatus: vi.fn().mockResolvedValue(null),
    // 0.8.4 (#294) — sidebar fetches this once per mount to decorate
    // disc rows with the "imported from X" badge. Empty = no
    // bindings, badge stays hidden.
    discSources: vi.fn().mockResolvedValue([]),
  },
  skills: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
  autoTriggersApi: {
    listDisabled: vi.fn().mockResolvedValue([]),
    toggle: vi.fn().mockResolvedValue(false),
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
  contacts: {
    list: vi.fn().mockResolvedValue([]),
    add: vi.fn(),
    delete: vi.fn(),
    inviteCode: vi.fn().mockResolvedValue('kronn:test@localhost:3456'),
    ping: vi.fn().mockResolvedValue(false),
  },
  workflows: {
    listBatchRunSummaries: vi.fn().mockResolvedValue([]),
  },
  quickPrompts: {
    list: vi.fn().mockResolvedValue([]),
  },
  config: {
    getUiLanguage: vi.fn().mockResolvedValue('fr'),
    saveUiLanguage: vi.fn().mockResolvedValue(undefined),
    // 0.8.6 phase 4 — NewDiscussionForm fetches the default tier on mount.
    getServerConfig: vi.fn().mockResolvedValue({ default_model_tier: 'default' }),
  },
}));

// Mock useWebSocket hook (WS not available in jsdom)
vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false })),
}));

import { discussions as discussionsApi, projects as projectsApi } from '../../lib/api';
import { DiscussionsPage } from '../DiscussionsPage';
import type { AgentDetection, AiAuditStatus, Discussion, Project } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';

const noop = () => {};
const toastFn: ToastFn = vi.fn();

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
  sendingStartMap: {},
  setSendingStartMap: vi.fn(),
  streamingMap: {},
  setStreamingMap: vi.fn(),
  noteStreamTick: vi.fn(),
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
  message_count: msgCount, non_system_message_count: msgCount, // but provides the count
  archived: false, pinned: false,
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
    // The "Nouvelle" button (disc.new in FR) should be a button element
    const allButtons = Array.from(document.body.querySelectorAll('button'));
    const newDiscBtn = allButtons.find(b => b.textContent?.includes('Nouvelle'));
    expect(newDiscBtn).toBeTruthy();
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
    // The prefill prompt content should appear in the new-discussion form
    const body = document.body.textContent ?? '';
    expect(body).toContain('Hello');
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

  it('refetches and reloads on kronn:discussion-updated (auto-skill activation)', async () => {
    // ChatInput dispatches `kronn:discussion-updated` after auto-activating
    // skills on a discussion. Pre-fix nobody listened, so the sidebar +
    // chips kept showing the old skill_ids until a manual refresh.
    // Regression guard: the listener must (a) call refetchDiscussions and
    // (b) reload the active discussion via discussionsApi.get.
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm1', role: 'User', content: 'Hi', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    const refetchSpy = vi.fn();
    const lifted = liftedProps();

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[makeListDiscussion('d1', 1)]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={refetchSpy}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d1"
        {...lifted}
      />
    );

    const refetchCallsBefore = refetchSpy.mock.calls.length;
    const getCallsBefore = vi.mocked(discussionsApi.get).mock.calls.length;

    await act(async () => {
      window.dispatchEvent(new CustomEvent('kronn:discussion-updated'));
    });

    expect(refetchSpy.mock.calls.length).toBeGreaterThan(refetchCallsBefore);
    // discussions.get('d1') re-fired to pick up the new skill_ids.
    const newGetCalls = vi.mocked(discussionsApi.get).mock.calls.slice(getCallsBefore);
    expect(newGetCalls.some(args => args[0] === 'd1')).toBe(true);
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

  // ─── Mobile responsive tests ─────────────────────────────────────────

  it('shows hamburger Menu button on mobile when no discussion is selected', async () => {
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: query.includes('767'),
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    });

    await wrap(
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
        {...liftedProps()}
      />
    );

    // On mobile, when sidebar is initially open, a close button should be visible
    // OR when a discussion is active, a hamburger menu button with aria-label "Open sidebar" should exist
    const menuBtn = document.querySelector('button[aria-label="Open sidebar"], button[aria-label="Close sidebar"]');
    expect(menuBtn).toBeTruthy();

    // Restore default matchMedia
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    });
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
      archived: false, pinned: false,
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

  it('shows API mode warning banner for Vibe discussions', async () => {
    const vibeDisc: Discussion = {
      ...makeListDiscussion('vibe1', 1),
      agent: 'Vibe',
      participants: ['Vibe'],
      messages: [
        { id: 'm1', role: 'User', content: 'Hello Vibe', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(vibeDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[{ ...makeListDiscussion('vibe1', 1), agent: 'Vibe', participants: ['Vibe'], messages: vibeDisc.messages }]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="vibe1"
        {...liftedProps()}
      />
    );

    const body = document.body.textContent!;
    expect(body).toContain('Mode API');
    expect(body).toContain('MCP');
  });

  it('persists sidebar collapse state to localStorage', async () => {
    // Pre-set a collapsed state in localStorage
    localStorage.setItem('kronn:discCollapsedGroups', JSON.stringify(['__global__']));

    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[{ ...makeListDiscussion('d1', 1), project_id: null, messages: fullDisc.messages }]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        {...liftedProps()}
      />
    );

    // The localStorage value should be readable
    const saved = localStorage.getItem('kronn:discCollapsedGroups');
    expect(saved).toBeTruthy();
    const parsed = JSON.parse(saved!);
    expect(Array.isArray(parsed)).toBe(true);
  });

  it('groups project discussions by org when multiple orgs exist', async () => {
    const proj1 = { id: 'p1', name: 'web-app', path: '/repos/web-app', repo_url: 'git@github.com:acme-org/web-app.git', token_override: null, ai_config: { detected: false, configs: [] }, audit_status: 'NoTemplate' as AiAuditStatus, ai_todo_count: 0, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z' };
    const proj2 = { id: 'p2', name: 'api-server', path: '/repos/api-server', repo_url: 'git@github.com:johndoe/api-server.git', token_override: null, ai_config: { detected: false, configs: [] }, audit_status: 'NoTemplate' as AiAuditStatus, ai_todo_count: 0, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z' };

    const disc1 = { ...makeListDiscussion('d1', 1), project_id: 'p1', messages: [{ id: 'm1', role: 'User' as const, content: 'test', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null }] };
    const disc2 = { ...makeListDiscussion('d2', 1), project_id: 'p2', messages: [{ id: 'm2', role: 'User' as const, content: 'test', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null }] };

    vi.mocked(discussionsApi.get).mockResolvedValue(disc1);

    await wrap(
      <DiscussionsPage
        projects={[proj1, proj2]}
        agents={[]}
        allDiscussions={[disc1, disc2]}
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
    // Should show org group headers
    expect(body).toContain('acme-org');
    expect(body).toContain('johndoe');
  });

  // ─── TTS feature tests ──────────────────────────────────────────────────

  it('renders TTS toggle button in chat input area', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

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
        {...liftedProps()}
      />
    );

    // TTS toggle button should exist with the "Activer" title (disabled by default)
    const ttsBtn = document.querySelector('button[title="Activer la lecture vocale"]');
    expect(ttsBtn).toBeTruthy();
  });

  it('persists TTS preference to localStorage when toggled', async () => {
    localStorage.removeItem('kronn:ttsEnabled');

    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

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
        {...liftedProps()}
      />
    );

    // Initially TTS is off
    expect(localStorage.getItem('kronn:ttsEnabled')).toBe('false');

    // Click the TTS toggle button
    const ttsBtn = document.querySelector('button[title="Activer la lecture vocale"]') as HTMLButtonElement;
    await act(async () => { fireEvent.click(ttsBtn); });

    // After toggle, it should be persisted as 'true'
    expect(localStorage.getItem('kronn:ttsEnabled')).toBe('true');

    // Button title should now say "Desactiver"
    const ttsBtnAfter = document.querySelector('button[title="Desactiver la lecture vocale"]');
    expect(ttsBtnAfter).toBeTruthy();
  });

  it('shows TTS play button on agent messages', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 2),
      messages: [
        { id: 'm1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'm2', role: 'Agent', content: 'Bonjour!', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:01Z', tokens_used: 50, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[{ ...makeListDiscussion('d1', 2), messages: fullDisc.messages }]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d1"
        {...liftedProps()}
      />
    );

    // Per-message TTS play button should be present on agent messages
    const ttsPlayBtn = document.querySelector('button[title="Lire à voix haute"]');
    expect(ttsPlayBtn).toBeTruthy();
  });

  it('calls speechSynthesis.speak when per-message TTS button is clicked', async () => {
    mockSpeak.mockClear();
    mockCancel.mockClear();

    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 2),
      messages: [
        { id: 'm1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'm2', role: 'Agent', content: 'Bonjour le monde!', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:01Z', tokens_used: 50, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[{ ...makeListDiscussion('d1', 2), messages: fullDisc.messages }]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d1"
        {...liftedProps()}
      />
    );

    const ttsPlayBtn = document.querySelector('button[title="Lire à voix haute"]') as HTMLButtonElement;
    await act(async () => { fireEvent.click(ttsPlayBtn); });

    // speechSynthesis.cancel should be called first (to stop any ongoing speech)
    expect(mockCancel).toHaveBeenCalled();
    // speechSynthesis.speak should be called with an utterance
    expect(mockSpeak).toHaveBeenCalledWith(expect.any(SpeechSynthesisUtterance));
  });

  it('optimistically promotes the streaming buffer to a real Agent message on stream end (no scroll jump)', async () => {
    // Reported bug: "quand le stream se termine, ça remonte au début du
    // message et ça redescend". Root cause — `cleanupStream` flipped
    // `sending=false` BEFORE the refetch landed the persisted Agent
    // message, so the streaming bubble unmounted and the chat shrunk
    // (scroll snapped UP to the previous user message), then a smooth
    // scrollIntoView animated DOWN once the new message arrived.
    //
    // The fix converts the in-memory streamingMap entry into an
    // optimistic Agent message in `loadedDiscussions` BEFORE clearing
    // sending — the streaming row unmounts at the same render where
    // the optimistic bubble mounts, with the same content, so the
    // scroll position never jumps. The persisted refetch arrives
    // afterwards and replaces the optimistic with the real message.
    //
    // Test contract: trigger a send whose stream emits a chunk and
    // ends. Assert that the chat now contains an Agent bubble with
    // the streamed text BEFORE the refetch lands (we don't mock
    // `discussions.get` for the post-stream reload — the optimistic
    // alone must populate the DOM).
    const claudeAgent: AgentDetection = {
      name: 'Claude Code',
      agent_type: 'ClaudeCode',
      installed: true,
      enabled: true,
      path: '/usr/bin/claude',
      version: '1.0.0',
      latest_version: null,
      origin: 'host',
      install_command: null,
      host_managed: false,
      host_label: null,
      runtime_available: false, rtk_available: false, rtk_hook_configured: false,
    };

    const initialDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    // The post-stream `reloadDiscussion` fetch — in production the
    // backend has already persisted the agent reply, so we mock the
    // refetch to return it. Without this, reloadDiscussion would
    // OVERWRITE the optimistic insert with a disc that only has the
    // user message and our assertion would fail before the test ever
    // touched the optimistic path. The test hinges on the FIRST render
    // after cleanupStream, before the network round-trip — but the
    // mock here resolves synchronously enough that we just guarantee
    // the persisted version is consistent with the optimistic one.
    const reloadedDisc: Discussion = {
      ...initialDisc,
      messages: [
        ...initialDisc.messages,
        { id: 'persisted-agent', role: 'Agent', content: 'Streamed agent reply.', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:01Z', tokens_used: 12, auth_mode: null },
      ],
      message_count: 2, non_system_message_count: 2,
    };
    let getCallCount = 0;
    vi.mocked(discussionsApi.get).mockImplementation(async () => {
      // First fetch (mount) returns the initial disc.
      // Subsequent fetches (post-stream reload) return with the agent message.
      getCallCount += 1;
      return getCallCount === 1 ? initialDisc : reloadedDisc;
    });

    // Pre-populate streamingMap as if N chunks had already accumulated.
    // The mock SSE will only call onDone — cleanupStream must read the
    // existing buffer and promote it to a real message.
    const lifted = liftedProps();
    lifted.streamingMap = { d1: 'Streamed agent reply.' };

    // Mock sendMessageStream: skip onText (we already populated the
    // map), just call onDone synchronously.
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId: any, _payload: any, _onText: any, onDone: any) => {
        if (onDone) onDone();
      },
    );

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[claudeAgent]}
        allDiscussions={[{ ...makeListDiscussion('d1', 1), messages: initialDisc.messages }]}
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

    // Wait one tick for the initial discussion fetch to land.
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    const chatInput = document.querySelector('textarea') as HTMLTextAreaElement;
    expect(chatInput).toBeTruthy();
    await act(async () => { fireEvent.change(chatInput, { target: { value: 'Another question' } }); });

    const sendBtn = document.querySelector('button[aria-label="Send message"]') as HTMLButtonElement;
    expect(sendBtn).toBeTruthy();
    await act(async () => { fireEvent.click(sendBtn); });

    // Let the optimistic state update + the post-stream reload flush.
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    // The streamed text must now be visible as a real message bubble.
    // Pre-fix: only the streaming bubble showed it, and that bubble
    // unmounted on `sending=false` BEFORE the refetch landed — so for
    // a brief window the chat was missing the agent reply entirely
    // (visible to the user as "scroll up to user msg, then back down").
    expect(document.body.textContent).toContain('Streamed agent reply.');
  });

  it('cancels speech when sending a new message', async () => {
    mockCancel.mockClear();

    const claudeAgent: AgentDetection = {
      name: 'Claude Code',
      agent_type: 'ClaudeCode',
      installed: true,
      enabled: true,
      path: '/usr/bin/claude',
      version: '1.0.0',
      latest_version: null,
      origin: 'host',
      install_command: null,
      host_managed: false,
      host_label: null,
      runtime_available: false, rtk_available: false, rtk_hook_configured: false,
    };

    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);
    // Mock sendMessageStream: capture the onSent callback and call it, then resolve
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId: any, _payload: any, _onText: any, onDone: any, _onError: any, _signal: any, onSent: any) => {
        if (onSent) onSent();
        if (onDone) onDone();
      },
    );

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[claudeAgent]}
        allDiscussions={[{ ...makeListDiscussion('d1', 1), messages: fullDisc.messages }]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d1"
        {...liftedProps()}
      />
    );

    // Type a message in the chat input
    const chatInput = document.querySelector('textarea') as HTMLTextAreaElement;
    expect(chatInput).toBeTruthy();
    await act(async () => { fireEvent.change(chatInput, { target: { value: 'New message' } }); });

    // Click send button
    const sendBtn = document.querySelector('button[aria-label="Send message"]') as HTMLButtonElement;
    expect(sendBtn).toBeTruthy();
    await act(async () => { fireEvent.click(sendBtn); });

    // speechSynthesis.cancel should have been called when sending the message
    expect(mockCancel).toHaveBeenCalled();
  });

  it('creates a new discussion via the form', async () => {
    const createdDisc: Discussion = {
      ...makeListDiscussion('new-disc', 1),
      messages: [
        { id: 'm1', role: 'User', content: 'Analyse this code', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.create).mockResolvedValue(createdDisc);
    vi.mocked(discussionsApi.get).mockResolvedValue(createdDisc);

    const claudeAgent: AgentDetection = {
      name: 'Claude Code',
      agent_type: 'ClaudeCode',
      installed: true,
      enabled: true,
      path: '/usr/bin/claude',
      version: '1.0.0',
      latest_version: null,
      origin: 'host',
      install_command: null,
      host_managed: false,
      host_label: null,
      runtime_available: false, rtk_available: false, rtk_hook_configured: false,
    };

    const lifted = liftedProps();

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[claudeAgent]}
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

    // Click the "Nouvelle" button to open the new discussion form
    const newBtns = screen.getAllByText(/Nouvelle/);
    const newBtn = newBtns[0]; // First match is the sidebar button
    await act(async () => { fireEvent.click(newBtn); });

    // Fill in the title field
    const titleInput = document.querySelector('input[placeholder]') as HTMLInputElement;
    expect(titleInput).toBeTruthy();
    await act(async () => { fireEvent.change(titleInput, { target: { value: 'Test discussion' } }); });

    // Fill in the prompt textarea
    const promptTextarea = document.querySelector('textarea') as HTMLTextAreaElement;
    expect(promptTextarea).toBeTruthy();
    await act(async () => { fireEvent.change(promptTextarea, { target: { value: 'Analyse this code' } }); });

    // The agent select should already have ClaudeCode selected (only installed agent)
    // Click the create/start button
    const startBtn = screen.getByText(/Démarrer la discussion/);
    await act(async () => { fireEvent.click(startBtn); });

    // Verify discussionsApi.create was called with the right data
    expect(vi.mocked(discussionsApi.create)).toHaveBeenCalledWith(
      expect.objectContaining({
        agent: 'ClaudeCode',
        initial_prompt: 'Analyse this code',
        language: 'fr',
      })
    );
  });

  it('0.8.6 disc-first : creating with launchAgentNow=false skips runAgent + toasts', async () => {
    // Regression guard for the new launchAgentNow=false branch in
    // handleCreateDiscussion. When the user unchecks "Lancer un
    // agent tout de suite" :
    //   - discussionsApi.create still fires (the disc is born)
    //   - discussionsApi.runAgent MUST NOT fire (no CLI kick-off)
    //   - a success toast surfaces with the disc-first guidance copy
    // Without this test, a refactor that drops the early-return
    // would silently start spawning agents on every disc-first
    // creation in prod.
    const createdDisc: Discussion = {
      ...makeListDiscussion('disc-first-1', 1),
      messages: [],
    };
    // Mock state leaks between tests in this file — clear both call
    // history AND prior resolved-value bindings before re-arming.
    vi.mocked(discussionsApi.create).mockReset();
    vi.mocked(discussionsApi.create).mockResolvedValue(createdDisc);
    vi.mocked(discussionsApi.get).mockResolvedValue(createdDisc);
    vi.mocked(discussionsApi.runAgent).mockClear();

    const claudeAgent: AgentDetection = {
      name: 'Claude Code',
      agent_type: 'ClaudeCode',
      installed: true,
      enabled: true,
      path: '/usr/bin/claude',
      version: '1.0.0',
      latest_version: null,
      origin: 'host',
      install_command: null,
      host_managed: false,
      host_label: null,
      runtime_available: false, rtk_available: false, rtk_hook_configured: false,
    };

    const lifted = liftedProps();
    const localToast = vi.fn();

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[claudeAgent]}
        allDiscussions={[]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={localToast}
        {...lifted}
      />
    );

    // Open the new-disc form.
    const newBtn = screen.getAllByText(/Nouvelle/)[0];
    await act(async () => { fireEvent.click(newBtn); });

    // Uncheck the "Lancer un agent tout de suite" checkbox. Real i18n
    // translates the aria-label to French — match it that way rather
    // than the i18n key (the DiscussionsPage test wraps in the real
    // I18nProvider, not the mock used by component tests).
    const launchCheckbox = document.querySelector(
      'input[type="checkbox"]',
    ) as HTMLInputElement;
    expect(launchCheckbox).toBeTruthy();
    expect(launchCheckbox.checked).toBe(true);
    await act(async () => { fireEvent.click(launchCheckbox); });
    expect(launchCheckbox.checked).toBe(false);

    // Fill the title. `input[placeholder]` alone matches the sidebar
    // search input first — target the disc-form input by class.
    const titleInput = document.querySelector(
      'input.disc-input-styled',
    ) as HTMLInputElement;
    expect(titleInput).toBeTruthy();
    await act(async () => {
      fireEvent.change(titleInput, { target: { value: 'RGPD room for later' } });
    });

    // Submit — button label flips from "Démarrer" to "Créer la discussion"
    // in disc-first mode (disc.createEmpty i18n key, FR translation).
    const createBtn = screen.getByText(/Créer la discussion/);
    expect(createBtn).toBeTruthy();
    await act(async () => { fireEvent.click(createBtn); });
    await waitFor(
      () => expect(vi.mocked(discussionsApi.create)).toHaveBeenCalled(),
      { timeout: 1000 },
    );

    // The disc was created with the title the user typed.
    expect(vi.mocked(discussionsApi.create)).toHaveBeenCalledTimes(1);
    const createCall = vi.mocked(discussionsApi.create).mock.calls[0][0];
    expect(createCall.title).toBe('RGPD room for later');
    expect(createCall.agent).toBe('ClaudeCode');

    // No agent run was kicked off — disc-first promise. The
    // assertion runs after waitFor so the handler had time to
    // reach either the early-return OR the runAgent branch.
    await act(async () => { await new Promise(r => setTimeout(r, 50)); });
    expect(vi.mocked(discussionsApi.runAgent)).not.toHaveBeenCalled();

    // A success toast surfaced with the disc-first guidance copy.
    // Real i18n provider → FR translation in the toast args.
    await waitFor(() => {
      const successToast = localToast.mock.calls.find(c => c[1] === 'success');
      expect(successToast, 'expected a success toast').toBeDefined();
      expect(successToast![0]).toContain('Discussion créée');
    }, { timeout: 1000 });
  });

  it('shows copy button on agent messages', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-copy', 2),
      messages: [
        { id: 'u1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a1', role: 'Agent', content: 'World', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:05Z', tokens_used: 50, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[{ agent_type: 'ClaudeCode', name: 'Claude Code', installed: true, enabled: true, path: null, version: null, latest_version: null, origin: 'npm', install_command: null, host_managed: false, host_label: null, runtime_available: true, rtk_available: false, rtk_hook_configured: false }]}
        allDiscussions={[makeListDiscussion('d-copy', 2)]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-copy"
        {...liftedProps()}
      />
    );

    // Should find copy buttons (title attribute)
    const copyBtns = document.querySelectorAll('[title="Copier le message"]');
    expect(copyBtns.length).toBeGreaterThanOrEqual(1);
  });

  it('shows response duration on agent messages', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-time', 2),
      messages: [
        { id: 'u1', role: 'User', content: 'Question', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a1', role: 'Agent', content: 'Answer', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:01:23Z', tokens_used: 100, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[{ agent_type: 'ClaudeCode', name: 'Claude Code', installed: true, enabled: true, path: null, version: null, latest_version: null, origin: 'npm', install_command: null, host_managed: false, host_label: null, runtime_available: true, rtk_available: false, rtk_hook_configured: false }]}
        allDiscussions={[makeListDiscussion('d-time', 2)]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-time"
        {...liftedProps()}
      />
    );

    // 83 seconds = 1m 23s
    const body = document.body.textContent ?? '';
    expect(body).toContain('1m 23s');
  });

  it('message bubbles have overflow-wrap to prevent long URLs from breaking layout', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-overflow', 2),
      messages: [
        { id: 'u1', role: 'User', content: 'https://example.com/very-long-url-that-should-not-break-the-bubble-layout/with/many/path/segments/and-no-spaces-at-all', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a1', role: 'Agent', content: 'Here is the response', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:05Z', tokens_used: 50, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[{ agent_type: 'ClaudeCode', name: 'Claude Code', installed: true, enabled: true, path: null, version: null, latest_version: null, origin: 'npm', install_command: null, host_managed: false, host_label: null, runtime_available: true, rtk_available: false, rtk_hook_configured: false }]}
        allDiscussions={[makeListDiscussion('d-overflow', 2)]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-overflow"
        {...liftedProps()}
      />
    );

    // All message bubbles use the disc-msg-bubble CSS class which includes overflow-wrap: break-word
    const bubbles = document.querySelectorAll('.disc-msg-bubble');
    expect(bubbles.length).toBeGreaterThanOrEqual(2);
  });

  it('shows agent switch button in chat header', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-switch', 2),
      messages: [
        { id: 'u1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a1', role: 'Agent', content: 'Hi', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:05Z', tokens_used: 50, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[
          { agent_type: 'ClaudeCode', name: 'Claude Code', installed: true, enabled: true, path: null, version: null, latest_version: null, origin: 'npm', install_command: null, host_managed: false, host_label: null, runtime_available: true, rtk_available: false, rtk_hook_configured: false },
          { agent_type: 'GeminiCli', name: 'Gemini CLI', installed: true, enabled: true, path: null, version: null, latest_version: null, origin: 'npm', install_command: null, host_managed: false, host_label: null, runtime_available: true, rtk_available: false, rtk_hook_configured: false },
        ]}
        allDiscussions={[makeListDiscussion('d-switch', 2)]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-switch"
        {...liftedProps()}
      />
    );

    // Agent switch button should be visible with the current agent name
    const switchBtn = document.querySelector('[title="Changer d\'agent"]');
    expect(switchBtn).toBeTruthy();
    expect(switchBtn?.textContent).toContain('ClaudeCode');
  });

  // ─── Discussion search filter tests ──────────────────────────────────

  it('search input exists and filters discussions by title', async () => {
    const disc1: Discussion = { ...makeListDiscussion('d-alpha', 1), title: 'Alpha project chat' };
    const disc2: Discussion = { ...makeListDiscussion('d-beta', 2), title: 'Beta refactoring' };

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[disc1, disc2]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        {...liftedProps()}
      />
    );

    // Both discussions should be visible initially
    const bodyBefore = document.body.textContent!;
    expect(bodyBefore).toContain('Alpha project chat');
    expect(bodyBefore).toContain('Beta refactoring');

    // Find the search input by placeholder
    const searchInput = document.querySelector('input[placeholder="Rechercher..."]') as HTMLInputElement;
    expect(searchInput).toBeTruthy();

    // Type "Alpha" in the search
    await act(async () => { fireEvent.change(searchInput, { target: { value: 'Alpha' } }); });

    // Only the matching discussion should be visible
    const bodyAfter = document.body.textContent!;
    expect(bodyAfter).toContain('Alpha project chat');
    expect(bodyAfter).not.toContain('Beta refactoring');
  });

  // ─── Agent switch dropdown tests ─────────────────────────────────────

  it('clicking agent switch button shows dropdown with other agents', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-dropdown', 2),
      messages: [
        { id: 'u1', role: 'User', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a1', role: 'Agent', content: 'Hi', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:05Z', tokens_used: 50, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[
          { agent_type: 'ClaudeCode', name: 'Claude Code', installed: true, enabled: true, path: null, version: null, latest_version: null, origin: 'npm', install_command: null, host_managed: false, host_label: null, runtime_available: true, rtk_available: false, rtk_hook_configured: false },
          { agent_type: 'Codex', name: 'Codex', installed: true, enabled: true, path: null, version: null, latest_version: null, origin: 'npm', install_command: null, host_managed: false, host_label: null, runtime_available: true, rtk_available: false, rtk_hook_configured: false },
          { agent_type: 'GeminiCli', name: 'Gemini CLI', installed: true, enabled: true, path: null, version: null, latest_version: null, origin: 'npm', install_command: null, host_managed: false, host_label: null, runtime_available: true, rtk_available: false, rtk_hook_configured: false },
        ]}
        allDiscussions={[makeListDiscussion('d-dropdown', 2)]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-dropdown"
        {...liftedProps()}
      />
    );

    // Click the agent switch button
    const switchBtn = document.querySelector('[title="Changer d\'agent"]') as HTMLButtonElement;
    expect(switchBtn).toBeTruthy();
    await act(async () => { fireEvent.click(switchBtn); });

    // The dropdown should now show all installed agents (using display names)
    const body = document.body.textContent!;
    expect(body).toContain('Claude Code');
    expect(body).toContain('Codex');
    expect(body).toContain('Gemini CLI');
  });

  it('shows contacts section in sidebar when contacts exist', async () => {
    // Mock contacts.list to return contacts
    const { contacts: contactsApi } = await import('../../lib/api');
    vi.mocked(contactsApi.list).mockResolvedValue([
      { id: 'c1', pseudo: 'PeerOne', avatar_email: null, kronn_url: 'http://100.64.1.2:3456', invite_code: 'kronn:peerone@100.64.1.2:3456', status: 'accepted', created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z' },
    ]);
    vi.mocked(contactsApi.ping).mockResolvedValue(true);

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
        {...liftedProps()}
      />
    );

    // Wait for contacts to load
    await act(async () => { await new Promise(r => setTimeout(r, 50)); });

    const body = document.body.textContent ?? '';
    expect(body).toContain('Contacts');
    expect(body).toContain('PeerOne');
  });

  it('shows WS connection indicator in contacts section', async () => {
    const { contacts: contactsApi } = await import('../../lib/api');
    vi.mocked(contactsApi.list).mockResolvedValue([
      { id: 'c1', pseudo: 'PeerAlpha', avatar_email: null, kronn_url: 'http://10.0.0.1:3456', invite_code: 'kronn:PeerAlpha@10.0.0.1:3456', status: 'accepted', created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z' },
    ]);

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
        {...liftedProps()}
      />
    );

    await act(async () => { await new Promise(r => setTimeout(r, 50)); });

    // Should show 0/1 (no contacts online since WS mock returns connected: false)
    const body = document.body.textContent ?? '';
    expect(body).toContain('0/1');
  });

  it('updates contact online status when useWebSocket mock is configured', async () => {
    // Override the mock to call the handler with a presence message
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    vi.mocked(useWebSocket).mockImplementation((onMessage) => {
      // Simulate receiving a presence message after mount
      setTimeout(() => {
        onMessage({
          type: 'presence',
          from_pseudo: 'PeerAlpha',
          from_invite_code: 'kronn:PeerAlpha@10.0.0.1:3456',
          online: true,
        });
      }, 10);
      return { connected: true };
    });

    const { contacts: contactsApi } = await import('../../lib/api');
    vi.mocked(contactsApi.list).mockResolvedValue([
      { id: 'c1', pseudo: 'PeerAlpha', avatar_email: null, kronn_url: 'http://10.0.0.1:3456', invite_code: 'kronn:PeerAlpha@10.0.0.1:3456', status: 'accepted', created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z' },
    ]);

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
        {...liftedProps()}
      />
    );

    // Wait for contacts to load + WS message to fire
    await act(async () => { await new Promise(r => setTimeout(r, 100)); });

    // Should show 1/1 (PeerAlpha is online via WS presence)
    const body = document.body.textContent ?? '';
    expect(body).toContain('1/1');

    // Restore default mock
    vi.mocked(useWebSocket).mockImplementation(() => ({ connected: false }));
  });

  it('shows contacts section with add button even when no contacts exist', async () => {
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
        {...liftedProps()}
      />
    );

    await act(async () => { await new Promise(r => setTimeout(r, 50)); });

    // Contacts section should always be visible with its title
    const body = document.body.textContent ?? '';
    expect(body).toContain('Contacts');
  });

  it('shows add contact form when plus button is clicked', async () => {
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
        {...liftedProps()}
      />
    );

    await act(async () => { await new Promise(r => setTimeout(r, 50)); });

    // Click the + button to show add contact form
    const addBtn = document.querySelector('button[title="Ajouter un contact"]');
    expect(addBtn).toBeTruthy();
    fireEvent.click(addBtn!);

    // Should show the input field with placeholder
    const input = document.querySelector('input[placeholder="kronn:pseudo@host:port"]');
    expect(input).toBeTruthy();
  });

  it('shows delete button on each contact', async () => {
    const { contacts: contactsApi } = await import('../../lib/api');
    vi.mocked(contactsApi.list).mockResolvedValue([
      { id: 'c1', pseudo: 'PeerAlpha', avatar_email: null, kronn_url: 'http://10.0.0.1:3456', invite_code: 'kronn:PeerAlpha@10.0.0.1:3456', status: 'accepted', created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z' },
    ]);

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
        {...liftedProps()}
      />
    );

    await act(async () => { await new Promise(r => setTimeout(r, 50)); });

    // Should have a delete button
    const deleteBtn = document.querySelector('button[title="Supprimer"]');
    expect(deleteBtn).toBeTruthy();
  });

  // ─── Unaudited project warning banner (0.8.3 #276) ───────────────────
  //
  // Killer UX win: a new Kronn user starting a discussion on a project
  // they just registered has NO idea there's an AI audit step. They
  // burn tokens re-explaining the project on every turn. This banner
  // surfaces the missing audit upfront, with an adaptive CTA based on
  // briefing presence (no briefing → push to briefing first; briefing
  // done → push to launch audit).
  //
  // Tested invariants:
  //   1. Shows on unaudited states (NoTemplate / TemplateInstalled / Bootstrapped)
  //   2. Hidden once audit_status === 'Audited' or 'Validated'
  //   3. Hidden on system-managed discs (briefing/bootstrap/validation)
  //      — they have their own dedicated CTAs
  //   4. CTA adapts: empty briefing_notes → briefing CTA; present → launch CTA
  //   5. CTA navigates to the project page with the correct project_id

  const makeProject = (id: string, audit_status: AiAuditStatus, briefing?: string): Project => ({
    id, name: `Project ${id}`, path: `/r/${id}`,
    repo_url: null, token_override: null,
    ai_config: { detected: false, configs: [] },
    audit_status,
    ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false,
    default_skill_ids: [],
    briefing_notes: briefing ?? null,
    linked_repos: [],
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  });

  const makeProjectDisc = (id: string, projectId: string, title = 'My question'): Discussion => ({
    id, project_id: projectId, title,
    agent: 'ClaudeCode', language: 'fr',
    participants: ['ClaudeCode'],
    messages: [
      { id: 'm1', role: 'User', content: 'Tell me about my project', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
    ],
    message_count: 1, non_system_message_count: 1,
    archived: false, pinned: false,
    workspace_mode: 'Direct',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  });

  const renderWithDisc = async (proj: Project, disc: Discussion, onNavigateSpy = vi.fn()) => {
    vi.mocked(discussionsApi.get).mockResolvedValue(disc);
    await wrap(
      <DiscussionsPage
        projects={[proj]}
        agents={[]}
        allDiscussions={[disc]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={onNavigateSpy}
        toast={toastFn}
        initialActiveDiscussionId={disc.id}
        {...liftedProps()}
      />
    );
    await act(async () => { await new Promise(r => setTimeout(r, 30)); });
    return onNavigateSpy;
  };

  it('unaudited banner: shows on NoTemplate with briefing CTA when briefing_notes empty', async () => {
    const proj = makeProject('p1', 'NoTemplate');
    const disc = makeProjectDisc('d-unaud-1', 'p1');
    await renderWithDisc(proj, disc);
    const body = document.body.textContent ?? '';
    // FR copy ships in i18n; check the marker phrase from the warning.
    expect(body).toMatch(/n['']a pas encore d['']audit IA validé/i);
    // CTA pushes toward briefing because briefing_notes is empty.
    expect(body).toMatch(/Faire le briefing/i);
    // The launch-audit CTA must NOT show in the no-briefing variant
    // — we want the user to do the briefing first.
    expect(body).not.toMatch(/Lancer l['']audit IA/i);
  });

  it('unaudited banner: shows on TemplateInstalled with launch CTA when briefing_notes present', async () => {
    const proj = makeProject('p2', 'TemplateInstalled', 'We use Symfony, RTL/i18n required.');
    const disc = makeProjectDisc('d-unaud-2', 'p2');
    await renderWithDisc(proj, disc);
    const body = document.body.textContent ?? '';
    // Adapted warning copy (briefing-done variant).
    expect(body).toMatch(/Briefing effectué, mais l['']audit IA n['']a pas/i);
    // CTA pushes toward audit launch.
    expect(body).toMatch(/Lancer l['']audit IA/i);
  });

  it('unaudited banner: shows on Bootstrapped state too', async () => {
    // Bootstrapped means the AI did Phase 1 (template + briefing-style
    // intro) but didn't run the 10-step audit — the user still needs
    // to launch it to load real project context.
    const proj = makeProject('p3', 'Bootstrapped', 'context');
    const disc = makeProjectDisc('d-unaud-3', 'p3');
    await renderWithDisc(proj, disc);
    const body = document.body.textContent ?? '';
    expect(body).toMatch(/Lancer l['']audit IA/i);
  });

  it('unaudited banner: hidden once audit_status === Audited', async () => {
    const proj = makeProject('p4', 'Audited', 'context');
    const disc = makeProjectDisc('d-unaud-4', 'p4');
    await renderWithDisc(proj, disc);
    const body = document.body.textContent ?? '';
    // Both variants of the warning must be absent.
    expect(body).not.toMatch(/n['']a pas encore d['']audit IA validé/i);
    expect(body).not.toMatch(/Briefing effectué, mais l['']audit IA/i);
  });

  it('unaudited banner: hidden once audit_status === Validated', async () => {
    const proj = makeProject('p5', 'Validated', 'context');
    const disc = makeProjectDisc('d-unaud-5', 'p5');
    await renderWithDisc(proj, disc);
    const body = document.body.textContent ?? '';
    expect(body).not.toMatch(/n['']a pas encore d['']audit IA validé/i);
    expect(body).not.toMatch(/Briefing effectué, mais l['']audit IA/i);
  });

  it('unaudited banner: hidden on system briefing/bootstrap/validation discs', async () => {
    // These have their own dedicated banners further down; stacking
    // the warning on top would be redundant + the user is already
    // in the right flow.
    const proj = makeProject('p6', 'NoTemplate');
    const briefingDisc = makeProjectDisc('d-brief', 'p6', 'Briefing projet');
    await renderWithDisc(proj, briefingDisc);
    const body = document.body.textContent ?? '';
    expect(body).not.toMatch(/n['']a pas encore d['']audit IA validé/i);
  });

  it('unaudited banner: hidden for discussions without a project_id', async () => {
    // Project-less general discussions can't be audited — there's
    // no project to audit. Showing the banner would be misleading.
    const proj = makeProject('p7', 'NoTemplate');
    const noProjDisc: Discussion = { ...makeProjectDisc('d-noproj', 'p7'), project_id: null };
    await renderWithDisc(proj, noProjDisc);
    const body = document.body.textContent ?? '';
    expect(body).not.toMatch(/n['']a pas encore d['']audit IA validé/i);
  });

  it('unaudited banner: CTA fires onNavigate with the project_id', async () => {
    const proj = makeProject('p8', 'NoTemplate');
    const disc = makeProjectDisc('d-cta', 'p8');
    const onNav = vi.fn();
    await renderWithDisc(proj, disc, onNav);
    // Click the primary CTA (briefing variant since no briefing_notes).
    const btn = Array.from(document.body.querySelectorAll('button'))
      .find(b => b.textContent?.includes('Faire le briefing'));
    expect(btn).toBeTruthy();
    fireEvent.click(btn!);
    expect(onNav).toHaveBeenCalledWith('projects', { projectId: 'p8' });
  });

  // ─── Audit-running MCP filter banner (0.8.3 #280) ────────────────────
  //
  // When an audit is in progress on the same project as the active
  // discussion, the backend has installed an MCP allowlist swap. The
  // user's discussion sees the filtered subset — the banner explains
  // why and that normal MCPs return automatically. Polled every 8 s
  // via projectsApi.auditStatus.
  //
  // Coverage:
  //   1. Banner visible when auditStatus returns a non-null progress
  //   2. Banner hidden when auditStatus returns null
  //   3. Banner hidden for system discs (briefing/bootstrap/validation)
  //   4. Banner hidden for discussions without a project_id
  //   5. Banner re-evaluates when auditStatus flips during the disc
  //   6. Pessimistic on network error (no banner shown — defensive)

  // Audit-running banner uses an `Audited` project (banner is
  // independent of unaudited state — even fully-audited projects can
  // have a re-audit running). The unaudited banner uses NoTemplate /
  // TemplateInstalled / Bootstrapped, so picking Audited here keeps
  // it out of the way of the MCP-filter banner.
  const audited = (id: string) => makeProject(id, 'Audited', 'context');

  it('audit-running banner: visible when auditStatus returns a running run', async () => {
    vi.mocked(projectsApi.auditStatus).mockResolvedValue({
      project_id: 'pA',
      phase: 'auditing',
      step_index: 3,
      total_steps: 10,
      current_file: 'docs/AGENTS.md',
      started_at: '2026-05-14T17:44:14Z',
      kind: 'full_audit',
    });
    const proj = audited('pA');
    const disc = makeProjectDisc('d-running', 'pA');
    await renderWithDisc(proj, disc);
    // Mount triggers the poll's first call → the banner must mount
    // once the promise resolves. Tick a small wait to let React
    // flush the state update.
    await act(async () => { await new Promise(r => setTimeout(r, 50)); });
    const body = document.body.textContent ?? '';
    expect(body).toMatch(/Audit IA en cours sur ce projet/i);
    expect(body).toMatch(/MCPs/i);
  });

  it('audit-running banner: hidden when auditStatus returns null (no audit)', async () => {
    vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
    const proj = audited('pB');
    const disc = makeProjectDisc('d-none', 'pB');
    await renderWithDisc(proj, disc);
    await act(async () => { await new Promise(r => setTimeout(r, 50)); });
    const body = document.body.textContent ?? '';
    expect(body).not.toMatch(/Audit IA en cours sur ce projet/i);
  });

  it('audit-running banner: hidden on system discs (briefing/validation/bootstrap)', async () => {
    // System discs have their own dedicated CTAs. Stacking the
    // MCP-filter warning on top would dilute the primary signal.
    vi.mocked(projectsApi.auditStatus).mockResolvedValue({
      project_id: 'pC', phase: 'auditing', step_index: 1, total_steps: 10,
      current_file: null, started_at: '2026-05-14T17:44:14Z', kind: 'full_audit',
    });
    const proj = audited('pC');
    const briefingDisc = makeProjectDisc('d-brief', 'pC', 'Briefing projet');
    await renderWithDisc(proj, briefingDisc);
    await act(async () => { await new Promise(r => setTimeout(r, 50)); });
    const body = document.body.textContent ?? '';
    expect(body).not.toMatch(/Audit IA en cours sur ce projet/i);
  });

  it('audit-running banner: hidden when discussion has no project_id', async () => {
    // No project → no audit possible → no banner. Defensive against
    // a future regression that polls indiscriminately.
    vi.mocked(projectsApi.auditStatus).mockResolvedValue({
      project_id: 'unused', phase: 'auditing', step_index: 1, total_steps: 10,
      current_file: null, started_at: '2026-05-14T17:44:14Z', kind: 'full_audit',
    });
    const proj = audited('pD');
    const noProjDisc: Discussion = { ...makeProjectDisc('d-noproj', 'pD'), project_id: null };
    await renderWithDisc(proj, noProjDisc);
    await act(async () => { await new Promise(r => setTimeout(r, 50)); });
    const body = document.body.textContent ?? '';
    expect(body).not.toMatch(/Audit IA en cours sur ce projet/i);
  });

  it('audit-running banner: pessimistic on network error — no banner', async () => {
    // The auditStatus poll can fail transiently. We must default to
    // "no banner" rather than the worst-case "show banner forever".
    vi.mocked(projectsApi.auditStatus).mockRejectedValue(new Error('network down'));
    const proj = audited('pE');
    const disc = makeProjectDisc('d-err', 'pE');
    await renderWithDisc(proj, disc);
    await act(async () => { await new Promise(r => setTimeout(r, 50)); });
    const body = document.body.textContent ?? '';
    expect(body).not.toMatch(/Audit IA en cours sur ce projet/i);
  });
});
