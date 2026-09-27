import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, act, cleanup } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type * as MessageBubbleModule from '../../components/MessageBubble';

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

// Counts renders of each bubble; the real component still renders inside.
const bubbleRenders = vi.hoisted(() => new Map<string, number>());
vi.mock('../../components/MessageBubble', async importOriginal => {
  const actual = await importOriginal<typeof MessageBubbleModule>();
  const { memo, createElement } = await import('react');
  const Counted = memo((props: Parameters<typeof actual.MessageBubble>[0]) => {
    bubbleRenders.set(props.msg.id, (bubbleRenders.get(props.msg.id) ?? 0) + 1);
    return createElement(actual.MessageBubble, props);
  });
  return { ...actual, MessageBubble: Counted };
});

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
  // KT-619 — the composer asks for a publication proof before every send that
  // carries a card. Absent from this mock, the property access alone throws and
  // every send in this file dies before `sendMessageStream`.
  publicationCredentials: {
    proof: vi.fn().mockResolvedValue('proof-test'),
    list: vi.fn().mockResolvedValue([]),
    enrol: vi.fn(),
    revoke: vi.fn(),
    rotate: vi.fn(),
    rotateAdmin: vi.fn(),
  },
  discussions: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn().mockResolvedValue(null),
    poll: vi.fn(),
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
    contextFileBlob: vi.fn().mockResolvedValue(new Blob()),
    // 0.9.2 — the composer lists joined CLI sessions to offer their `-cli` aliases.
    participants: vi.fn().mockResolvedValue([]),
    // KT-595 — the arbitration banner reads the room's questions on mount.
    questions: vi.fn().mockResolvedValue({ questions: [], pending_count: 0 }),
    importantMessages: vi.fn().mockResolvedValue({ items: [], total: 0, total_all: 0 }),
    answerQuestion: vi.fn(),
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
  // KT-476 — the message list fetches proposed inline actions per discussion.
  discussionActions: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    cancel: vi.fn(),
    launch: vi.fn(),
  },
  config: {
    getUiLanguage: vi.fn().mockResolvedValue('fr'),
    saveUiLanguage: vi.fn().mockResolvedValue(undefined),
    // 0.8.6 phase 4 — NewDiscussionForm fetches the default tier on mount.
    getServerConfig: vi.fn().mockResolvedValue({ default_model_tier: 'default' }),
  },
  media: {
    capabilities: vi.fn().mockResolvedValue({ model: 'image-model', capabilities: null }),
    estimate: vi.fn().mockResolvedValue({ model: 'image-model', estimated_usd: null, samples: 0 }),
    generate: vi.fn(),
  },
  // KT-531 — AgentSwitchPicker reads the dynamic model catalog when its
  // popover opens.
  modelCatalogApi: {
    list: vi.fn().mockResolvedValue({ targets: [] }),
  },
}));

// Mock useWebSocket hook (WS not available in jsdom)
vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));

import {
  discussions as discussionsApi,
  planning as planningApi,
  projects as projectsApi,
  runsApi,
} from '../../lib/api';
import { DiscussionsPage } from '../DiscussionsPage';
import { discardImportantDraft } from '../../lib/importantMessageDraft';
import type { Discussion } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';

const noop = () => {};
const toastFn: ToastFn = vi.fn();

beforeEach(() => {
  for (const id of ['d-no-native-provider', 'd-important-queued', 'd-important-other']) discardImportantDraft(id);
  vi.mocked(discussionsApi.nativeAgentMode).mockReset();
  vi.mocked(discussionsApi.nativeAgentMode).mockResolvedValue({ disabled: false });
  vi.mocked(discussionsApi.get).mockReset();
  // The periodic refresh reads the same detail through `poll`; a fresh revision
  // each time keeps these tests on the full-detail path they were written for.
  let pollRevision = 0;
  vi.mocked(discussionsApi.poll).mockReset();
  vi.mocked(discussionsApi.poll).mockImplementation(async id => {
    const detail = await discussionsApi.get(id);
    return detail ? { revision: `r${++pollRevision}`, detail } as never : null as never;
  });
  vi.mocked(discussionsApi.searchMessages).mockReset();
  vi.mocked(discussionsApi.searchMessages).mockResolvedValue([]);
  vi.mocked(discussionsApi.listContextFiles).mockReset();
  vi.mocked(discussionsApi.listContextFiles).mockResolvedValue([]);
  vi.mocked(discussionsApi.deleteMessage).mockReset();
  vi.mocked(discussionsApi.deleteMessage).mockResolvedValue(undefined);
  vi.mocked(runsApi.list).mockReset();
  vi.mocked(runsApi.list).mockResolvedValue([]);
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
  // KT-581 — the page remembers which panel was last open, and the test store
  // is shared across specs in this file.
  localStorage.removeItem('kronn:lastPanel');
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
  tier: "default" as const, summary_strategy: "OnDemand" as const, introspection_call_count: 0,
  workspace_mode: 'Direct',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  awaiting_agent: false,
});

describe('DiscussionsPage render stability', () => {
  it('does not re-render the bubbles of an unchanged transcript', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      bubbleRenders.clear();
      const fullDisc: Discussion = {
        ...makeListDiscussion('d-stable', 3),
        messages: [
          { id: 's1', role: 'User', channel: 'main', content: 'first question', agent_type: null, timestamp: '2026-01-01T00:00:00Z', tokens_used: 0, auth_mode: null },
          { id: 's2', role: 'Agent', channel: 'main', content: '**first answer**', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:01Z', tokens_used: 1, auth_mode: null },
          { id: 's3', role: 'User', channel: 'main', content: 'a follow-up', agent_type: null, timestamp: '2026-01-01T00:00:02Z', tokens_used: 0, auth_mode: null, reply_to_message_id: 's2' },
        ],
      };
      vi.mocked(discussionsApi.poll).mockReset();
      vi.mocked(discussionsApi.poll)
        .mockResolvedValueOnce({ revision: 'rev-1', detail: fullDisc } as never)
        .mockResolvedValue({ revision: 'rev-1', detail: null } as never);
      const props = {
        projects: [], agents: [], allDiscussions: [makeListDiscussion('d-stable', 3)], configLanguage: 'fr',
        agentAccess: null, refetchDiscussions: noop, refetchProjects: noop, toast: toastFn,
        initialActiveDiscussionId: 'd-stable', ...liftedProps(),
      };
      const view = await wrap(<DiscussionsPage {...props} onNavigate={() => {}} />);
      expect(await screen.findByText('a follow-up')).toBeInTheDocument();
      const settled = new Map(bubbleRenders);
      expect(settled.get('s2')).toBeGreaterThan(0);

      // A parent re-render with fresh callback identities, then idle refreshes
      // that report no change: no bubble may render again.
      await act(async () => {
        view.rerender(<I18nProvider><DiscussionsPage {...props} onNavigate={() => {}} /></I18nProvider>);
      });
      await act(async () => { vi.advanceTimersByTime(10_000); });
      expect(vi.mocked(discussionsApi.poll)).toHaveBeenCalledWith('d-stable', 'rev-1');
      expect(new Map(bubbleRenders)).toEqual(settled);

      // A real change still reaches the screen.
      vi.mocked(discussionsApi.poll).mockResolvedValue({
        revision: 'rev-2',
        detail: { ...fullDisc, messages: [...fullDisc.messages, { id: 's4', role: 'Agent', channel: 'main', content: 'new reply', agent_type: 'ClaudeCode', timestamp: '2026-01-01T00:00:03Z', tokens_used: 1, auth_mode: null }] },
      } as never);
      await act(async () => { vi.advanceTimersByTime(5_000); });
      expect(await screen.findByText('new reply')).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});
