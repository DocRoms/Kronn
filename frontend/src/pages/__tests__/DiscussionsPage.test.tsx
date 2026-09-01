import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { useRef, useState } from 'react';
import { I18nProvider } from '../../lib/I18nContext';
import { loadDraft } from '../../lib/chat-drafts';
import { clearReplyDraft, loadReplyDraft } from '../../lib/chat-reply-drafts';
import type { DiscussionMessage } from '../../types/generated';

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
  // 0.10.0 — ChatHeader renders <LearningsBadge> which polls learnings.pending().
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
    deleteMessage: vi.fn().mockResolvedValue(undefined),
    update: vi.fn(),
    nativeAgentMode: vi.fn().mockResolvedValue({ disabled: false }),
    meta: vi.fn().mockResolvedValue({ poll_policy: { max_delay_seconds: 120 } }),
    sendMessage: vi.fn(),
    sendMessageStream: vi.fn().mockResolvedValue(undefined),
    run: vi.fn(),
    runAgent: vi.fn().mockResolvedValue(undefined),
    orchestrate: vi.fn().mockResolvedValue(undefined),
    stop: vi.fn(),
    stopDispatch: vi.fn().mockResolvedValue({
      cancelled: true,
      dispatch_id: 'dispatch',
      still_awaiting: true,
    }),
    searchMessages: vi.fn().mockResolvedValue([]),
    _streamSSE: vi.fn(),
    worktreeUnlock: vi.fn().mockResolvedValue('ok'),
    worktreeLock: vi.fn().mockResolvedValue('ok'),
    dismissPartial: vi.fn().mockResolvedValue({ recovered: false }),
    listContextFiles: vi.fn().mockResolvedValue([]),
    contextFileBlob: vi.fn(),
    // 0.9.2 — the composer lists joined CLI sessions to offer their `-cli` aliases.
    participants: vi.fn().mockResolvedValue([]),
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
    validateAudit: vi.fn().mockResolvedValue('Validated'),
    // 0.8.4 (#294) — sidebar fetches this once per mount to decorate
    // disc rows with the "bound to X" badge. Empty = no bindings,
    // badge stays hidden.
    discSources: vi.fn().mockResolvedValue([]),
    // KT-74 — same for portable-import provenance.
    discImports: vi.fn().mockResolvedValue([]),
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
  externalApi: {
    list: vi.fn().mockResolvedValue([]),
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
  planning: {
    discussionPlan: vi.fn().mockResolvedValue({
      discussion_id: 'test',
      primary_objective: null,
      active: [],
      later: [],
      completed_active: 0,
      total_active: 0,
      stats: { ready: 0, blocked: 0, in_progress: 0, ideas: 0, done: 0, later: 0 },
    }),
    proposals: vi.fn().mockResolvedValue({
      proposals: [],
      pending_proposal_count: 0,
      pending_item_count: 0,
    }),
    changes: vi.fn().mockResolvedValue([]),
    list: vi.fn().mockResolvedValue({ items: [], next_cursor: null }),
    get: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
    linkDiscussion: vi.fn(),
    addBlocker: vi.fn(),
  },
  orchestration: {
    discussionLinks: vi.fn().mockResolvedValue([]),
  },
  // KT-243 — DiscussionAttachedRuns polls attached SharedRuns per discussion.
  runsApi: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
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
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));

import {
  discussions as discussionsApi,
  externalApi as externalApiConnections,
  planning as planningApi,
  projects as projectsApi,
} from '../../lib/api';
import { DiscussionsPage } from '../DiscussionsPage';
import { findRenderedTextRanges } from '../../lib/discussionMessageSearch';
import type { AgentDetection, AgentType, AgentsConfig, AiAuditStatus, ContextFile, Discussion, Project } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';

const noop = () => {};
const toastFn: ToastFn = vi.fn();

beforeEach(() => {
  vi.mocked(discussionsApi.get).mockReset();
  vi.mocked(discussionsApi.searchMessages).mockReset();
  vi.mocked(discussionsApi.searchMessages).mockResolvedValue([]);
  vi.mocked(discussionsApi.listContextFiles).mockReset();
  vi.mocked(discussionsApi.listContextFiles).mockResolvedValue([]);
  vi.mocked(discussionsApi.deleteMessage).mockReset();
  vi.mocked(discussionsApi.deleteMessage).mockResolvedValue(undefined);
  vi.mocked(projectsApi.validateAudit).mockReset();
  vi.mocked(projectsApi.validateAudit).mockResolvedValue('Validated');
  sessionStorage.clear();
  vi.mocked(planningApi.proposals).mockReset();
  vi.mocked(planningApi.proposals).mockResolvedValue({
    proposals: [],
    pending_proposal_count: 0,
    pending_item_count: 0,
  });
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

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
  queuedMap: {},
  setQueuedMap: vi.fn(),
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
  archived: false, pinned: false, pin_first_message: false,
  tier: "default" as const, summary_strategy: "Auto" as const, introspection_call_count: 0,
  workspace_mode: 'Direct',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  awaiting_agent: false,
});

describe('DiscussionsPage', () => {
  it('confirms and tombstones the selected message before refreshing the discussion', async () => {
    const message: DiscussionMessage = {
      id: 'message-delete-me',
      role: 'User',
      channel: 'main',
      content: 'Message to remove',
      agent_type: null,
      timestamp: '2026-01-01T00:00:00Z',
      tokens_used: 0,
      auth_mode: null,
    };
    const discussion = { ...makeListDiscussion('d-delete-message', 1), messages: [message] };
    vi.mocked(discussionsApi.get).mockResolvedValue(discussion);
    const confirmDelete = vi.fn(() => true);
    vi.stubGlobal('confirm', confirmDelete);
    const refetchDiscussions = vi.fn();

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[discussion]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={refetchDiscussions}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId={discussion.id}
        {...liftedProps()}
      />,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'Supprimer le message' }));

    await waitFor(() => expect(discussionsApi.deleteMessage)
      .toHaveBeenCalledWith(discussion.id, message.id));
    expect(confirmDelete).toHaveBeenCalledTimes(1);
    expect(refetchDiscussions).toHaveBeenCalled();
  });

  it('finds a rendered occurrence even when Markdown splits it across text nodes', () => {
    const root = document.createElement('div');
    root.append('foo ');
    const strong = document.createElement('strong');
    strong.textContent = 'bar';
    root.append(strong, ' baz');

    const ranges = findRenderedTextRanges(root, 'foo bar');

    expect(ranges).toHaveLength(1);
    expect(ranges[0].toString()).toBe('foo bar');
  });

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

  it('opens and closes the dedicated terminal panel from the discussion header', async () => {
    const discussion = makeListDiscussion('d-terminal', 0);
    vi.mocked(discussionsApi.get).mockResolvedValue(discussion);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[discussion]}
        configLanguage="fr"
        agentAccess={{
          claude_code: { full_access: true },
        } as unknown as AgentsConfig}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-terminal"
        {...liftedProps()}
      />,
    );

    const terminalButton = await screen.findByRole('button', { name: 'Terminal' });
    expect(terminalButton).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(terminalButton);
    expect(screen.getByRole('complementary', { name: 'Terminal' })).toBeInTheDocument();
    expect(terminalButton).toHaveAttribute('aria-expanded', 'true');

    const gitButton = screen.getByRole('button', { name: 'Fichiers' });
    fireEvent.click(gitButton);
    expect(screen.queryByRole('complementary', { name: 'Terminal' })).not.toBeInTheDocument();
    expect(gitButton).toHaveAttribute('aria-expanded', 'true');

    fireEvent.click(terminalButton);
    expect(screen.getByRole('complementary', { name: 'Terminal' })).toBeInTheDocument();
    expect(gitButton).toHaveAttribute('aria-expanded', 'false');

    fireEvent.click(terminalButton);
    expect(screen.queryByRole('complementary', { name: 'Terminal' })).not.toBeInTheDocument();
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

  it('keeps notes interleaved but lets the user hide them globally', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-notes', 2),
      messages: [
        { id: 'main-1', role: 'User', channel: 'main', content: 'Visible main turn', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'note-2', role: 'User', channel: 'note', content: 'Private timeline note', agent_type: null, timestamp: '2026-01-01T00:00:01Z', tokens_used: 0, auth_mode: null, author_pseudo: 'Romuald' },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[{ ...makeListDiscussion('d-notes', 2), messages: fullDisc.messages }]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-notes"
        {...liftedProps()}
      />,
    );

    await waitFor(() => expect(screen.getAllByText('Private timeline note').length).toBeGreaterThan(0));
    const hideNotes = screen.getByRole('button', { name: 'Masquer les notes' });
    expect(hideNotes.closest('.disc-note-tools')).not.toBeNull();
    expect(document.querySelector('.disc-messages .disc-notes-filter')).toBeNull();
    fireEvent.click(hideNotes);
    expect(screen.queryByText('Private timeline note')).not.toBeInTheDocument();
    expect(screen.getByText('Visible main turn')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Afficher les notes' }));
    expect(screen.getAllByText('Private timeline note').length).toBeGreaterThan(0);
  });

  it('passes persisted routing tiers from the discussion detail to each user message', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-routing', 1),
      messages: [
        { id: 'u-routing', role: 'User', channel: 'main', content: 'Passe en mode rapide', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue({
      ...fullDisc,
      active_agent_dispatches: [],
      message_targets: {
        'u-routing': [{ kind: 'agent', agent_type: 'ClaudeCode', tier: 'economy' }],
      },
    });

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[fullDisc]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-routing"
        {...liftedProps()}
      />,
    );

    const receipt = await screen.findByTestId('message-routing-receipt');
    expect(receipt.closest('.disc-msg-author')).not.toBeNull();
    expect(receipt).toHaveAccessibleName('Routage demandé');
    expect(receipt).not.toHaveTextContent('Routage demandé');
    expect(receipt).toHaveTextContent('@claude · ⚡ Éco');
  });

  // ─── Streaming & tab-switch behavior tests ──────────────────────────────

  it('shows thinking loader when sendingMap has active entry', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
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

  it('persists a queued follow-up immediately and reconciles its receipt once', async () => {
    const fullDisc = {
      ...makeListDiscussion('d-queued-inline', 1),
      awaiting_agent: true,
      messages: [
        {
          id: 'u-running', role: 'User' as const, channel: 'main' as const,
          content: 'Question en cours', agent_type: null,
          timestamp: '2026-08-26T10:00:00Z', tokens_used: 0, auth_mode: null,
        },
      ],
      active_agent_dispatches: [{
        id: 'job-running',
        trigger_message_id: 'u-running',
        agent_type: 'ClaudeCode' as const,
        status: 'Running',
      }],
      message_targets: {},
    };
    const completedDisc = {
      ...fullDisc,
      awaiting_agent: false,
      active_agent_dispatches: [],
      messages: [
        ...fullDisc.messages,
        {
          id: 'a-running', role: 'Agent' as const, channel: 'main' as const,
          content: 'Réponse terminée', agent_type: 'ClaudeCode' as const,
          reply_to_message_id: 'u-running', timestamp: '2026-08-26T10:00:10Z',
          tokens_used: 12, auth_mode: null,
        },
      ],
      message_count: 2,
      non_system_message_count: 2,
    };
    let currentRunFinished = false;
    let queuedRunFinished = false;
    let sentPayload: Parameters<typeof discussionsApi.sendMessageStream>[1] | undefined;
    let acceptQueued: (() => void) | undefined;
    vi.mocked(discussionsApi.get).mockImplementation(async () => {
      if (!currentRunFinished) return fullDisc;
      if (!sentPayload?.client_message_id) return completedDisc;
      const queuedUser = {
        id: sentPayload.client_message_id,
        role: 'User' as const,
        channel: 'main' as const,
        content: sentPayload.content,
        agent_type: null,
        timestamp: '2026-08-26T10:00:11Z',
        tokens_used: 0,
        auth_mode: null,
      };
      return {
        ...completedDisc,
        awaiting_agent: !queuedRunFinished,
        messages: queuedRunFinished
          ? [
              ...completedDisc.messages,
              queuedUser,
              {
                id: 'a-queued', role: 'Agent' as const, channel: 'main' as const,
                content: 'Réponse au message en queue', agent_type: 'ClaudeCode' as const,
                reply_to_message_id: queuedUser.id, timestamp: '2026-08-26T10:00:12Z',
                tokens_used: 8, auth_mode: null,
              },
            ]
          : [...completedDisc.messages, queuedUser],
        message_count: queuedRunFinished ? 4 : 3,
        non_system_message_count: queuedRunFinished ? 4 : 3,
      };
    });
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId, payload, _onText, onDone, _onError, _signal, onStart, _onLog, onAccepted) => {
        sentPayload = payload;
        onStart?.();
        acceptQueued = () => {
          onAccepted?.({
            message_id: payload.client_message_id ?? 'u-queued',
            sort_order: 3,
            duplicate: false,
          });
          onDone?.();
        };
      },
    );

    const lifted = liftedProps();
    lifted.sendingMap = { 'd-queued-inline': true };
    lifted.streamingMap = { 'd-queued-inline': 'Réponse partielle' };

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[fullDisc]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-queued-inline"
        {...lifted}
      />,
    );

    const textarea = document.querySelector('textarea') as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'Mon prochain message' } });
    fireEvent.click(screen.getByRole('button', { name: /Ajouter à la file/ }));

    await waitFor(() => expect(sentPayload?.content).toBe('Mon prochain message'));
    expect(sentPayload).toEqual(expect.objectContaining({
      defer_dispatch: true,
      client_message_id: expect.stringMatching(/^[0-9a-f-]{36}$/i),
    }));

    const transcript = document.querySelector('.disc-messages');
    const liveReply = screen.getByTestId('streaming-agent-ClaudeCode');
    const queued = screen.getByLabelText("Messages en file d'attente");
    expect(transcript).toContainElement(queued);
    expect(liveReply.compareDocumentPosition(queued) & Node.DOCUMENT_POSITION_FOLLOWING)
      .not.toBe(0);
    // The queue is the chronological tail immediately before the scroll anchor.
    expect(queued.nextElementSibling).toBe(transcript?.lastElementChild);

    expect(queued).toHaveTextContent('persistance…');

    // The durable receipt removes the local outbox entry. Replaying the same
    // client_message_id would be duplicate=true server-side, so a lost receipt
    // cannot create a second User row or dispatch.
    currentRunFinished = true;
    queuedRunFinished = true;
    await act(async () => {
      acceptQueued?.();
    });

    await waitFor(() => expect(screen.queryByLabelText("Messages en file d'attente")).toBeNull());
    await waitFor(() => expect(screen.getByText('Réponse au message en queue')).toBeInTheDocument());
    expect(screen.getAllByText('Mon prochain message')).toHaveLength(1);
  });

  it('shows only the principal placeholder for a general turn', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-plural', 1),
      agent: 'LiteLlm',
      participants: ['LiteLlm', 'Ollama'],
      awaiting_agent: true,
      messages: [
        {
          id: 'u-plural', role: 'User', channel: 'main',
          content: 'Vous devriez connaître vos points forts et faibles.',
          agent_type: null, timestamp: '2026-08-10T09:53:05Z', tokens_used: 0, auth_mode: null,
        },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);
    const lifted = liftedProps();
    lifted.sendingMap = { 'd-plural': true };
    lifted.streamingMap = { 'd-plural': '' };

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[fullDisc]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-plural"
        {...lifted}
      />
    );

    expect(screen.getByTestId('streaming-agent-LiteLlm')).toHaveTextContent('LiteLLM');
    expect(screen.queryByTestId('pending-agent-Ollama')).toBeNull();
    expect(screen.getAllByText("Agent en cours d'exécution...")).toHaveLength(1);
    expect(screen.queryByText("En file — en attente d'un créneau agent")).toBeNull();
  });

  it('renders a checkpointed response after backend restart instead of an empty placeholder', async () => {
    const fullDisc = {
      ...makeListDiscussion('d-restart-partial', 1),
      agent: 'Custom' as const,
      participants: ['Custom' as const],
      awaiting_agent: true,
      messages: [{
        id: 'u-restart', role: 'User' as const, channel: 'main' as const,
        content: 'Analyse longtemps', agent_type: null,
        timestamp: '2026-09-01T09:45:13Z', tokens_used: 0, auth_mode: null,
      }],
      active_agent_dispatches: [{
        id: 'job-restart',
        trigger_message_id: 'u-restart',
        agent_type: 'Custom' as const,
        status: 'Pending',
        attempts: 2,
        last_error: 'backend_restarted',
        connection_id: 'openrouter-main',
      }],
      partial_response: {
        message_id: 'partial-restart',
        content: 'Les 1 919 caractères déjà analysés restent visibles.',
        started_at: '2026-09-01T09:46:40Z',
        agent_type: 'Custom' as const,
        model: 'claude-sonnet',
        trigger_message_id: 'u-restart',
        connection_id: 'openrouter-main',
        dispatch: {
          id: 'job-restart',
          trigger_message_id: 'u-restart',
          agent_type: 'Custom' as const,
          status: 'Pending',
          attempts: 2,
          last_error: 'backend_restarted',
          connection_id: 'openrouter-main',
        },
      },
    };
    vi.mocked(externalApiConnections.list).mockResolvedValueOnce([{
      id: 'openrouter-main',
      display_name: 'OpenRouter',
      mention_alias: 'openrouter',
      origin_preset: 'open_router',
    } as never]);
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[fullDisc]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-restart-partial"
        {...liftedProps()}
      />,
    );

    const response = await screen.findByTestId('streaming-agent-Custom');
    expect(response).toHaveTextContent('OpenRouter');
    expect(response).toHaveTextContent('Les 1 919 caractères déjà analysés restent visibles.');
    expect(response).toHaveTextContent('Backend redémarré — brouillon sauvegardé');
    expect(response).not.toHaveTextContent("Agent en cours d'exécution...");
  });

  it('keeps the latest local chunks visible when an accepted stream disconnects', async () => {
    const fullDisc = {
      ...makeListDiscussion('d-stream-disconnect', 1),
      messages: [{
        id: 'u-existing', role: 'User' as const, channel: 'main' as const,
        content: 'Question précédente', agent_type: null,
        timestamp: '2026-09-01T09:45:13Z', tokens_used: 0, auth_mode: null,
      }],
    };
    let detailFetches = 0;
    vi.mocked(discussionsApi.get).mockImplementation(async () => {
      detailFetches += 1;
      if (detailFetches === 1) return fullDisc;
      throw new Error('backend restarting');
    });
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId, payload, onText, _onDone, onError, _signal, onStart, _onLog, onAccepted) => {
        onStart?.();
        onAccepted?.({
          message_id: payload.client_message_id ?? 'u-new',
          sort_order: 2,
          duplicate: false,
        });
        onText?.('Analyse locale déjà reçue avant la coupure.');
        // Same-frame failure: the rAF-backed lifted map has not rendered yet.
        // The synchronous recovery buffer must still retain this chunk.
        onError?.('network disconnected');
      },
    );

    function StatefulDiscussion() {
      const [sendingMap, setSendingMap] = useState<Record<string, boolean>>({});
      const [streamingMap, setStreamingMap] = useState<Record<string, string>>({});
      const [sendingStartMap, setSendingStartMap] = useState<Record<string, number>>({});
      const [queuedMap, setQueuedMap] = useState<Record<string, boolean>>({});
      const abortControllers = useRef<Record<string, AbortController>>({});
      return (
        <DiscussionsPage
          projects={[]}
          agents={[]}
          allDiscussions={[fullDisc]}
          configLanguage="fr"
          agentAccess={null}
          refetchDiscussions={noop}
          refetchProjects={noop}
          onNavigate={noop}
          toast={toastFn}
          initialActiveDiscussionId="d-stream-disconnect"
          sendingMap={sendingMap}
          setSendingMap={setSendingMap}
          queuedMap={queuedMap}
          setQueuedMap={setQueuedMap}
          sendingStartMap={sendingStartMap}
          setSendingStartMap={setSendingStartMap}
          streamingMap={streamingMap}
          setStreamingMap={setStreamingMap}
          noteStreamTick={noop}
          abortControllers={abortControllers}
          cleanupStream={(discId) => {
            setSendingMap(previous => ({ ...previous, [discId]: false }));
            setStreamingMap(previous => {
              const { [discId]: _removed, ...rest } = previous;
              return rest;
            });
          }}
          markDiscussionSeen={noop}
          onActiveDiscussionChange={noop}
          lastSeenMsgCount={{}}
        />
      );
    }

    await wrap(<StatefulDiscussion />);
    const chatInput = document.querySelector('textarea') as HTMLTextAreaElement;
    await act(async () => {
      fireEvent.change(chatInput, { target: { value: 'Lancer une longue analyse' } });
    });
    await act(async () => {
      fireEvent.click(document.querySelector('button[aria-label="Send message"]') as HTMLButtonElement);
      await new Promise(resolve => setTimeout(resolve, 10));
    });

    await waitFor(() => {
      expect(document.body).toHaveTextContent('Analyse locale déjà reçue avant la coupure.');
    });
    expect(screen.getAllByText('Analyse locale déjà reçue avant la coupure.')).toHaveLength(1);
    expect(document.body).toHaveTextContent('Connexion au flux interrompue');
    expect(document.body).not.toHaveTextContent("Agent en cours d'exécution...");
  });

  it('renders a legacy checkpoint even when no active dispatch row survives', async () => {
    const fullDisc = {
      ...makeListDiscussion('d-orphan-checkpoint', 1),
      messages: [{
        id: 'u-orphan', role: 'User' as const, channel: 'main' as const,
        content: 'Analyse interrompue', agent_type: null,
        timestamp: '2026-09-01T09:45:13Z', tokens_used: 0, auth_mode: null,
      }],
      partial_response: {
        message_id: 'partial-orphan',
        content: 'Fragment durable sans ligne de dispatch.',
        agent_type: 'ClaudeCode' as const,
        trigger_message_id: 'u-orphan',
      },
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[fullDisc]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-orphan-checkpoint"
        {...liftedProps()}
      />,
    );

    expect(await screen.findByTestId('streaming-agent-ClaudeCode'))
      .toHaveTextContent('Fragment durable sans ligne de dispatch.');
  });

  it('keeps overlapping reply slots attached to their own turns and reorders a late reply', async () => {
    const fullDisc = {
      ...makeListDiscussion('d-overlap', 5),
      agent: 'LiteLlm' as const,
      participants: ['LiteLlm', 'Ollama', 'Codex'] as AgentType[],
      awaiting_agent: true,
      messages: [
        { id: 'u-old', role: 'User' as const, channel: 'main' as const, content: 'Premier tour', agent_type: null, timestamp: '2026-08-10T11:07:48Z', tokens_used: 0, auth_mode: null },
        { id: 'a-fast', role: 'Agent' as const, channel: 'main' as const, content: 'Lite rapide', agent_type: 'LiteLlm' as const, reply_to_message_id: 'u-old', timestamp: '2026-08-10T11:07:58Z', tokens_used: 1, auth_mode: 'local' },
        { id: 'u-new', role: 'User' as const, channel: 'main' as const, content: 'Second tour', agent_type: null, timestamp: '2026-08-10T11:08:25Z', tokens_used: 0, auth_mode: null },
        { id: 'a-late', role: 'Agent' as const, channel: 'main' as const, content: 'Ollama lent', agent_type: 'Ollama' as const, reply_to_message_id: 'u-old', timestamp: '2026-08-10T11:08:52Z', tokens_used: 1, auth_mode: 'local' },
      ],
      active_agent_dispatches: [
        { id: 'new-lite', trigger_message_id: 'u-new', agent_type: 'LiteLlm' as const, status: 'Running' },
        { id: 'new-ollama', trigger_message_id: 'u-new', agent_type: 'Ollama' as const, status: 'Pending' },
        { id: 'new-codex', trigger_message_id: 'u-new', agent_type: 'Codex' as const, status: 'Pending' },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);
    vi.mocked(discussionsApi.stopDispatch).mockResolvedValue({
      cancelled: true,
      dispatch_id: 'new-ollama',
      still_awaiting: true,
    });

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[fullDisc]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-overlap"
        {...liftedProps()}
      />,
    );

    const lateReply = screen.getByText('Ollama lent').closest('.disc-msg-row');
    const secondTurn = screen.getByText('Second tour').closest('.disc-msg-row');
    expect(lateReply).not.toBeNull();
    expect(secondTurn).not.toBeNull();
    expect((lateReply as Node).compareDocumentPosition(secondTurn as Node) & Node.DOCUMENT_POSITION_FOLLOWING)
      .toBeTruthy();

    expect(screen.getByTestId('pending-agent-LiteLlm')).toHaveAttribute('data-reply-trigger', 'u-new');
    expect(screen.getByTestId('pending-agent-Ollama')).toHaveAttribute('data-reply-trigger', 'u-new');
    expect(screen.getByTestId('pending-agent-Codex')).toHaveAttribute('data-reply-trigger', 'u-new');

    fireEvent.click(screen.getByRole('button', {
      name: 'Arrêter uniquement la réponse de Ollama',
    }));
    await waitFor(() => {
      expect(discussionsApi.stopDispatch).toHaveBeenCalledWith('d-overlap', 'new-ollama');
    });
    expect(toastFn).toHaveBeenCalledWith('Réponse agent arrêtée', 'success');
  });

  it('restores active discussion on remount via initialActiveDiscussionId', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 2),
      messages: [
        { id: 'm1', role: 'User', channel: 'main', content: 'My question', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'm2', role: 'Agent', channel: 'main', content: 'My answer', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:01Z', tokens_used: 100, auth_mode: null },
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

  it('re-syncs the active discussion on WS reconnect (0.9.2-F gate DoD #4)', async () => {
    // A dropped WS that re-connects must RE-SYNC and APPLY the result, not
    // leave a stale view or duplicate. useWebSocket calls onConnect on every
    // (re)connect (proven in useWebSocket.test.ts); here we assert that
    // DiscussionsPage's onConnect refetches the list AND reloads the active
    // discussion, and that a NEW message from the reload actually renders once.
    const before: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm1', role: 'User', channel: 'main', content: 'before reconnect', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(before);
    const refetch = vi.fn();
    const lifted = liftedProps();

    const { useWebSocket } = await import('../../hooks/useWebSocket');
    let capturedOnConnect: (() => void) | undefined;
    vi.mocked(useWebSocket).mockImplementation((_onMessage, onConnect) => {
      capturedOnConnect = onConnect as (() => void) | undefined;
      return { connected: true, connectionState: 'connected' };
    });

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[{ ...makeListDiscussion('d1', 1), messages: before.messages }]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={refetch}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d1"
        {...lifted}
      />
    );

    // The reload after reconnect carries a NEW, distinctive message.
    const marker = 'applied-after-reconnect-42';
    const after: Discussion = {
      ...makeListDiscussion('d1', 2),
      messages: [
        { id: 'm1', role: 'User', channel: 'main', content: 'before reconnect', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'm2', role: 'Agent', channel: 'main', content: marker, agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:02Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(after);
    refetch.mockClear();

    // Simulate the socket dropping and re-connecting.
    await act(async () => {
      capturedOnConnect?.();
    });

    // The reloaded disc's new message is APPLIED and rendered exactly once
    // (proves messages re-sync + no duplicate). No arbitrary sleep — waitFor.
    await waitFor(() => {
      expect(screen.getAllByText(marker)).toHaveLength(1);
    });
    // The list snapshot (carries awaiting_agent) is re-fetched, and the active
    // disc was reloaded by id.
    expect(refetch).toHaveBeenCalled();
    expect(vi.mocked(discussionsApi.get)).toHaveBeenCalledWith('d1');
    expect(vi.mocked(discussionsApi.listContextFiles)).toHaveBeenCalledWith('d1');

    vi.mocked(useWebSocket).mockImplementation(() => ({ connected: false, connectionState: 'connecting' }));
  });

  it('shows files pinned by an MCP agent as soon as the context event arrives', async () => {
    const discussion: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm-agent', role: 'Agent', channel: 'main', content: 'Voici le rapport', agent_type: 'Codex', timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(discussion);
    vi.mocked(discussionsApi.listContextFiles)
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([{
        id: 'cf-report',
        discussion_id: 'd1',
        filename: 'rapport.csv',
        mime_type: 'text/csv',
        original_size: 128,
        extracted_size: 128,
        disk_path: null,
        message_id: 'm-agent',
        created_at: '2026-01-01T00:00:01Z',
      }]);

    const { useWebSocket } = await import('../../hooks/useWebSocket');
    let fireContextChanged: (() => void) | undefined;
    vi.mocked(useWebSocket).mockImplementation((onMessage) => {
      fireContextChanged = () => onMessage({
        type: 'context_files_changed',
        discussion_id: 'd1',
        message_id: 'm-agent',
      });
      return { connected: true, connectionState: 'connected' };
    });

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[discussion]}
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

    await act(async () => { fireContextChanged?.(); });

    await waitFor(() => expect(screen.getByText('rapport.csv')).toBeInTheDocument());
    expect(vi.mocked(discussionsApi.listContextFiles)).toHaveBeenLastCalledWith('d1');
    vi.mocked(useWebSocket).mockImplementation(() => ({ connected: false, connectionState: 'connecting' }));
  });

  it('refreshes a previously cached empty asset list when returning to a discussion', async () => {
    const first: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm-first', role: 'Agent', channel: 'main', content: 'Asset source', agent_type: 'Codex', timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    const second: Discussion = {
      ...makeListDiscussion('d2', 1),
      messages: [
        { id: 'm-second', role: 'User', channel: 'main', content: 'Other room', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    const lateAsset: ContextFile = {
      id: 'cf-late',
      discussion_id: 'd1',
      filename: 'late-report.csv',
      mime_type: 'text/csv',
      original_size: 256,
      extracted_size: 256,
      disk_path: null,
      message_id: 'm-first',
      created_at: '2026-01-01T00:01:00Z',
    };
    vi.mocked(discussionsApi.get).mockImplementation(async id => id === 'd1' ? first : second);
    vi.mocked(discussionsApi.listContextFiles).mockImplementation(async id => {
      const callsForFirst = vi.mocked(discussionsApi.listContextFiles).mock.calls
        .filter(call => call[0] === 'd1').length;
      if (id === 'd1' && callsForFirst > 1) return [lateAsset];
      return [];
    });

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[first, second]}
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

    await waitFor(() => expect(discussionsApi.listContextFiles).toHaveBeenCalledWith('d1'));
    expect(screen.queryByRole('button', { name: /Parcourir tous les assets/ })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /Discussion d2 —/ }));
    await waitFor(() => expect(discussionsApi.listContextFiles).toHaveBeenCalledWith('d2'));
    fireEvent.click(screen.getByRole('button', { name: /Discussion d1 —/ }));

    await waitFor(() => {
      expect(vi.mocked(discussionsApi.listContextFiles).mock.calls
        .filter(call => call[0] === 'd1')).toHaveLength(2);
    });
    fireEvent.click(await screen.findByRole('button', { name: /Parcourir tous les assets.*1/ }));
    expect((await screen.findAllByText('late-report.csv')).length).toBeGreaterThan(0);
  });

  it('opens the discussion asset inventory and jumps back to the source message', async () => {
    const discussion: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm-asset', role: 'Agent', channel: 'main', content: 'Here is the file', agent_type: 'Codex', timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(discussion);
    vi.mocked(discussionsApi.listContextFiles).mockResolvedValue([{
      id: 'cf-source',
      discussion_id: 'd1',
      filename: 'source.csv',
      mime_type: 'text/csv',
      original_size: 128,
      extracted_size: 128,
      disk_path: null,
      message_id: 'm-asset',
      created_at: '2026-01-01T00:01:00Z',
    }]);
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', {
      configurable: true,
      value: scrollIntoView,
    });

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[discussion]}
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

    const assetsButton = await screen.findByRole('button', { name: /Parcourir tous les assets.*1/ });
    fireEvent.click(assetsButton);
    expect(screen.getByRole('complementary', { name: 'Assets' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Aller au message contenant source\.csv/ }));

    await waitFor(() => expect(scrollIntoView).toHaveBeenCalled());
    expect(screen.queryByRole('complementary', { name: 'Assets' })).toBeNull();
  });

  it('explains that real-time data will resync while the socket reconnects', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    const discussion = makeListDiscussion('d1', 0);
    vi.mocked(discussionsApi.get).mockResolvedValue(discussion);
    vi.mocked(useWebSocket).mockImplementation(() => ({
      connected: false,
      connectionState: 'reconnecting',
    }));

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[discussion]}
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

    expect(screen.getByRole('status')).toHaveTextContent(/reconnexion en cours/i);
    expect(screen.getByRole('status')).toHaveTextContent(/brouillons sont conservés/i);

    vi.mocked(useWebSocket).mockImplementation(() => ({
      connected: false,
      connectionState: 'connecting',
    }));
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

  it('clears the orphaned SSE controller when a partial_response_recovered WS event arrives', async () => {
    // Regression: a backend restart mid-stream flips sending=false via this WS
    // event but used to leave abortControllers[disc] set. That orphaned
    // controller kept handleSendMessage's re-entry guard armed forever, so
    // every queued follow-up re-enqueued instead of firing — the queue got
    // permanently stuck. The handler must drop the controller to restore the
    // "sending=false ⟺ no controller" invariant.
    const controller = new AbortController();
    const lifted = liftedProps();
    lifted.abortControllers = { current: { d1: controller } };

    vi.mocked(discussionsApi.get).mockResolvedValue(makeListDiscussion('d1', 2));

    const { useWebSocket } = await import('../../hooks/useWebSocket');
    let firedRecovered = false;
    vi.mocked(useWebSocket).mockImplementation((onMessage) => {
      // Fire-once: this mock runs on EVERY render — an unguarded setTimeout
      // per render leaks a late timer into the NEXT test (cleared mocks →
      // undefined.then crash, seen as a Vitest unhandled error).
      if (!firedRecovered) {
        firedRecovered = true;
        setTimeout(() => onMessage({ type: 'partial_response_recovered', discussion_ids: ['d1'] }), 10);
      }
      return { connected: true, connectionState: 'connected' };
    });

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
        {...lifted}
      />
    );
    await act(async () => { await new Promise(r => setTimeout(r, 100)); });

    expect(lifted.abortControllers.current.d1).toBeUndefined();
    vi.mocked(useWebSocket).mockImplementation(() => ({ connected: false, connectionState: 'connecting' }));
  });

  it('marks a child queued on batch_run_child_queued WITHOUT refetching the list', async () => {
    // These frames arrive N-at-once for a big batch; a refetch per frame
    // would burst N requests. The handler must only flip queuedMap.
    const lifted = liftedProps();
    const refetch = vi.fn();

    // Defensive: a stray late timer from a prior test may call reloadDiscussion
    // during this test — give it a working mock instead of undefined.then.
    vi.mocked(discussionsApi.get).mockResolvedValue(makeListDiscussion('d1', 2));

    const { useWebSocket } = await import('../../hooks/useWebSocket');
    let firedQueued = false;
    vi.mocked(useWebSocket).mockImplementation((onMessage) => {
      if (!firedQueued) {
        firedQueued = true;
        setTimeout(() => onMessage({ type: 'batch_run_child_queued', run_id: 'r1', discussion_id: 'd1' }), 10);
      }
      return { connected: true, connectionState: 'connected' };
    });

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={refetch}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        {...lifted}
      />
    );
    await act(async () => { await new Promise(r => setTimeout(r, 100)); });

    // queuedMap flipped for d1 (functional updater called with prev state)…
    expect(lifted.setQueuedMap).toHaveBeenCalled();
    const updater = lifted.setQueuedMap.mock.calls[0][0];
    expect(updater({})).toEqual({ d1: true });
    // …and NOT a single list refetch from this frame.
    expect(refetch).not.toHaveBeenCalled();
    vi.mocked(useWebSocket).mockImplementation(() => ({ connected: false, connectionState: 'connecting' }));
  });

  it('clears sending AND queued state on agent_runs_interrupted', async () => {
    // A browser left open across a backend restart must drop BOTH
    // indicators for the interrupted discs, or the queued dot sticks forever.
    const controller = new AbortController();
    const lifted = liftedProps();
    lifted.abortControllers = { current: { d1: controller } };
    vi.mocked(discussionsApi.get).mockResolvedValue(makeListDiscussion('d1', 2));

    const { useWebSocket } = await import('../../hooks/useWebSocket');
    let firedInterrupted = false;
    vi.mocked(useWebSocket).mockImplementation((onMessage) => {
      if (!firedInterrupted) {
        firedInterrupted = true;
        setTimeout(() => onMessage({ type: 'agent_runs_interrupted', discussion_ids: ['d1'] }), 10);
      }
      return { connected: true, connectionState: 'connected' };
    });

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
        {...lifted}
      />
    );
    await act(async () => { await new Promise(r => setTimeout(r, 100)); });

    // Both maps cleared for d1 (functional updaters), controller dropped, user toasted.
    const sendingUpdater = lifted.setSendingMap.mock.calls.at(-1)![0];
    expect(sendingUpdater({ d1: true })).toEqual({ d1: false });
    const queuedUpdater = lifted.setQueuedMap.mock.calls.at(-1)![0];
    expect(queuedUpdater({ d1: true })).toEqual({ d1: false });
    expect(lifted.abortControllers.current.d1).toBeUndefined();
    // The component runs the real fr i18n — assert on the translated copy.
    expect(toastFn).toHaveBeenCalledWith(expect.stringContaining("en attente d'agent interrompue"), 'info');
    vi.mocked(useWebSocket).mockImplementation(() => ({ connected: false, connectionState: 'connecting' }));
  });

  it('refetches discussion when sending finishes (activeSending changes)', async () => {
    const discWithResponse: Discussion = {
      ...makeListDiscussion('d1', 2),
      messages: [
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'm2', role: 'Agent', channel: 'main', content: 'Response', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:01Z', tokens_used: 50, auth_mode: null },
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

  it('refreshes the pending-proposal header count when a new Agent message lands', async () => {
    const first = makeListDiscussion('d1', 1);
    const withProposal = makeListDiscussion('d1', 2);
    vi.mocked(discussionsApi.get)
      .mockResolvedValueOnce(first)
      .mockResolvedValue(withProposal);
    vi.mocked(planningApi.proposals)
      .mockResolvedValueOnce({
        proposals: [],
        pending_proposal_count: 0,
        pending_item_count: 0,
      })
      .mockResolvedValue({
        proposals: [],
        pending_proposal_count: 1,
        pending_item_count: 1,
      });

    const lifted = liftedProps();
    lifted.sendingMap = { d1: true };
    const { rerender, container } = await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[first]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d1"
        {...lifted}
      />,
    );
    await waitFor(() => expect(planningApi.proposals).toHaveBeenCalledTimes(1));
    expect(container.querySelector('.disc-plan-pending')).toBeNull();

    await act(async () => {
      rerender(
        <I18nProvider>
          <DiscussionsPage
            projects={[]}
            agents={[]}
            allDiscussions={[first]}
            configLanguage="fr"
            agentAccess={null}
            refetchDiscussions={noop}
            refetchProjects={noop}
            onNavigate={noop}
            toast={toastFn}
            initialActiveDiscussionId="d1"
            {...lifted}
            sendingMap={{ d1: false }}
          />
        </I18nProvider>,
      );
    });

    await waitFor(() => expect(planningApi.proposals).toHaveBeenCalledTimes(2));
    expect(container.querySelector('.disc-plan-pending')?.textContent).toBe('1');
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
        { id: 'm1', role: 'User', channel: 'main', content: 'Hi', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
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
      archived: false, pinned: false, pin_first_message: false,
  tier: "default" as const, summary_strategy: "Auto" as const, introspection_call_count: 0,
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
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello Vibe', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
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
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
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
    const proj1 = { id: 'p1', name: 'web-app', path: '/repos/web-app', repo_url: 'git@github.com:acme-org/web-app.git', token_override: null, ai_config: { detected: false, configs: [] }, audit_status: 'NoTemplate' as AiAuditStatus, ai_todo_count: 0, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z', path_exists: true, tech_debt_count: 0, needs_docs_migration: false };
    const proj2 = { id: 'p2', name: 'api-server', path: '/repos/api-server', repo_url: 'git@github.com:johndoe/api-server.git', token_override: null, ai_config: { detected: false, configs: [] }, audit_status: 'NoTemplate' as AiAuditStatus, ai_todo_count: 0, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z', path_exists: true, tech_debt_count: 0, needs_docs_migration: false };

    const disc1 = { ...makeListDiscussion('d1', 1), project_id: 'p1', messages: [{ id: 'm1', role: 'User' as const, channel: 'main' as const, content: 'test', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null }] };
    const disc2 = { ...makeListDiscussion('d2', 1), project_id: 'p2', messages: [{ id: 'm2', role: 'User' as const, channel: 'main' as const, content: 'test', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null }] };

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
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
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
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
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
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'm2', role: 'Agent', channel: 'main', content: 'Bonjour!', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:01Z', tokens_used: 50, auth_mode: null },
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
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'm2', role: 'Agent', channel: 'main', content: 'Bonjour le monde!', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:01Z', tokens_used: 50, auth_mode: null },
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

  it('reaches the real bottom in a single click while the list is still growing', async () => {
    // Reported bug 2026-07-27: "on clique 3/4 fois sur la flèche pour arriver
    // tout en bas". A single scrollIntoView aims at the height measured when it
    // fires, so late layout (markdown, mermaid, media) leaves it short.
    const messages = Array.from({ length: 30 }, (_, i) => ({
      id: `m${i}`,
      role: (i % 2 === 0 ? 'User' : 'Agent') as 'User' | 'Agent',
      channel: 'main' as const,
      content: `message ${i}`,
      agent_type: i % 2 === 0 ? null : ('ClaudeCode' as const),
      timestamp: '2026-01-01T00:00:00Z',
      tokens_used: 0,
      auth_mode: null,
    }));

    const disc = { ...makeListDiscussion('d1', messages.length), messages };
    vi.mocked(discussionsApi.get).mockResolvedValue(disc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[disc]}
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

    // jsdom performs no layout, so the scroll metrics are driven by hand.
    const container = document.querySelector('.disc-messages') as HTMLElement;
    let scrollHeight = 5000;
    Object.defineProperty(container, 'scrollHeight', { configurable: true, get: () => scrollHeight });
    Object.defineProperty(container, 'clientHeight', { configurable: true, get: () => 800 });
    container.scrollTop = 0;
    await act(async () => { fireEvent.scroll(container); });

    const button = document.querySelector('[data-testid="disc-scroll-to-bottom"]') as HTMLButtonElement;
    expect(button).not.toBeNull();

    // The list keeps growing for a few frames after the click, then settles.
    const grow = setInterval(() => { if (scrollHeight < 9000) scrollHeight += 1000; }, 16);
    await act(async () => { fireEvent.click(button); });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 300)); });
    clearInterval(grow);
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 200)); });

    // One click, and the container sits at the final height — not the one
    // measured when the click happened.
    expect(scrollHeight).toBe(9000);
    expect(container.scrollTop).toBe(9000);
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
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
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
        { id: 'persisted-agent', role: 'Agent', channel: 'main', content: 'Streamed agent reply.', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:01Z', tokens_used: 12, auth_mode: null },
      ],
      message_count: 2, non_system_message_count: 2, tier: "default" as const, summary_strategy: "Auto" as const, introspection_call_count: 0,
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
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
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

  it('uses the client message UUID for the optimistic row and request', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);
    let sentPayload: { content: string; client_message_id?: string } | undefined;
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId: any, payload: any) => {
        sentPayload = payload;
      },
    );

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

    await act(async () => { await new Promise(r => setTimeout(r, 0)); });
    const chatInput = document.querySelector('textarea') as HTMLTextAreaElement;
    await act(async () => {
      fireEvent.change(chatInput, { target: { value: 'Stable optimistic id' } });
    });
    const sendBtn = document.querySelector('button[aria-label="Send message"]') as HTMLButtonElement;
    await act(async () => { fireEvent.click(sendBtn); });

    expect(sentPayload?.client_message_id).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    const optimisticIdPill = document.querySelector(
      `button[title*="${sentPayload?.client_message_id}"]`,
    );
    expect(optimisticIdPill).toBeTruthy();
  });

  it('sends the selected durable reply target from the composer', async () => {
    const sourceId = '12345678-1234-4234-8234-123456789abc';
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        {
          id: sourceId,
          role: 'Agent',
          channel: 'main',
          content: 'Original answer',
          agent_type: 'Codex',
          timestamp: '2026-01-01T00:00:00Z',
          tokens_used: 0,
          auth_mode: null,
        },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);
    let sentPayload: {
      content: string;
      reply_to_message_id?: string | null;
    } | undefined;
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId: any, payload: any) => {
        sentPayload = payload;
      },
    );

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
      />,
    );
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });

    fireEvent.click(screen.getByRole('button', { name: /Reply|Répondre/ }));
    expect(document.querySelector('.disc-reply-composer-preview'))
      .toHaveTextContent('#12345678');
    expect(document.querySelector('.disc-messages-col'))
      .toHaveAttribute('data-replying', 'true');
    const chatInput = document.querySelector('textarea') as HTMLTextAreaElement;
    fireEvent.change(chatInput, { target: { value: 'Follow-up answer' } });
    const sendBtn = document.querySelector(
      'button[aria-label="Send message"]',
    ) as HTMLButtonElement;
    await act(async () => { fireEvent.click(sendBtn); });

    expect(sentPayload?.reply_to_message_id).toBe(sourceId);
  });

  it('restores the reply target when a send is refused by a 502', async () => {
    const sourceId = '87654321-1234-4234-8234-123456789abc';
    clearReplyDraft('d1');
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [{
        id: sourceId,
        role: 'Agent',
        channel: 'main',
        content: 'Original answer',
        agent_type: 'Codex',
        timestamp: '2026-01-01T00:00:00Z',
        tokens_used: 0,
        auth_mode: null,
      }],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId: any, _payload: any, _onText: any, _onDone: any, onError: any) => {
        onError('502 Bad Gateway');
      },
    );

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
      />,
    );
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });

    fireEvent.click(screen.getByRole('button', { name: /Reply|Répondre/ }));
    const chatInput = document.querySelector('textarea') as HTMLTextAreaElement;
    fireEvent.change(chatInput, { target: { value: 'Retry after reconnect' } });
    await act(async () => {
      fireEvent.click(document.querySelector(
        'button[aria-label="Send message"]',
      ) as HTMLButtonElement);
    });

    expect(document.querySelector('.disc-reply-composer-preview'))
      .toHaveTextContent('#87654321');
    expect(loadReplyDraft('d1')?.messageId).toBe(sourceId);
  });

  // ── KT-113 — a send refused while the previous run is still recovering ─────

  const renderForPartialPending = async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [{
        id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null,
        timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null,
      }],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);
    const lifted = liftedProps();
    // Mirror Dashboard's real cleanupStream, which drops the abort controller.
    // With the default no-op stub the refused send would still look in-flight and
    // the resend would silently take the queue path instead.
    lifted.cleanupStream = vi.fn((discId: string) => {
      delete lifted.abortControllers.current[discId];
    });
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
      />,
    );
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
    const textarea = document.querySelector('textarea') as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'Message pendant la récupération' } });
    await act(async () => {
      fireEvent.click(document.querySelector(
        'button[aria-label="Send message"]',
      ) as HTMLButtonElement);
    });
    return textarea;
  };

  it('shows a non-blocking banner instead of freezing the tab, and keeps the text', async () => {
    // `confirm()` blocks the whole tab; recovery can last minutes, so the user
    // could not even copy their message while waiting. happy-dom has no native
    // confirm, so install one and assert nothing ever reaches it.
    const confirmSpy = vi.fn(() => true);
    vi.stubGlobal('confirm', confirmSpy);
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId: any, _payload: any, _onText: any, _onDone: any, onError: any) => {
        onError('partial_pending: previous run still recovering');
      },
    );

    const textarea = await renderForPartialPending();

    expect(confirmSpy).not.toHaveBeenCalled();
    expect(screen.getByTestId('disc-partial-pending')).toBeInTheDocument();
    // The refused settlement puts the text back, so nothing has to be retyped.
    expect(textarea.value).toContain('Message pendant la récupération');
    expect(loadDraft('d1')?.text).toContain('Message pendant la récupération');
    vi.unstubAllGlobals();
  });

  it('forces the recovery and resends the refused message exactly once', async () => {
    const payloads: Array<{ content: string }> = [];
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId: any, payload: any, _onText: any, _onDone: any, onError: any) => {
        payloads.push(payload);
        // Only the FIRST attempt is refused: the resend must go through.
        if (payloads.length === 1) {
          onError('partial_pending: previous run still recovering');
        }
      },
    );

    await renderForPartialPending();
    await act(async () => {
      fireEvent.click(screen.getByTestId('disc-partial-pending-force'));
    });

    expect(discussionsApi.dismissPartial).toHaveBeenCalledWith('d1');
    // Two calls total: the refused one, then a single resend — not three.
    expect(payloads).toHaveLength(2);
    expect(payloads[1].content).toBe('Message pendant la récupération');
    expect(screen.queryByTestId('disc-partial-pending')).toBeNull();
  });

  it('sends the held message by itself once the recovery finishes', async () => {
    // The user asked for the queue's contract: it fires on its own when the run
    // ends. Here the end-of-recovery WS event is that edge.
    const payloads: Array<{ content: string }> = [];
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId: any, payload: any, _onText: any, _onDone: any, onError: any) => {
        payloads.push(payload);
        if (payloads.length === 1) {
          onError('partial_pending: previous run still recovering');
        }
      },
    );
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    let fireRecovered: (() => void) | null = null;
    vi.mocked(useWebSocket).mockImplementation((onMessage) => {
      fireRecovered = () => onMessage(
        { type: 'partial_response_recovered', discussion_ids: ['d1'] } as any,
      );
      return { connected: true, connectionState: 'connected' };
    });

    await renderForPartialPending();
    expect(payloads).toHaveLength(1);

    await act(async () => { fireRecovered?.(); await new Promise(r => setTimeout(r, 0)); });

    // Sent without any click, and only once.
    expect(payloads).toHaveLength(2);
    expect(payloads[1].content).toBe('Message pendant la récupération');
    expect(screen.queryByTestId('disc-partial-pending')).toBeNull();
  });

  it('cancelling the auto-send stops it even when the recovery finishes later', async () => {
    const payloads: Array<{ content: string }> = [];
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId: any, payload: any, _onText: any, _onDone: any, onError: any) => {
        payloads.push(payload);
        if (payloads.length === 1) {
          onError('partial_pending: previous run still recovering');
        }
      },
    );
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    let fireRecovered: (() => void) | null = null;
    vi.mocked(useWebSocket).mockImplementation((onMessage) => {
      fireRecovered = () => onMessage(
        { type: 'partial_response_recovered', discussion_ids: ['d1'] } as any,
      );
      return { connected: true, connectionState: 'connected' };
    });

    const textarea = await renderForPartialPending();
    await act(async () => {
      fireEvent.click(screen.getByTestId('disc-partial-pending-dismiss'));
    });
    await act(async () => { fireRecovered?.(); await new Promise(r => setTimeout(r, 0)); });

    // Cancel wins over the automatic edge, and the text is still there to resend.
    expect(payloads).toHaveLength(1);
    expect(textarea.value).toContain('Message pendant la récupération');
  });

  it('dismissing the banner leaves the message in the composer', async () => {
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId: any, _payload: any, _onText: any, _onDone: any, onError: any) => {
        onError('partial_pending: previous run still recovering');
      },
    );

    const textarea = await renderForPartialPending();
    await act(async () => {
      fireEvent.click(screen.getByTestId('disc-partial-pending-dismiss'));
    });

    expect(screen.queryByTestId('disc-partial-pending')).toBeNull();
    expect(textarea.value).toContain('Message pendant la récupération');
  });

  it('navigates from a reply header to the original message', async () => {
    const sourceId = 'aaaaaaaa-1234-4234-8234-123456789abc';
    const replyId = 'bbbbbbbb-1234-4234-8234-123456789abc';
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 2),
      messages: [
        {
          id: sourceId,
          role: 'Agent',
          channel: 'main',
          content: 'Original answer',
          agent_type: 'Codex',
          timestamp: '2026-01-01T00:00:00Z',
          tokens_used: 0,
          auth_mode: null,
        },
        {
          id: replyId,
          role: 'User',
          channel: 'main',
          content: 'A reply',
          agent_type: null,
          timestamp: '2026-01-01T00:01:00Z',
          tokens_used: 0,
          auth_mode: null,
          reply_to_message_id: sourceId,
        },
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
      />,
    );
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });

    fireEvent.click(screen.getByTitle(/Show original message|Afficher le message d’origine/));

    expect(
      document.querySelector(`[data-message-id="${sourceId}"]`),
    ).toHaveAttribute('data-search-current', 'true');
  });

  it('reverts the optimistic row when the send errors before acceptance', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d1', 1),
      messages: [
        { id: 'm1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);
    // No `accepted` receipt: the backend refused the send (e.g. partial_pending),
    // so the optimistic User row must NOT linger as a phantom "sent" message.
    // jsdom has no `confirm`; user cancels the dismiss prompt (returns false).
    const prevConfirm = window.confirm;
    window.confirm = vi.fn(() => false);
    vi.mocked(discussionsApi.sendMessageStream).mockImplementation(
      async (_discId: any, _payload: any, _onChunk: any, _onDone: any, onError: any) => {
        onError('partial_pending: previous run in recovery');
      },
    );

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

    await act(async () => { await new Promise(r => setTimeout(r, 0)); });
    const chatInput = document.querySelector('textarea') as HTMLTextAreaElement;
    await act(async () => {
      fireEvent.change(chatInput, { target: { value: 'Phantom message' } });
    });
    const sendBtn = document.querySelector('button[aria-label="Send message"]') as HTMLButtonElement;
    await act(async () => { fireEvent.click(sendBtn); });
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    // The refused message must not remain in the thread.
    expect(document.body.textContent).not.toContain('Phantom message');
    expect(chatInput.value).toBe('Phantom message');
    expect(loadDraft('d1')?.text).toBe('Phantom message');
    window.confirm = prevConfirm;
  });

  it('creates a new discussion via the form', async () => {
    const createdDisc: Discussion = {
      ...makeListDiscussion('new-disc', 1),
      messages: [
        { id: 'm1', role: 'User', channel: 'main', content: 'Analyse this code', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
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
    // KT-128 — the placeholder agent must arrive DISABLED: left active, the
    // first invited CLI made both it and the CLI answer the same turn.
    expect(createCall.no_agent).toBe(true);

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

  it('launches every agent mentioned in a new-discussion prompt as independent replies', async () => {
    const createdDisc: Discussion = {
      ...makeListDiscussion('prompt-agents-1', 1),
      agent: 'Codex',
      participants: ['Codex'],
      messages: [{
        id: 'prompt-agents-message',
        role: 'User',
        channel: 'main',
        content: '@codex @claude comparez vos approches',
        agent_type: null,
        timestamp: '2026-07-28T00:00:00Z',
        tokens_used: 0,
        auth_mode: null,
      }],
    };
    vi.mocked(discussionsApi.create).mockReset();
    vi.mocked(discussionsApi.create).mockResolvedValue(createdDisc);
    vi.mocked(discussionsApi.get).mockResolvedValue(createdDisc);
    vi.mocked(discussionsApi.runAgent).mockClear();
    vi.mocked(discussionsApi.orchestrate).mockClear();

    const codexAgent: AgentDetection = {
      name: 'Codex',
      agent_type: 'Codex',
      installed: true,
      enabled: true,
      path: '/usr/bin/codex',
      version: '1.0.0',
      latest_version: null,
      origin: 'host',
      install_command: null,
      host_managed: false,
      host_label: null,
      runtime_available: false,
      rtk_available: false,
      rtk_hook_configured: false,
    };
    const claudeAgent: AgentDetection = {
      ...codexAgent,
      name: 'Claude Code',
      agent_type: 'ClaudeCode',
      path: '/usr/bin/claude',
    };

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[codexAgent, claudeAgent]}
        allDiscussions={[]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={vi.fn()}
        {...liftedProps()}
      />,
    );

    await act(async () => { fireEvent.click(screen.getAllByText(/Nouvelle/)[0]); });
    const prompt = document.querySelector(
      'textarea[aria-label="Prompt initial"]',
    ) as HTMLTextAreaElement;
    expect(prompt).toBeTruthy();
    await act(async () => {
      fireEvent.change(prompt, {
        target: { value: '@codex @claude comparez vos approches' },
      });
    });
    expect(screen.getByText('Agents définis par le prompt')).toBeTruthy();
    await waitFor(() => {
      expect(screen.getByText('Réponses multi-agents.')).toBeTruthy();
      expect(screen.getByText(/Collaboration entre agents désactivée/)).toBeTruthy();
    });

    await act(async () => {
      fireEvent.click(screen.getByText(/Démarrer la discussion/));
    });
    await waitFor(() => {
      expect(vi.mocked(discussionsApi.create)).toHaveBeenCalledWith(
        expect.objectContaining({
          agent: 'Codex',
          initial_targets: [
            { kind: 'discussion_agent', agent_type: 'Codex', tier: 'default' },
            { kind: 'agent', agent_type: 'ClaudeCode', tier: 'default' },
          ],
        }),
      );
    });
    await waitFor(() => {
      expect(vi.mocked(discussionsApi.runAgent)).toHaveBeenCalledWith(
        'prompt-agents-1',
        expect.any(Function),
        expect.any(Function),
        expect.any(Function),
        expect.any(AbortSignal),
        expect.any(Function),
        undefined,
        expect.any(Function),
      );
    });
    expect(vi.mocked(discussionsApi.orchestrate)).not.toHaveBeenCalled();
  });

  it('shows copy button on agent messages', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-copy', 2),
      messages: [
        { id: 'u1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a1', role: 'Agent', channel: 'main', content: 'World', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:05Z', tokens_used: 50, auth_mode: null },
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

  it('searches rendered message bodies and navigates every occurrence', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-search-messages', 2),
      messages: [
        {
          id: 'search-user',
          role: 'User',
          channel: 'main',
          content: `${'contenu long '.repeat(1_000)}Alpha dans le premier message.`,
          agent_type: null,
          timestamp: '2026-01-01T00:00:00Z',
          tokens_used: 0,
          auth_mode: null,
        },
        {
          id: 'search-agent',
          role: 'Agent',
          channel: 'main',
          content: 'alpha puis **alpha** dans la réponse',
          agent_type: 'ClaudeCode',
          timestamp: '2026-01-01T00:00:05Z',
          tokens_used: 20,
          auth_mode: null,
        },
      ],
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(fullDisc);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[makeListDiscussion('d-search-messages', 2)]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId="d-search-messages"
        {...liftedProps()}
      />,
    );

    const openSearch = await screen.findByRole('button', { name: 'Rechercher dans les messages' });
    fireEvent.click(openSearch);
    const input = await screen.findByRole('searchbox', { name: 'Rechercher dans cette discussion…' });
    fireEvent.change(input, { target: { value: 'alpha' } });

    await waitFor(() => expect(screen.getByText('1 / 3')).toBeTruthy());
    expect(document.querySelector('[data-message-id="search-user"]')?.getAttribute('data-search-current')).toBe('true');

    fireEvent.click(screen.getByRole('button', { name: 'Occurrence suivante' }));
    await waitFor(() => expect(screen.getByText('2 / 3')).toBeTruthy());
    expect(document.querySelector('[data-message-id="search-agent"]')?.getAttribute('data-search-current')).toBe('true');

    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(screen.getByText('3 / 3')).toBeTruthy());
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(screen.getByText('1 / 3')).toBeTruthy());

    fireEvent.change(input, { target: { value: 'aucune occurrence ici' } });
    await waitFor(() => expect(screen.getByText('Aucun résultat')).toBeTruthy());
    expect(document.querySelector('[data-search-current="true"]')).toBeNull();
  });

  it('opens a global result at the exact message in its discussion', async () => {
    const sourceList = makeListDiscussion('d-global-source', 1);
    const targetList = makeListDiscussion('d-global-target', 1);
    const sourceFull: Discussion = {
      ...sourceList,
      messages: [{
        id: 'source-message',
        role: 'User',
        channel: 'main',
        content: 'Départ',
        agent_type: null,
        timestamp: '2026-01-01T00:00:00Z',
        tokens_used: 0,
        auth_mode: null,
      }],
    };
    const targetFull: Discussion = {
      ...targetList,
      messages: [{
        id: 'global-target-message',
        role: 'Agent',
        channel: 'main',
        content: 'Le résultat Fastly recherché',
        agent_type: 'Codex',
        timestamp: '2026-01-02T00:00:00Z',
        tokens_used: 10,
        auth_mode: null,
      }],
    };
    vi.mocked(discussionsApi.get).mockImplementation(async id => (
      id === targetList.id ? targetFull : sourceFull
    ));
    vi.mocked(discussionsApi.searchMessages).mockResolvedValue([{
      disc_id: targetList.id,
      disc_title: targetList.title,
      message_id: 'global-target-message',
      sort_order: 1,
      role: 'Agent',
      timestamp: '2026-01-02T00:00:00Z',
      snippet: '…résultat Fastly recherché…',
      agent_type: 'Codex',
      author_pseudo: null,
      project_id: null,
    }]);

    await wrap(
      <DiscussionsPage
        projects={[]}
        agents={[]}
        allDiscussions={[sourceList, targetList]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={noop}
        refetchProjects={noop}
        onNavigate={noop}
        toast={toastFn}
        initialActiveDiscussionId={sourceList.id}
        {...liftedProps()}
      />,
    );

    fireEvent.click(await screen.findByRole('button', {
      name: 'Recherche avancée dans tous les messages',
    }));
    const input = await screen.findByTestId('global-search-input');
    fireEvent.change(input, { target: { value: 'Fastly' } });
    await act(async () => {
      fireEvent.submit(input.closest('form')!);
      await Promise.resolve();
    });
    fireEvent.click(await screen.findByRole('button', { name: /Discussion d-global-target/ }));

    await waitFor(() => {
      expect(discussionsApi.get).toHaveBeenCalledWith(targetList.id);
      expect(
        document.querySelector('[data-message-id="global-target-message"]')
          ?.getAttribute('data-search-current'),
      ).toBe('true');
    });
  });

  it('shows response duration on agent messages', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-time', 2),
      messages: [
        { id: 'u1', role: 'User', channel: 'main', content: 'Question', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a1', role: 'Agent', channel: 'main', content: 'Answer', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:01:23Z', tokens_used: 100, auth_mode: null },
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
        { id: 'u1', role: 'User', channel: 'main', content: 'https://example.com/very-long-url-that-should-not-break-the-bubble-layout/with/many/path/segments/and-no-spaces-at-all', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a1', role: 'Agent', channel: 'main', content: 'Here is the response', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:05Z', tokens_used: 50, auth_mode: null },
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
        { id: 'u1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a1', role: 'Agent', channel: 'main', content: 'Hi', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:05Z', tokens_used: 50, auth_mode: null },
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

    // The button carries the configured agent under its canonical mention plus its
    // role, so the same identity reads the same way here, in the composer and in
    // the participant list.
    const switchBtn = screen.getByRole('button', { name: "Changer d'agent ou de mode IA" });
    expect(switchBtn).toBeTruthy();
    expect(switchBtn?.textContent).toContain('@claude');
    expect(switchBtn?.textContent).toContain('agent de discussion');
  });

  // ─── Global discussion search entry point ────────────────────────────

  it('search input exists and does not remount the local tree while composing', async () => {
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
    const searchInput = document.querySelector(
      'input[placeholder="Chercher dans tous les messages…"]',
    ) as HTMLInputElement;
    expect(searchInput).toBeTruthy();

    // Type "Alpha" in the search
    await act(async () => { fireEvent.change(searchInput, { target: { value: 'Alpha' } }); });

    // The primary field runs the bounded backend search on Enter. Merely
    // composing must keep the canonical tree stable, especially with hundreds
    // of discussions.
    const bodyAfter = document.body.textContent!;
    expect(bodyAfter).toContain('Alpha project chat');
    expect(bodyAfter).toContain('Beta refactoring');
  });

  // ─── Agent switch dropdown tests ─────────────────────────────────────

  it('switches the discussion agent and mode silently without launching a response', async () => {
    const fullDisc: Discussion = {
      ...makeListDiscussion('d-dropdown', 2),
      messages: [
        { id: 'u1', role: 'User', channel: 'main', content: 'Hello', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'a1', role: 'Agent', channel: 'main', content: 'Hi', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:05Z', tokens_used: 50, auth_mode: null },
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
    const switchBtn = screen.getByRole('button', { name: "Changer d'agent ou de mode IA" });
    expect(switchBtn).toBeTruthy();
    await act(async () => { fireEvent.click(switchBtn); });

    // The dropdown should now show all installed agents (using display names)
    const body = document.body.textContent!;
    expect(body).toContain('Claude Code');
    expect(body).toContain('Codex');
    expect(body).toContain('Gemini CLI');

    vi.mocked(discussionsApi.update).mockClear();
    vi.mocked(discussionsApi.runAgent).mockClear();
    await act(async () => {
      fireEvent.click(screen.getByRole('menuitem', { name: 'Codex · Standard' }));
    });

    await waitFor(() => {
      expect(vi.mocked(discussionsApi.update))
        .toHaveBeenCalledWith('d-dropdown', { agent: 'Codex', tier: 'default' });
    });
    expect(vi.mocked(discussionsApi.runAgent)).not.toHaveBeenCalled();
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
      return { connected: true, connectionState: 'connected' };
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
    vi.mocked(useWebSocket).mockImplementation(() => ({ connected: false, connectionState: 'connecting' }));
  });

  it('flips sendingMap[disc] ON when a batch_run_child_started WS event arrives', async () => {
    // Batch children run server-side with no SSE consumer on the client, so
    // this WS event is the ONLY signal that an agent began — it must set the
    // per-disc spinner indicator on (sidebar pill + open chat view).
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    vi.mocked(useWebSocket).mockImplementation((onMessage) => {
      setTimeout(() => {
        onMessage({ type: 'batch_run_child_started', run_id: 'run-1', discussion_id: 'd1' });
      }, 10);
      return { connected: true, connectionState: 'connected' };
    });

    const setSendingMap = vi.fn();
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
        setSendingMap={setSendingMap}
      />
    );
    await act(async () => { await new Promise(r => setTimeout(r, 100)); });

    // At least one setSendingMap updater must turn d1 on.
    const turnedOnD1 = setSendingMap.mock.calls.some(
      ([arg]) => typeof arg === 'function' && arg({}).d1 === true,
    );
    expect(turnedOnD1).toBe(true);

    vi.mocked(useWebSocket).mockImplementation(() => ({ connected: false, connectionState: 'connecting' }));
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
    ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
    write_access: { status: 'Writable' },
    mcp_sync_report: null,
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
      { id: 'm1', role: 'User', channel: 'main', content: 'Tell me about my project', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
    ],
    message_count: 1, non_system_message_count: 1, tier: "default" as const, summary_strategy: "Auto" as const, introspection_call_count: 0,
    archived: false, pinned: false, pin_first_message: false,
    workspace_mode: 'Direct',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    awaiting_agent: false,
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
    // intro) but didn't run the 9-step audit — the user still needs
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

  it('validation CTA validates, records the Audit deep-link, refreshes, and opens the project', async () => {
    const proj = makeProject('p-validation', 'Audited', 'context');
    const disc: Discussion = {
      ...makeProjectDisc('d-validation', proj.id, 'Validation audit AI'),
      messages: [
        { id: 'm-user', role: 'User', channel: 'main', content: 'Validate', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
        { id: 'm-agent', role: 'Agent', channel: 'main', content: 'KRONN:VALIDATION_COMPLETE', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:01:00Z', tokens_used: 10, auth_mode: null },
      ],
      message_count: 2,
      non_system_message_count: 2,
    };
    vi.mocked(discussionsApi.get).mockResolvedValue(disc);
    const onNavigate = vi.fn();
    const refetchProjects = vi.fn();
    const refetchDiscussions = vi.fn();

    await wrap(
      <DiscussionsPage
        projects={[proj]}
        agents={[]}
        allDiscussions={[disc]}
        configLanguage="fr"
        agentAccess={null}
        refetchDiscussions={refetchDiscussions}
        refetchProjects={refetchProjects}
        onNavigate={onNavigate}
        toast={toastFn}
        initialActiveDiscussionId={disc.id}
        {...liftedProps()}
      />,
    );

    fireEvent.click(await screen.findByRole('button', { name: /Marquer l'audit comme valide/i }));
    await waitFor(() => expect(projectsApi.validateAudit).toHaveBeenCalledWith(proj.id));
    expect(sessionStorage.getItem(`kronn:projectView:${proj.id}`)).toBe('audit');
    expect(refetchProjects).toHaveBeenCalled();
    expect(refetchDiscussions).toHaveBeenCalled();
    expect(onNavigate).toHaveBeenCalledWith('projects', { projectId: proj.id });
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
