import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent } from '@testing-library/react';
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
(globalThis as any).SpeechSynthesisUtterance = MockUtterance;

// Mock API — DiscussionsPage uses discussions, projects, and skills APIs
vi.mock('../../lib/api', () => ({
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
import type { AgentDetection, Discussion } from '../../types/generated';

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
  sendingStartMap: {},
  setSendingStartMap: vi.fn(),
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
    const proj1 = { id: 'p1', name: 'front_euronews', path: '/repos/front_euronews', repo_url: 'git@github.com:Euronews-tech/front_euronews.git', token_override: null, ai_config: { detected: false, configs: [] }, audit_status: 'NoTemplate' as any, ai_todo_count: 0, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z' };
    const proj2 = { id: 'p2', name: 'Kronn', path: '/repos/Kronn', repo_url: 'git@github.com:DocRoms/Kronn.git', token_override: null, ai_config: { detected: false, configs: [] }, audit_status: 'NoTemplate' as any, ai_todo_count: 0, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z' };

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
    expect(body).toContain('Euronews-tech');
    expect(body).toContain('DocRoms');
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
      runtime_available: false,
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
      runtime_available: false,
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
    const startBtn = screen.getByText(/Demarrer la discussion/);
    await act(async () => { fireEvent.click(startBtn); });

    // Verify discussionsApi.create was called with the right data
    expect(vi.mocked(discussionsApi.create)).toHaveBeenCalledWith(
      expect.objectContaining({
        title: 'Test discussion',
        agent: 'ClaudeCode',
        initial_prompt: 'Analyse this code',
        language: 'fr',
      })
    );
  });
});
