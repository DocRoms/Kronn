import { useState, useRef, useEffect, useCallback, useMemo, useDeferredValue } from 'react';
import './DiscussionsPage.css';
import { MessageBubble, MarkdownContent } from '../components/MessageBubble';
import { unseenBasis } from '../components/SwipeableDiscItem';
import { ToolCallsGroup } from '../components/ToolCallsGroup';
import { groupMessagesWithToolFold } from '../lib/discussionMessageGrouping';
import { ChatInput } from '../components/ChatInput';
import { discussions as discussionsApi, projects as projectsApi, skills as skillsApi, profiles as profilesApi, directives as directivesApi, contacts as contactsApi, workflows as workflowsApi, quickPrompts as quickPromptsApi } from '../lib/api';
import { GitPanel } from '../components/GitPanel';
import { TestModeBanner } from '../components/TestModeBanner';
import { TestModeModal } from '../components/TestModeModal';
import type { TestModeBlocker } from '../types/extensions';
import { ChatHeader } from '../components/ChatHeader';
import { DiscussionSidebar } from '../components/DiscussionSidebar';
import { NewDiscussionForm } from '../components/NewDiscussionForm';
import type { NewDiscConfig } from '../components/NewDiscussionForm';
import { AgentQuestionForm } from '../components/AgentQuestionForm';
import { parseAgentQuestions } from '../lib/agent-question-parse';
import { userError } from '../lib/userError';
import { getDeployedVersion, setDeployedVersion } from '../lib/qp-improver-banner';
import { sanitizeQpImproverPayload } from '../lib/qp-improver-sanitize';
import type { Project, AgentDetection, Discussion, AgentType, AgentsConfig, Skill, AgentProfile, Directive, McpConfigDisplay, McpIncompatibility, Contact, WsMessage, ContextFile, BatchRunSummary } from '../types/generated';
import { useWebSocket } from '../hooks/useWebSocket';
import { useQpChain } from '../hooks/useQpChain';
import { useMessageQueue } from '../hooks/useMessageQueue';
import { useRafBatchedStream } from '../hooks/useRafBatchedStream';
import { buildStreamingFlush } from '../lib/stream-flush';
import { findLastAgentMessage } from '../lib/discussionHelpers';
import { saveDraft } from '../lib/chat-drafts';
import { buildBatchTriageRows, buildContinuationDraft, type BatchTriageRow } from '../lib/batchTriage';
import { useT } from '../lib/I18nContext';
import { AGENT_LABELS, agentColor, isAgentRestricted as isAgentRestrictedUtil, hasAgentFullAccess, getProjectGroup, isUsable, isBriefingDisc, isBootstrapDisc, isValidationDisc } from '../lib/constants';
import type { ToastFn } from '../hooks/useToast';
import {
  ChevronRight, Cpu, Loader2,
  MessageSquare, AlertTriangle,
  ShieldCheck, Check, Rocket, Play, Zap,
  Menu, X, Clock, ExternalLink,
} from 'lucide-react';
import { useIsMobile } from '../hooks/useMediaQuery';

export interface DiscussionsPageProps {
  projects: Project[];
  agents: AgentDetection[];
  allDiscussions: Discussion[];
  configLanguage: string | null;
  agentAccess: AgentsConfig | null;
  refetchDiscussions: () => void;
  refetchProjects: () => void;
  onNavigate: (page: string, opts?: { projectId?: string; scrollTo?: string; workflowId?: string }) => void;
  prefill?: { projectId: string; title: string; prompt: string; locked?: boolean } | null;
  initialActiveDiscussionId?: string | null;
  onPrefillConsumed?: () => void;
  /** Lets a banner inside DiscussionsPage seed a NewDiscussionForm
   *  prefill on a sibling render — e.g. the post-bootstrap "Start dev
   *  on issue #1" CTA needs to open the new-disc dialog with a
   *  prefilled prompt. Symmetric with the existing path used by the
   *  Projects page audit-validation CTA. */
  onSetDiscPrefill?: (p: { projectId: string; title: string; prompt: string; locked?: boolean }) => void;
  /** Auto-open an existing discussion and trigger agent run (used after full audit) */
  autoRunDiscussionId?: string | null;
  onAutoRunConsumed?: () => void;
  /** Open a specific discussion without triggering agent (e.g. Resume Validation) */
  openDiscussionId?: string | null;
  onOpenDiscConsumed?: () => void;
  /** When clicking "📋 N conversations" on a workflow run, the parent passes
   *  the batch run id here. We auto-uncollapse the matching project + batch
   *  group in the sidebar and scroll to it, then ack via onFocusBatchConsumed
   *  so the same id doesn't re-trigger on every render. */
  focusBatchId?: string | null;
  onFocusBatchConsumed?: () => void;
  toast: ToastFn;
  // Lifted streaming state (lives in Dashboard, survives page changes)
  sendingMap: Record<string, boolean>;
  setSendingMap: React.Dispatch<React.SetStateAction<Record<string, boolean>>>;
  sendingStartMap: Record<string, number>;
  setSendingStartMap: React.Dispatch<React.SetStateAction<Record<string, number>>>;
  streamingMap: Record<string, string>;
  setStreamingMap: React.Dispatch<React.SetStateAction<Record<string, string>>>;
  /** Watchdog tick (TD-20260504) — call from chunk handlers so the
   *  Dashboard-side stale-stream detector knows the channel is alive. */
  noteStreamTick: (discId: string) => void;
  abortControllers: React.MutableRefObject<Record<string, AbortController>>;
  cleanupStream: (discId: string) => void;
  // Lifted unseen tracking (lives in Dashboard for cross-page visibility)
  markDiscussionSeen: (discId: string, msgCount: number) => void;
  /** 0.8.3 (#277) — bulk-seed every discussion's last-seen counter to
   *  its current `message_count`. Wired to the sidebar "Mark all as
   *  read" button so users can purge accumulated unread backlogs from
   *  pre-feature discussions / archived discs / batch children in one
   *  click. Lives in Dashboard alongside `markDiscussionSeen` so both
   *  use the same `lastSeenMsgCount` state + localStorage entry.
   *  Optional so existing tests that pre-date this feature don't need
   *  to be updated — the sidebar simply hides the button when no
   *  handler is wired. */
  markAllDiscussionsSeen?: () => void;
  onActiveDiscussionChange: (id: string | null) => void;
  lastSeenMsgCount: Record<string, number>;
  mcpConfigs?: McpConfigDisplay[];
  mcpIncompatibilities?: McpIncompatibility[];
  /** 0.8.2 — Bubbles up "open the workflow wizard with this preset
   *  pre-applied" from the validation-complete CTA. Dashboard sets the
   *  pending preset state + flips the page to Workflows. */
  onLaunchWorkflowFromPreset?: (presetId: string, projectId: string) => void;
}

// ─── TTS imports ──
import { speakText, stopTts, pauseTts, resumeTts, isTtsPaused } from '../lib/tts-engine';

// Stable empty array so messages with no attachments keep the same prop
// reference across renders (don't bust MessageBubble's memo).
const EMPTY_ATTACHMENTS: ContextFile[] = [];

let ttsWorker: Worker | null = null;
function getTtsWorker(): Worker {
  if (!ttsWorker) {
    ttsWorker = new Worker(
      new URL('../lib/tts-worker.ts', import.meta.url),
      { type: 'module' }
    );
  }
  return ttsWorker;
}

export function DiscussionsPage({
  projects,
  agents,
  allDiscussions,
  configLanguage,
  agentAccess,
  refetchDiscussions,
  refetchProjects,
  onNavigate,
  prefill,
  onPrefillConsumed,
  onSetDiscPrefill,
  autoRunDiscussionId,
  onAutoRunConsumed,
  openDiscussionId,
  onOpenDiscConsumed,
  focusBatchId,
  onFocusBatchConsumed,
  toast,
  sendingMap,
  setSendingMap,
  sendingStartMap,
  setSendingStartMap,
  streamingMap,
  setStreamingMap,
  noteStreamTick,
  abortControllers,
  cleanupStream: cleanupStreamBase,
  markDiscussionSeen,
  markAllDiscussionsSeen,
  onActiveDiscussionChange,
  lastSeenMsgCount,
  initialActiveDiscussionId,
  mcpConfigs = [],
  mcpIncompatibilities = [],
  onLaunchWorkflowFromPreset,
}: DiscussionsPageProps) {
  const { t } = useT();
  const isMobile = useIsMobile();

  // ─── Internal state ──────────────────────────────────────────────────────
  const [sidebarOpen, setSidebarOpen] = useState(true); // always start open; mobile auto-closes on select
  const [sidebarCollapsed, setSidebarCollapsed] = useState<boolean>(() => {
    try { return localStorage.getItem('kronn:sidebarCollapsed') === 'true'; } catch { return false; }
  });
  const [activeDiscussionId, setActiveDiscussionId] = useState<string | null>(initialActiveDiscussionId ?? null);
  const [showNewDiscussion, setShowNewDiscussion] = useState(false);
  const [showGitPanel, setShowGitPanel] = useState(false);
  // 0.8.3 (#280) — flag set when an audit is running on the same
  // project as the active discussion. Drives the banner that warns
  // the user "MCPs are temporarily filtered for audit perf — re-run
  // your query after the audit if you need the full set". Polled
  // every 8 s while the discussion is open AND on a project with no
  // current audit signal yet (cheap GET; stops polling once an audit
  // is detected and again polls to confirm completion).
  const [auditRunningOnActiveProject, setAuditRunningOnActiveProject] = useState(false);
  const [pendingFilesCount, setPendingFilesCount] = useState(0);
  // Test mode (worktree swap-in-main flow). `busy` guards against
  // double-clicks on enter/exit which would race with the backend's
  // rollback logic. The blocker state drives the preflight modal that
  // lets the user pick stash-and-proceed / commit-first / cancel.
  const [testModeBusy, setTestModeBusy] = useState(false);
  const [testModeBlocker, setTestModeBlocker] = useState<TestModeBlocker | null>(null);
  const [testModePendingDiscId, setTestModePendingDiscId] = useState<string | null>(null);
  const [editingMsgId, setEditingMsgId] = useState<string | null>(null);
  // Race-free guard for handleEditMessage. A fast double Ctrl+Enter on
  // the message-edit textarea (the only re-submit path) was reading
  // `sending` from a stale closure → both invocations called
  // `deleteLastAgentMessages` + `editLastUserMessage` + `runAgent` in
  // parallel, producing duplicate user edits and parallel agent runs.
  const editingMsgInFlightRef = useRef(false);
  // 0.8.4 follow-up — QP improver deploy CTA needs (a) a busy state to
  // disable the button + show "Déploiement en cours…" while the PUT
  // round-trips, and (b) a useRef guard against a fast double-click
  // (the useState is closure-stale for ~1 paint after click). Pattern
  // mirrors `editingMsgInFlightRef` above. Cf. [[feedback_race_guards]].
  const deployingQpRef = useRef(false);
  const [deployingQpDiscId, setDeployingQpDiscId] = useState<string | null>(null);
  const [editingText, setEditingText] = useState('');
  // Per-discussion override of the agent shown in the streaming placeholder.
  // Set when the user pings `@codex` (or any other agent) instead of the
  // discussion's default — the bubble must say "Codex" while it spins,
  // not the default agent. Cleared when the stream ends.
  const [streamingTargetMap, setStreamingTargetMap] = useState<Record<string, AgentType>>({});
  const [collapsedDiscGroups, setCollapsedDiscGroups] = useState<Set<string>>(() => {
    try {
      const saved = localStorage.getItem('kronn:discCollapsedGroups');
      return saved ? new Set(JSON.parse(saved) as string[]) : new Set();
    } catch { return new Set(); }
  });
  const [orchState, setOrchState] = useState<Record<string, {
    active: boolean;
    round: number | string;
    totalRounds: number;
    currentAgent: string | null;
    agentStreams: { agent: string; agentType: string; round: number | string; text: string; done: boolean }[];
    systemMessages: string[];
  }>>({});
  const [availableSkills, setAvailableSkills] = useState<Skill[]>([]);
  const [availableProfiles, setAvailableProfiles] = useState<AgentProfile[]>([]);
  const [availableDirectives, setAvailableDirectives] = useState<Directive[]>([]);
  const [expandedSummaryMsgId, setExpandedSummaryMsgId] = useState<string | null>(null);
  const [worktreeError, setWorktreeError] = useState<string | null>(null);

  const [contextFilesMap, setContextFilesMap] = useState<Record<string, ContextFile[]>>({});
  const [uploadingFiles, setUploadingFiles] = useState(false);
  const [contactsList, setContactsList] = useState<Contact[]>([]);
  const [contactsOnline, setContactsOnline] = useState<Record<string, boolean>>({});
  // message_ids whose federated attachment is announced but not yet
  // fetched/linked (F15+) → render a "downloading…" placeholder on the bubble
  // until the file lands. Set by the `file_attached{pending}` WS events.
  const [pendingFileMsgIds, setPendingFileMsgIds] = useState<Set<string>>(() => new Set());
  // Batch run summaries — feeds the sidebar pastille that links a batch group
  // back to the workflow run that spawned it. Refetched on batch WS events
  // (see handleWsMessage below) so newly-finished batches pick up their
  // parent_run_sequence label without a full page reload.
  const [batchSummaries, setBatchSummaries] = useState<BatchRunSummary[]>([]);
  const [batchReview, setBatchReview] = useState<{ runId: string; label: string; discIds: string[] } | null>(null);
  const [batchReviewDiscs, setBatchReviewDiscs] = useState<Discussion[]>([]);
  const [batchReviewLoading, setBatchReviewLoading] = useState(false);
  const [batchReviewError, setBatchReviewError] = useState<string | null>(null);
  const refetchBatchSummaries = useCallback(() => {
    workflowsApi.listBatchRunSummaries()
      .then(setBatchSummaries)
      .catch((e) => {
        // Log so silent API/network failures stop manifesting as
        // "batch groups have no parent pastille" without any signal.
        console.warn('Failed to load batch run summaries:', e);
      });
  }, []);
  const openBatchReview = useCallback(async (runId: string, label: string, discIds: string[]) => {
    setBatchReview({ runId, label, discIds });
    setBatchReviewDiscs([]);
    setBatchReviewError(null);
    setBatchReviewLoading(true);
    try {
      const loaded = await Promise.all(discIds.map(id => discussionsApi.get(id)));
      setBatchReviewDiscs(loaded);
    } catch (e) {
      setBatchReviewError(userError(e));
    } finally {
      setBatchReviewLoading(false);
    }
  }, []);
  const [copiedMsgId, setCopiedMsgId] = useState<string | null>(null);
  const [ttsEnabled, setTtsEnabled] = useState<boolean>(() => {
    try { return localStorage.getItem('kronn:ttsEnabled') === 'true'; } catch { return false; }
  });
  const [ttsState, setTtsState] = useState<'idle' | 'loading' | 'playing' | 'paused'>('idle');
  const [ttsPlayingMsgId, setTtsPlayingMsgId] = useState<string | null>(null);
  const [sendingElapsed, setSendingElapsed] = useState(0);
  const sendingTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const [agentLogs, setAgentLogs] = useState<string[]>([]);
  const [showLogs, setShowLogs] = useState(false);
  const onAgentLog = useCallback((log: string) => setAgentLogs(prev => [...prev.slice(-50), log]), []);
  const resetAgentLogs = useCallback(() => { setAgentLogs([]); setShowLogs(false); }, []);
  const chatEndRef = useRef<HTMLDivElement>(null);
  const messagesContainerRef = useRef<HTMLDivElement>(null);
  // True when the user is reading higher up in the message log. We freeze
  // the auto-scroll behavior so the streaming output doesn't yank the
  // scroll position back to the bottom every chunk. Re-enabled the moment
  // the user manually scrolls back near the bottom.
  const [stickToBottom, setStickToBottom] = useState(true);
  const [hasNewWhileScrolledUp, setHasNewWhileScrolledUp] = useState(false);
  // Scroll-position state for the top/bottom jump arrows. Updated by
  // `handleMessagesScroll` together with `stickToBottom`. We only render
  // an arrow when the user actually *can* go further in that direction —
  // the buttons stay out of the way on short discussions.
  const [scrolledFromTop, setScrolledFromTop] = useState(false);

  // Persist sidebar collapse state to localStorage
  useEffect(() => {
    localStorage.setItem('kronn:discCollapsedGroups', JSON.stringify([...collapsedDiscGroups]));
  }, [collapsedDiscGroups]);
  useEffect(() => {
    localStorage.setItem('kronn:sidebarCollapsed', String(sidebarCollapsed));
  }, [sidebarCollapsed]);

  // Persist TTS preference
  useEffect(() => {
    localStorage.setItem('kronn:ttsEnabled', String(ttsEnabled));
  }, [ttsEnabled]);

  // Stop TTS when switching conversations
  useEffect(() => {
    stopTts();
    setTtsState('idle');
    setTtsPlayingMsgId(null);
  }, [activeDiscussionId]);

  // Orchestration chunk buffer (same rAF pattern as streaming)
  const orchChunkBuffer = useRef<Record<string, string>>({});
  const orchRafId = useRef<number | null>(null);

  // Batched streaming: accumulate chunks in a ref, flush to state via rAF.
  // Extracted to hooks/useRafBatchedStream.ts — collapses dozens of SSE
  // deltas per frame into a single React state update.
  // Flush logic (merge + watchdog tick) lives in `lib/stream-flush.ts`
  // for unit-test coverage of the streaming contract.
  const appendStreamChunk = useRafBatchedStream(
    buildStreamingFlush(setStreamingMap, noteStreamTick),
  );

  // Cache of fully-loaded discussions (with messages)
  const [loadedDiscussions, setLoadedDiscussions] = useState<Record<string, Discussion>>({});

  // Fetch full discussion (with messages) when active discussion changes
  // or when sending finishes (to pick up the agent's response).
  // 0.8.6 phase 3 — also poll every 5 s so messages posted by OTHER
  // agents (via the multi-agent collab `disc_append` flow) show up
  // without the user having to switch discussions and back. Cheap :
  // a single SELECT on the indexed messages table. Will be replaced
  // by SSE when DiscMessageAppended events are plumbed through the
  // existing ws_broadcast pipeline.
  const activeSending = activeDiscussionId ? !!sendingMap[activeDiscussionId] : false;
  useEffect(() => {
    if (!activeDiscussionId) return;
    let cancelled = false;
    const fetchActive = () => {
      discussionsApi.get(activeDiscussionId).then(disc => {
        if (!cancelled && disc) {
          setLoadedDiscussions(prev => ({ ...prev, [disc.id]: disc }));
        }
      }).catch(() => { /* ignore fetch errors */ });
    };
    fetchActive();
    const id = setInterval(fetchActive, 5000);
    return () => { cancelled = true; clearInterval(id); };
  }, [activeDiscussionId, activeSending]);

  // Clear worktree error when switching discussions
  useEffect(() => { setWorktreeError(null); }, [activeDiscussionId]);

  // ─── Derived data ────────────────────────────────────────────────────────
  const activeDiscussion = (activeDiscussionId && loadedDiscussions[activeDiscussionId])
    ? loadedDiscussions[activeDiscussionId]
    : allDiscussions.find(d => d.id === activeDiscussionId) ?? null;
  const batchReviewRows = useMemo(
    () => buildBatchTriageRows(batchReviewDiscs),
    [batchReviewDiscs],
  );

  const continueBatchTriage = useCallback((row: BatchTriageRow) => {
    const draft = buildContinuationDraft(row);
    saveDraft(row.discussion.id, draft);
    setActiveDiscussionId(row.discussion.id);
    setBatchReview(null);
    setSidebarOpen(false);
    toast(t('disc.batchReviewDraftReady'), 'success');
  }, [toast, t]);

  const activeAgentDisabled = useMemo(() => {
    if (!activeDiscussion || agents.length === 0) return false;
    const agentDet = agents.find(a => a.agent_type === activeDiscussion.agent);
    return !agentDet || !isUsable(agentDet);
  }, [activeDiscussion, agents]);

  const sending = activeDiscussionId ? !!sendingMap[activeDiscussionId] : false;
  const streamingText = activeDiscussionId ? (streamingMap[activeDiscussionId] ?? '') : '';
  // Deferred value for markdown rendering — every SSE chunk pushes a new
  // streamingText, and ReactMarkdown re-parses the whole buffer each time.
  // For long responses (5000+ chars), each parse takes 5-20 ms × 30 fps =
  // saturated main thread. useDeferredValue lets the input/scroll handlers
  // run first; the markdown re-parse happens at lower priority and may be
  // interrupted if a new chunk arrives. Visible lag is ~1 frame at worst.
  const deferredStreamingText = useDeferredValue(streamingText);

  // Auto-read new agent responses when TTS is enabled
  const prevMsgCountRef = useRef(-1);
  useEffect(() => {
    if (!activeDiscussion) { prevMsgCountRef.current = -1; return; }
    const msgs = activeDiscussion.messages;
    // Skip the first render (initialize the ref) — only speak on subsequent updates
    if (prevMsgCountRef.current < 0) {
      prevMsgCountRef.current = msgs.length;
      return;
    }
    if (ttsEnabled && msgs.length > prevMsgCountRef.current) {
      const newMsgs = msgs.slice(prevMsgCountRef.current);
      const lastAgent = findLastAgentMessage(newMsgs);
      if (lastAgent && !sending) {
        const autoId = lastAgent.id;
        setTtsPlayingMsgId(autoId);
        setTtsState('loading');
        speakText(getTtsWorker, lastAgent.content, activeDiscussion?.language || 'fr', () => setTtsState('playing'))
          .finally(() => {
            setTtsPlayingMsgId(cur => {
              if (cur === autoId && !isTtsPaused()) { setTtsState('idle'); return null; }
              return cur;
            });
          });
      }
    }
    prevMsgCountRef.current = msgs.length;
  }, [activeDiscussion?.messages.length, ttsEnabled, sending]);

  // ─── Agent access helpers (shared from constants.ts) ─────────────────────
  const isAgentRestricted = useCallback((agentType: AgentType): boolean =>
    isAgentRestrictedUtil(agentAccess ?? undefined, agentType), [agentAccess]);

  const hasFullAccess = useCallback((agentType: AgentType): boolean =>
    hasAgentFullAccess(agentAccess ?? undefined, agentType), [agentAccess]);

  // ─── Effects ─────────────────────────────────────────────────────────────

  // NOTE: Do NOT abort SSE controllers on unmount.
  // The SSE callbacks use Dashboard state setters (sendingMap, streamingMap)
  // which survive page switches. Aborting here would kill in-flight agent
  // streams when the user simply switches tabs, causing the "thinking" loader
  // to disappear. Controllers are cleaned up by cleanupStream (on SSE done)
  // or by the explicit Stop button (handleStop).

  // Fetch available skills, profiles, directives & contacts.
  // Re-fetch profiles on secret-code unlock so Batman shows up in
  // ChatHeader without a page reload.
  useEffect(() => {
    const refetchProfiles = () => profilesApi.list().then(setAvailableProfiles).catch(() => {});
    skillsApi.list().then(setAvailableSkills).catch(() => {});
    refetchProfiles();
    directivesApi.list().then(setAvailableDirectives).catch(() => {});
    contactsApi.list().then(setContactsList).catch(() => {});
    refetchBatchSummaries();
    window.addEventListener('kronn:profiles-changed', refetchProfiles);
    return () => window.removeEventListener('kronn:profiles-changed', refetchProfiles);
  }, [refetchBatchSummaries]);

  // WebSocket-based real-time events (presence, chat, invites)
  const handleWsMessage = useCallback((msg: WsMessage) => {
    if (msg.type === 'presence') {
      const contact = contactsList.find(c => c.invite_code === msg.from_invite_code);
      if (contact) {
        setContactsOnline(prev => ({ ...prev, [contact.id]: msg.online }));
      }
    }
    // Remote peer sent a message in a shared discussion → reload it
    if (msg.type === 'chat_message') {
      // If we're viewing this discussion, reload to show the new message
      refetchDiscussions();
      if (activeDiscussionId) {
        reloadDiscussion(activeDiscussionId);
      }
    }
    // A federated file finished landing (announced, then fetched + linked to its
    // message) → reload so the attachment renders WITHOUT waiting for the next
    // message. The file federates slightly after its ChatMessage, so the reload
    // on `chat_message` above fired before the file was ready (the "image
    // n'apparaît qu'après un 2e message" glitch).
    if (msg.type === 'file_attached') {
      if (msg.pending) {
        // Announced, binary not fetched yet → flag the message so its bubble
        // shows "📎 téléchargement…" instead of nothing, then reload so the
        // message itself is visible during the download.
        setPendingFileMsgIds(prev => {
          if (prev.has(msg.message_id)) return prev;
          const next = new Set(prev); next.add(msg.message_id); return next;
        });
      } else {
        // Binary fetched + linked → clear the placeholder; the reload below
        // surfaces the real attachment.
        setPendingFileMsgIds(prev => {
          if (!prev.has(msg.message_id)) return prev;
          const next = new Set(prev); next.delete(msg.message_id); return next;
        });
      }
      refetchDiscussions();
      if (activeDiscussionId) {
        reloadDiscussion(activeDiscussionId);
      }
    }
    // Remote peer shared a discussion with us → refresh list
    if (msg.type === 'discussion_invite') {
      refetchDiscussions();
      toast(t('contacts.sharedByPeer', msg.from_pseudo, msg.title), 'info');
    }
    // Batch workflow run finished — show a toast + refresh the disc list so
    // the sidebar group pill updates from "⏳ 7/12" to "✓ 12/12".
    if (msg.type === 'batch_run_finished') {
      refetchDiscussions();
      // Refresh batch summaries so the "↗ run #N" pastille picks up the
      // just-finalized parent workflow link for this run.
      refetchBatchSummaries();
      // Clear the per-disc "Agent en cours..." indicator for the child that
      // just finished. Batch children are fire-and-forget on the client side
      // (no SSE consumer → cleanupStream never runs), so the WS event is the
      // only signal we get that the agent actually finished.
      setSendingMap(prev => ({ ...prev, [msg.discussion_id]: false }));
      // Invariant: sending=false ⟺ no live controller. If this disc's run had
      // been started via SSE (controller set) but ends via this WS event, an
      // orphaned controller would keep the send re-entry guard armed forever —
      // every queued follow-up would re-enqueue instead of firing (stuck queue).
      delete abortControllers.current[msg.discussion_id];
      reloadDiscussion(msg.discussion_id);
      const name = msg.batch_name ?? 'Batch';
      if (msg.batch_failed === 0) {
        toast(t('qp.batch.toast.ok', name, msg.batch_completed), 'success');
      } else {
        // No 'warning' variant in useToast — use 'info' so the toast still
        // shows distinctively without crashing the type check.
        toast(t('qp.batch.toast.partial', name, msg.batch_completed, msg.batch_failed), 'warning');
      }
    }
    // Batch child STARTED — set the per-disc "Agent en cours..." indicator on.
    // Batch children run server-side with no SSE consumer on the client, so
    // this WS event is the only signal that an agent actually began. Without
    // it, sendingMap[child] stays unset and an in-flight child shows no spinner
    // (sidebar pill + open chat view). The progress/finished events below clear
    // it. refetchDiscussions() so a child created mid-batch shows up in the list.
    if (msg.type === 'batch_run_child_started') {
      setSendingMap(prev => ({ ...prev, [msg.discussion_id]: true }));
      refetchDiscussions();
    }
    // Batch progress tick — clear the spinner for the disc that just finished
    // and refresh the list so the pill ticks live.
    if (msg.type === 'batch_run_progress') {
      refetchDiscussions();
      setSendingMap(prev => ({ ...prev, [msg.discussion_id]: false }));
      delete abortControllers.current[msg.discussion_id]; // keep sending⟺controller invariant
      reloadDiscussion(msg.discussion_id);
    }
    // Backend boot recovered in-flight agent partials — refresh the affected
    // discs + tell the user so they don't resend on top of the recovered run.
    if (msg.type === 'partial_response_recovered') {
      refetchDiscussions();
      for (const id of msg.discussion_ids) {
        // Synchronous cleanup FIRST (before the async reload), so a reload
        // failure can't skip it. Drop the stale "sending" indicator left over
        // from before the restart, AND the orphaned SSE controller — otherwise
        // handleSendMessage's re-entry guard stays armed and queued follow-ups
        // re-enqueue forever instead of firing (stuck queue).
        setSendingMap(prev => ({ ...prev, [id]: false }));
        delete abortControllers.current[id];
        reloadDiscussion(id);
      }
      toast(t('disc.partialRecoveredToast', msg.discussion_ids.length), 'info');
    }
  // NOTE: reloadDiscussion is defined later in the component and referenced
  // here only inside the callback body (closure). Do NOT add it to the dep
  // array — it would be in the temporal dead zone at this point in render
  // and throw a ReferenceError.
  }, [contactsList, activeDiscussionId, refetchDiscussions, setSendingMap, toast, t]);

  // Reliable presence SNAPSHOT. The `presence` WS events above are edge-triggered
  // (fired only when a peer connects/disconnects), so a frontend that subscribed
  // AFTER a peer came online — or missed the event during a backend rebuild —
  // shows the contact OFFLINE even though it's reachable (the "Romu apparaît hors
  // ligne alors qu'il est connecté" bug). Ping each contact's /health to
  // re-derive the dots; complements the instant edge events.
  const refreshContactsPresence = useCallback(async () => {
    if (contactsList.length === 0) return;
    const entries = await Promise.all(contactsList.map(async (c) => {
      try { return [c.id, await contactsApi.ping(c.id)] as const; }
      catch { return [c.id, false] as const; }
    }));
    setContactsOnline(prev => ({ ...prev, ...Object.fromEntries(entries) }));
  }, [contactsList]);

  // On every WS (re)connect, re-sync so the UI catches up on anything missed
  // while the socket was down (a backend rebuild / dropped connection drops
  // federated chat + presence events with no listener). Inline closure (not a
  // dep) — `reloadDiscussion` is declared further down; it's only *called* after
  // mount, so the temporal-dead-zone caveat that applies to dep arrays here
  // (see handleWsMessage above) doesn't apply to a deferred call.
  const { connected: wsConnected } = useWebSocket(handleWsMessage, () => {
    refetchDiscussions();
    if (activeDiscussionId) reloadDiscussion(activeDiscussionId);
    refreshContactsPresence();
  });

  // Baseline presence poll (every 30s) — edge events handle instant transitions
  // in between; this guarantees the dots converge to the truth even if an event
  // was missed. The initial fetch comes from the WS onConnect above (fires on
  // first connect), so we don't also ping synchronously here.
  useEffect(() => {
    const id = setInterval(refreshContactsPresence, 30000);
    return () => clearInterval(id);
  }, [refreshContactsPresence]);

  // 0.8.3 (#280) — Poll the audit-status of the active discussion's
  // project so we can show a banner when an audit is running. The
  // backend exposes `null` when no audit is in progress and an
  // `AuditProgress` object otherwise. Poll every 8 s while the
  // discussion is open + on a real project; stop when the disc is
  // closed or has no project_id. Banner reads from this state — see
  // the render section below for the JSX. Cheap call (single DB
  // read for the tracker map lookup); no impact on the audit run.
  useEffect(() => {
    const activeProjectId = activeDiscussionId
      ? allDiscussions.find(d => d.id === activeDiscussionId)?.project_id
      : null;
    if (!activeProjectId) {
      setAuditRunningOnActiveProject(false);
      return;
    }
    let cancelled = false;
    const fetchOnce = async () => {
      try {
        const progress = await projectsApi.auditStatus(activeProjectId);
        if (!cancelled) setAuditRunningOnActiveProject(progress !== null);
      } catch {
        // 404 / network → assume no audit, hide the banner. The
        // poll re-tries every 8s so a transient failure self-heals.
        if (!cancelled) setAuditRunningOnActiveProject(false);
      }
    };
    fetchOnce();
    const interval = setInterval(fetchOnce, 8_000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [activeDiscussionId, allDiscussions]);


  // Mark active discussion as seen + sync activeDiscussionId to parent
  useEffect(() => {
    onActiveDiscussionChange(activeDiscussionId);
  }, [activeDiscussionId, onActiveDiscussionChange]);

  useEffect(() => {
    if (activeDiscussionId && activeDiscussion) {
      // 0.8.3 (#277) — guard against the off-by-N+ bug where the
      // list endpoint returns `messages: []` (empty by design,
      // populated only by `discussions.get`). 0.8.7 — the badge
      // counts USER+AGENT messages only (System rows = tool / summary
      // breadcrumbs would inflate it), so we seed `lastSeen` with
      // the same basis used by `unseenBasis` — and take the max with
      // a filtered messages count to defend against the empty-array
      // first-render race from #277.
      const filtered = activeDiscussion.messages.filter(m => m.role !== 'System').length;
      const total = Math.max(filtered, unseenBasis(activeDiscussion));
      markDiscussionSeen(activeDiscussionId, total);
    }
  }, [activeDiscussionId, activeDiscussion?.messages.length, activeDiscussion?.non_system_message_count, markDiscussionSeen]);

  // Timer for agent activity duration — uses lifted startMap to survive page switches
  useEffect(() => {
    if (sending && activeDiscussionId) {
      // Hoist the (truthy here) id into a const so the closures below see
      // the narrowed string instead of the original `string | null` —
      // avoids non-null assertions and keeps the React-19 lint clean.
      const did = activeDiscussionId;
      // Record start time if not already set
      if (!sendingStartMap[did]) {
        setSendingStartMap(prev => ({ ...prev, [did]: Date.now() }));
      }
      // Update elapsed every second from the persistent start time
      const tick = () => {
        const start = sendingStartMap[did] || Date.now();
        setSendingElapsed(Math.floor((Date.now() - start) / 1000));
      };
      tick();
      sendingTimerRef.current = setInterval(tick, 1000);
    } else {
      if (sendingTimerRef.current) { clearInterval(sendingTimerRef.current); sendingTimerRef.current = null; }
      setSendingElapsed(0);
    }
    return () => { if (sendingTimerRef.current) clearInterval(sendingTimerRef.current); };
  }, [sending, activeDiscussionId, sendingStartMap]);

  // Auto-scroll on new messages, sending state, and streaming. Two rules:
  // 1. We only auto-scroll if the user is "stuck to bottom" — i.e. they
  //    haven't manually scrolled up to read older content. This is the
  //    classic "stick to bottom" pattern from chat UIs (Slack, Discord, …):
  //    if you scroll up, the stream stops yanking you back; if you scroll
  //    back near the bottom, auto-scroll re-engages.
  // 2. The streaming branch is throttled to ~250ms so we don't thrash
  //    layout at every chunk.
  //
  // CRITICAL: we read `stickToBottom` through a ref, NOT as a useEffect
  // dependency. Otherwise scrolling up flips stickToBottom → the effect
  // re-runs → it sees `!stickToBottom` → it incorrectly flags
  // hasNewWhileScrolledUp=true even though no new content has arrived.
  // The pill must only appear when fresh content shows up while the user
  // is already scrolled up — not just because they scrolled up.
  const lastScrollRef = useRef(0);
  const stickToBottomRef = useRef(stickToBottom);
  useEffect(() => { stickToBottomRef.current = stickToBottom; }, [stickToBottom]);
  // Update stickToBottom whenever the user scrolls inside the messages
  // container. Threshold = 80px from bottom counts as "still at bottom".
  const handleMessagesScroll = useCallback(() => {
    const el = messagesContainerRef.current;
    if (!el) return;
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    const atBottom = distanceFromBottom < 80;
    setStickToBottom(atBottom);
    if (atBottom) setHasNewWhileScrolledUp(false);
    // Show the "jump to top" arrow once the user has scrolled past the
    // first viewport. 80 px hysteresis at the top to avoid flicker.
    setScrolledFromTop(el.scrollTop > 80);
  }, []);
  useEffect(() => {
    if (!stickToBottomRef.current) {
      setHasNewWhileScrolledUp(true);
      return;
    }
    chatEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [activeDiscussion?.messages.length, sending]);
  useEffect(() => {
    if (!streamingText) return;
    if (!stickToBottomRef.current) {
      setHasNewWhileScrolledUp(true);
      return;
    }
    const now = Date.now();
    if (now - lastScrollRef.current < 250) return;
    lastScrollRef.current = now;
    chatEndRef.current?.scrollIntoView({ behavior: 'instant' as ScrollBehavior });
  }, [streamingText]);
  // Re-engage auto-scroll when the user switches discussions: jump to the
  // bottom of the new conversation and reset the stick flag.
  //
  // The two effects below split the work in two steps so the scroll
  // actually lands on the *latest* message instead of the first:
  //   - reset state immediately on disc switch,
  //   - defer the scroll until messages are rendered (driven by the
  //     `activeDiscussion?.messages.length` effect).
  // Pre-fix the scroll fired in the same frame as the switch — the disc
  // detail fetch (`discussionsApi.get`) was still in flight, so
  // `chatEndRef` was beneath an empty container and the user landed on
  // the first message of the previous render. User feedback on
  // 2026-05-09: "quand on clique sur un message d'une discussion, on
  // arrive toujours sur le premier message".
  useEffect(() => {
    setStickToBottom(true);
    setHasNewWhileScrolledUp(false);
  }, [activeDiscussionId]);

  // Scroll to bottom once messages of the active disc are actually
  // present. Triggers on: initial mount, disc switch (after the fetch
  // settles), and every new message length change while sticky.
  // We only auto-scroll on the cold load — once the user has scrolled
  // away the regular `stickToBottom` logic above takes over.
  const lastSettledDiscIdRef = useRef<string | null>(null);
  useEffect(() => {
    if (!activeDiscussion?.id) return;
    const isFirstSettle = lastSettledDiscIdRef.current !== activeDiscussion.id;
    if (!isFirstSettle) return;
    if ((activeDiscussion.messages?.length ?? 0) === 0) return;
    lastSettledDiscIdRef.current = activeDiscussion.id;
    requestAnimationFrame(() => {
      chatEndRef.current?.scrollIntoView({ behavior: 'instant' as ScrollBehavior });
    });
  }, [activeDiscussion?.id, activeDiscussion?.messages.length]);

  // Refresh the pending-files count on the git-panel icon for Isolated
  // discussions. Fires on: discussion switch, every new message (typically
  // after an agent reply lands), and when the agent run ends (sending: true
  // → false). Silent on failure — the badge just stays at its last value.
  useEffect(() => {
    if (!activeDiscussion || activeDiscussion.workspace_mode !== 'Isolated') {
      setPendingFilesCount(0);
      return;
    }
    if (sending) return; // let the stream finish before polling
    let cancelled = false;
    discussionsApi.gitStatus(activeDiscussion.id)
      .then((res: { files?: unknown }) => {
        if (cancelled) return;
        const files = res?.files ?? [];
        setPendingFilesCount(Array.isArray(files) ? files.length : 0);
      })
      .catch(() => { /* keep last count on transient errors */ });
    return () => { cancelled = true; };
  }, [activeDiscussion?.id, activeDiscussion?.messages.length, activeDiscussion?.workspace_mode, sending]);

  // Handle prefill from parent (e.g. "validate audit" button on Projects page)
  useEffect(() => {
    if (prefill) {
      setShowNewDiscussion(true);
    }
  }, [prefill]);

  // ─── Callbacks ───────────────────────────────────────────────────────────

  const reloadDiscussion = useCallback((discId: string) => {
    discussionsApi.get(discId).then(disc => {
      if (disc) setLoadedDiscussions(prev => ({ ...prev, [disc.id]: disc }));
    }).catch(() => {});
  }, []);

  const cleanupStream = useCallback((discId: string) => {
    // Reported scroll-jump bug: when the SSE stream finishes, the
    // streaming bubble unmounts (driven by `sending=false`) BEFORE the
    // refetch lands the persisted agent message. The DOM briefly loses
    // the bubble's height — the scroll position clamps up to the
    // previous user message — and then the refetch lands and a smooth
    // scroll animates down to the new bottom. The user perceives:
    // "ça remonte au début du message, puis ça redescend".
    //
    // Fix: before flipping `sending=false`, convert the in-memory
    // `streamingMap[discId]` buffer into an OPTIMISTIC agent message on
    // `loadedDiscussions[discId].messages`. The streaming bubble
    // unmounts at the same render where the optimistic bubble mounts
    // — same content, same position, no height delta, no scroll jump.
    // The async refetch arrives shortly after and REPLACES the
    // optimistic with the persisted message (matching content), so the
    // final state converges and no duplicate is left behind.
    const streamedText = streamingMap[discId];
    const targetAgent = streamingTargetMap[discId];
    if (streamedText && streamedText.length > 0) {
      setLoadedDiscussions(prev => {
        const disc = prev[discId];
        if (!disc) return prev;
        // Bail out if the last message is already an Agent — guards
        // against double-insert if cleanupStream is called twice
        // (orchestrate cancellation, fast retry, etc.).
        const last = disc.messages[disc.messages.length - 1];
        if (last?.role === 'Agent') return prev;
        return {
          ...prev,
          [discId]: {
            ...disc,
            messages: [...disc.messages, {
              id: `optimistic-agent-${Date.now()}`,
              role: 'Agent' as const,
              content: streamedText,
              agent_type: targetAgent ?? disc.agent,
              timestamp: new Date().toISOString(),
              tokens_used: 0,
              auth_mode: null,
            }],
            message_count: disc.message_count + 1,
            non_system_message_count: disc.non_system_message_count + 1,
          },
        };
      });
    }
    cleanupStreamBase(discId);
    refetchDiscussions();
    refetchProjects(); // Refresh project audit_status for CTA updates
    reloadDiscussion(discId);
    // Clear the @-mention target — next message goes back to the
    // discussion's default agent unless re-pinged.
    setStreamingTargetMap(prev => {
      const { [discId]: _drop, ...rest } = prev;
      return rest;
    });
  }, [cleanupStreamBase, refetchDiscussions, refetchProjects, reloadDiscussion, streamingMap, streamingTargetMap]);

  // Called by ChatHeader after any inline API update (title, skills, profiles, etc.)
  const handleDiscussionUpdated = useCallback(() => {
    refetchDiscussions();
    if (activeDiscussionId) reloadDiscussion(activeDiscussionId);
  }, [refetchDiscussions, activeDiscussionId, reloadDiscussion]);

  // ChatInput dispatches `kronn:discussion-updated` after auto-activating
  // skills (or any future inline mutation that doesn't go through
  // ChatHeader). Pre-fix this event was emitted but had no listener,
  // so the sidebar + chips kept the old skill_ids until a manual
  // refresh — fixed here by mirroring the WsMessage `chat_message`
  // path: refetch the list + reload the active disc.
  useEffect(() => {
    window.addEventListener('kronn:discussion-updated', handleDiscussionUpdated);
    return () => window.removeEventListener('kronn:discussion-updated', handleDiscussionUpdated);
  }, [handleDiscussionUpdated]);

  // Called by ChatHeader after agent switch — triggers agent run on new agent
  const handleAgentSwitch = useCallback(async (_newAgent: AgentType) => {
    if (!activeDiscussionId) return;
    const discId = activeDiscussionId;
    reloadDiscussion(discId);
    refetchDiscussions();
    // Auto-trigger the new agent to introduce itself with a summary
    const controller = new AbortController();
    abortControllers.current[discId] = controller;
    setSendingMap(prev => ({ ...prev, [discId]: true }));
    setSendingStartMap(prev => ({ ...prev, [discId]: Date.now() }));
    setStreamingMap(prev => ({ ...prev, [discId]: '' }));
    resetAgentLogs();
    await discussionsApi.runAgent(
      discId,
      (text) => appendStreamChunk(discId, text),
      () => cleanupStream(discId),
      (error) => { console.error('Agent error:', error); const e = userError(error); if (e.includes('checked out') || e.includes('worktree')) { setWorktreeError(e); } else { toast(e, 'error'); } cleanupStream(discId); },
      controller.signal,
      onAgentLog,
    );
  }, [activeDiscussionId, reloadDiscussion, refetchDiscussions, abortControllers, setSendingMap, setSendingStartMap, setStreamingMap, resetAgentLogs, appendStreamChunk, cleanupStream, toast, onAgentLog]);

  // Refetch projects when viewing a briefing/validation discussion to get fresh audit_status
  useEffect(() => {
    if (!activeDiscussionId) return;
    const disc = allDiscussions.find(d => d.id === activeDiscussionId);
    if (disc && (isBriefingDisc(disc.title) || isValidationDisc(disc.title))) {
      refetchProjects();
    }
  }, [activeDiscussionId, allDiscussions, refetchProjects]);

  // Handle auto-run: open existing discussion and trigger agent (e.g. after full audit)
  // Uses a ref to track the pending run so that re-renders (from onAutoRunConsumed/refetch)
  // don't cancel the timeout via effect cleanup.
  // Ensure the sidebar groups containing a discussion are expanded when navigating to it
  const ensureDiscussionVisible = useCallback((discId: string) => {
    const disc = allDiscussions.find(d => d.id === discId);
    if (!disc) return;
    setCollapsedDiscGroups(prev => {
      const next = new Set(prev);
      let changed = false;
      // Uncollapse the project group
      if (disc.project_id) {
        if (next.has(disc.project_id)) { next.delete(disc.project_id); changed = true; }
        // Uncollapse the org group
        const proj = projects.find(p => p.id === disc.project_id);
        if (proj) {
          const org = getProjectGroup(proj, t('disc.local'), t('disc.local'));
          const orgKey = `org::${org}`;
          if (next.has(orgKey)) { next.delete(orgKey); changed = true; }
        }
      } else {
        // Global discussion
        if (next.has('__global__')) { next.delete('__global__'); changed = true; }
      }
      return changed ? next : prev;
    });
  }, [allDiscussions, projects, t]);

  // Ensure the active discussion is visible in the sidebar once data is loaded
  const initialVisibilityDone = useRef(false);
  useEffect(() => {
    if (initialVisibilityDone.current) return;
    const targetId = activeDiscussionId;
    if (targetId && allDiscussions.length > 0) {
      ensureDiscussionVisible(targetId);
      initialVisibilityDone.current = true;
    }
  }, [activeDiscussionId, allDiscussions.length, ensureDiscussionVisible]);

  const pendingAutoRun = useRef<string | null>(null);
  useEffect(() => {
    if (!autoRunDiscussionId || pendingAutoRun.current === autoRunDiscussionId) return;
    const discId = autoRunDiscussionId;
    pendingAutoRun.current = discId;
    onAutoRunConsumed?.();

    // Select the discussion, uncollapse its group, and show loader immediately
    setActiveDiscussionId(discId);
    ensureDiscussionVisible(discId);
    setSendingMap(prev => ({ ...prev, [discId]: true }));
    setStreamingMap(prev => ({ ...prev, [discId]: '' }));
    refetchDiscussions();

    // Trigger agent run after a short delay to let discussion load in sidebar
    const controller = new AbortController();
    abortControllers.current[discId] = controller;
    setTimeout(async () => {
      pendingAutoRun.current = null;
      if (controller.signal.aborted) return;
      await discussionsApi.runAgent(
        discId,
        (text) => appendStreamChunk(discId, text),
        () => cleanupStream(discId),
        (error) => { console.error('Agent error:', error); const e = userError(error); if (e.includes('checked out') || e.includes('worktree')) { setWorktreeError(e); } else { toast(e, 'error'); } cleanupStream(discId); },
        controller.signal,
        onAgentLog,
      );
    }, 500);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoRunDiscussionId]);

  // Handle open-discussion: just select it without triggering agent (e.g. Resume Validation)
  useEffect(() => {
    if (!openDiscussionId) return;
    // Wait until allDiscussions is loaded before trying to ensure visibility
    if (allDiscussions.length === 0) return;
    setActiveDiscussionId(openDiscussionId);
    ensureDiscussionVisible(openDiscussionId);
    onOpenDiscConsumed?.();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openDiscussionId, allDiscussions.length]);

  // ── Cross-page navigation: WorkflowDetail "📋 N conversations" → here ──
  // The chip click sets `focusBatchId` on Dashboard, which lands as a prop here.
  // We expand the matching project + batch group, then scroll to it. The
  // sidebar render uses `data-batch-key` on the wrapper so we can target it.
  useEffect(() => {
    if (!focusBatchId || allDiscussions.length === 0) return;
    const childDisc = allDiscussions.find(d => d.workflow_run_id === focusBatchId);
    if (!childDisc) {
      // Batch not in the current discs list (deleted? still loading?). Ack
      // anyway so we don't loop on the same id forever.
      onFocusBatchConsumed?.();
      return;
    }
    const projectKey = childDisc.project_id ?? null;
    const batchKey = `batch::${focusBatchId}`;
    setCollapsedDiscGroups(prev => {
      const next = new Set(prev);
      if (projectKey != null) next.delete(projectKey);
      next.delete(batchKey);
      return next;
    });
    // Defer the scroll one tick so the just-uncollapsed nodes have time to render.
    requestAnimationFrame(() => {
      const el = document.querySelector(`[data-batch-key="${batchKey}"]`);
      if (el && 'scrollIntoView' in el) {
        (el as HTMLElement).scrollIntoView({ behavior: 'smooth', block: 'center' });
      }
    });
    onFocusBatchConsumed?.();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusBatchId, allDiscussions.length]);

  const handleCreateDiscussion = async (config: NewDiscConfig) => {
    let disc;
    try {
      disc = await discussionsApi.create({
        project_id: config.projectId,
        title: config.title,
        agent: config.agent,
        language: configLanguage ?? 'fr',
        initial_prompt: config.prompt,
        skill_ids: config.skillIds.length > 0 ? config.skillIds : undefined,
        profile_ids: config.profileIds.length > 0 ? config.profileIds : undefined,
        ...(config.directiveIds.length > 0 ? { directive_ids: config.directiveIds } : {}),
        workspace_mode: config.workspaceMode === 'Isolated' ? 'Isolated' : undefined,
        base_branch: config.workspaceMode === 'Isolated' ? config.baseBranch : undefined,
        tier: config.tier !== 'default' ? config.tier : undefined,
      });
    } catch (e) {
      toast(userError(e), 'error');
      return;
    }
    setShowNewDiscussion(false);
    setActiveDiscussionId(disc.id);
    refetchDiscussions();

    // Upload pending context files (from NewDiscussionForm) before running agent
    if (config.pendingFiles?.length) {
      for (const file of config.pendingFiles) {
        try {
          const resp = await discussionsApi.uploadContextFile(disc.id, file);
          setContextFilesMap(prev => ({
            ...prev,
            [disc.id]: [...(prev[disc.id] ?? []), resp.file],
          }));
        } catch (e) {
          toast(`${file.name}: ${userError(e)}`, 'error');
        }
      }
      // Pin them to the FIRST message (the initial_prompt). Unlike the in-disc
      // composer, the popup uploads after the first message exists and runs via
      // runAgent (which doesn't link), so without this they'd stay pending and
      // get vacuumed into message #2 on the next send. create() returns the
      // initial User message, so its id is right here.
      const firstMsgId = disc.messages?.find(m => m.role === 'User')?.id ?? disc.messages?.[0]?.id;
      if (firstMsgId) {
        try {
          await discussionsApi.linkPendingContextFiles(disc.id, firstMsgId);
          await loadContextFiles(disc.id);
        } catch (e) {
          console.warn('[create] link pending files failed:', e);
        }
      }
    }

    const discId = disc.id;

    // 0.8.6 phase 2 — disc-first refactor. When the user explicitly
    // chose NOT to launch an agent, the disc is created empty and
    // becomes a waiting room for invited peers ([+ Inviter] in the
    // header). Skip the CLI kick-off entirely — no streaming, no
    // tokens, no worktree setup. The first message (`initial_prompt`)
    // is already stored as part of `discussions.create`.
    if (!config.launchAgentNow) {
      toast(t('disc.discFirstCreatedToast'), 'success');
      return;
    }

    const controller = new AbortController();
    abortControllers.current[discId] = controller;
    setSendingMap(prev => ({ ...prev, [discId]: true }));
    setStreamingMap(prev => ({ ...prev, [discId]: '' }));
    resetAgentLogs();
    await discussionsApi.runAgent(
      discId,
      (text) => appendStreamChunk(discId, text),
      () => cleanupStream(discId),
      (error) => { console.error('Agent error:', error); const e = userError(error); if (e.includes('checked out') || e.includes('worktree')) { setWorktreeError(e); } else { toast(e, 'error'); } cleanupStream(discId); },
      controller.signal,
        onAgentLog,
    );
  };

  const handleSendMessage = async (msg: string, targetAgent?: AgentType) => {
    if (!activeDiscussionId || !msg.trim()) return;
    const discId = activeDiscussionId;
    // Synchronous re-entry guard: `sending` is derived from `sendingMap`
    // which only flips true inside the SSE `onStart` callback (backend
    // round-trip ~100 ms-2 s). A fast double / triple click on Send fires
    // before that flip and runs handleSendMessage in parallel — both
    // calls see `sending=false` from the same render closure, both
    // optimistically insert a user message, and both call
    // `sendMessageStream`, producing 2-3 parallel agent runs on the same
    // disc with duplicate user messages in the transcript.
    //
    // `abortControllers.current[discId]` is set synchronously a few
    // lines below; presence here means a previous call is in-flight.
    //
    // 0.8.8 — instead of dropping a message typed mid-stream, QUEUE it
    // (CLI-style). `useMessageQueue` drains one per completion edge, so the
    // queued follow-up auto-fires when the current run finishes. The double-
    // click guard is preserved: `abortControllers.current[discId]` is set
    // synchronously, so a fast second click within the same tick enqueues
    // rather than launching a parallel run.
    if (sendingMap[discId] || abortControllers.current[discId]) {
      enqueueMessage(msg, targetAgent);
      return;
    }
    stopTts();

    // Optimistically add user message to loadedDiscussions so it appears immediately
    setLoadedDiscussions(prev => {
      const disc = prev[discId];
      if (!disc) return prev;
      return {
        ...prev,
        [discId]: {
          ...disc,
          messages: [...disc.messages, {
            id: `optimistic-${Date.now()}`,
            role: 'User' as const,
            content: msg,
            agent_type: null,
            timestamp: new Date().toISOString(),
            tokens_used: 0,
            auth_mode: null,
          }],
          message_count: disc.message_count + 1,
          non_system_message_count: disc.non_system_message_count + 1,
        },
      };
    });

    const controller = new AbortController();
    abortControllers.current[discId] = controller;
    setStreamingMap(prev => ({ ...prev, [discId]: '' }));
    // Track the pinged agent so the streaming placeholder ("Codex · Agent
    // running…") shows the right name and color while we wait for the first
    // chunk. Without this the placeholder always shows the discussion's
    // default agent — the user pings @codex but sees Claude Code spinning,
    // which makes the @-mention feel broken.
    if (targetAgent) {
      setStreamingTargetMap(prev => ({ ...prev, [discId]: targetAgent }));
    } else {
      setStreamingTargetMap(prev => {
        const { [discId]: _drop, ...rest } = prev;
        return rest;
      });
    }

    resetAgentLogs();
    await discussionsApi.sendMessageStream(
      discId,
      { content: msg, target_agent: targetAgent },
      (text) => appendStreamChunk(discId, text),
      () => cleanupStream(discId),
      (error) => {
        console.error('Agent error:', error);
        const errStr = userError(error);
        if (errStr.includes('checked out') || errStr.includes('worktree')) {
          setWorktreeError(errStr);
        } else if (errStr.includes('partial_pending')) {
          // Backend refused: previous run still in recovery. Offer one-click
          // dismiss so the user can retype without waiting for the WS event.
          if (confirm(t('disc.partialPendingPrompt'))) {
            discussionsApi.dismissPartial(discId)
              .then(() => {
                refetchDiscussions();
                reloadDiscussion(discId);
                toast(t('disc.partialDismissed'), 'success');
              })
              .catch(e => toast(userError(e), 'error'));
          }
        } else {
          toast(errStr, 'error');
        }
        cleanupStream(discId);
      },
      controller.signal,
      () => {
        refetchDiscussions();
        setSendingMap(prev => ({ ...prev, [discId]: true }));
        // The backend just persisted the user message (real id) and pinned any
        // composer-staged files to it (0.8.8). Reload BOTH so the attachment
        // binds immediately: reloadDiscussion swaps the optimistic message
        // (id `optimistic-…`) for the real one, and loadContextFiles refreshes
        // each file's message_id. Without the disc reload the files stay keyed
        // to a message id the UI doesn't have yet, so the thumbnail wouldn't
        // appear until the agent finished (cleanupStream's reload) — defeating
        // the whole point of showing your attachment the instant you send.
        reloadDiscussion(discId);
        loadContextFiles(discId);
        // The optimistic update above bumped both counts by 1 (the freshly
        // queued User message); seed lastSeen with the matching non-System
        // basis so the badge resolves to 0 without waiting on the next tick.
        markDiscussionSeen(discId, activeDiscussion ? unseenBasis(activeDiscussion) + 1 : 1);
      },
      onAgentLog,
    );
  };

  // QP chain — load QPs, queue one mid-stream, auto-fire on the sending
  // true→false edge. Extracted to hooks/useQpChain.ts.
  const { chainableQPs, queuedQP, queueQP, cancelQueuedQP } = useQpChain({
    sending,
    onFire: handleSendMessage,
  });

  // CLI-style message queue — type follow-up messages while the agent is still
  // streaming; they auto-fire one-by-one as each response completes. Handled at
  // Kronn's orchestration layer (never concurrent sends), so it works for EVERY
  // agent type regardless of whether the underlying CLI supports queueing.
  const {
    queue: queuedMessages,
    enqueue: enqueueMessage,
    removeQueued: removeQueuedMessage,
    clearQueue: clearMessageQueue,
  } = useMessageQueue({
    discId: activeDiscussionId,
    sending,
    onFire: handleSendMessage,
  });

  const handleStop = () => {
    if (!activeDiscussionId) return;
    const discId = activeDiscussionId;
    // Backend cancellation FIRST — without this the agent keeps
    // running and burning tokens after the user clicked Stop.
    // Pre-fix the local abort just disconnected the SSE channel; the
    // backend's `tx.is_closed()` branch is "keep running to save
    // result in DB", so the agent finished its long thought + saved
    // a full response *despite* the user explicitly stopping.
    discussionsApi.stop(discId).catch((e) => {
      console.warn('Backend stop failed (continuing local abort):', e);
    });
    const controller = abortControllers.current[discId];
    if (controller) controller.abort();
    cleanupStream(discId);
    // Stop means stop: drop any messages the user queued during this run
    // rather than auto-firing them on the (now cancelled) completion edge.
    clearMessageQueue();
  };

  const handleTtsToggle = useCallback(() => {
    setTtsEnabled(prev => {
      if (prev) { stopTts(); setTtsState('idle'); setTtsPlayingMsgId(null); }
      return !prev;
    });
  }, []);

  const handleWorktreeRetry = useCallback(async () => {
    if (!activeDiscussionId) return;
    try {
      await discussionsApi.worktreeLock(activeDiscussionId);
      setWorktreeError(null);
      reloadDiscussion(activeDiscussionId);
      toast(t('disc.worktreeLock') + ' ✓', 'success');
    } catch (err) {
      setWorktreeError(String(err));
    }
  }, [activeDiscussionId, reloadDiscussion, toast, t]);

  // ── Context files ──────────────────────────────────────────────────────────
  const loadContextFiles = useCallback(async (discId: string) => {
    try {
      const files = await discussionsApi.listContextFiles(discId);
      setContextFilesMap(prev => ({ ...prev, [discId]: files }));
    } catch { /* ignore */ }
  }, []);

  // Load context files when a discussion becomes active
  useEffect(() => {
    if (activeDiscussionId && !contextFilesMap[activeDiscussionId]) {
      loadContextFiles(activeDiscussionId);
    }
  }, [activeDiscussionId, contextFilesMap, loadContextFiles]);

  const handleUploadFiles = useCallback(async (files: File[]) => {
    if (!activeDiscussionId) return;
    setUploadingFiles(true);
    for (const file of files) {
      try {
        const resp = await discussionsApi.uploadContextFile(activeDiscussionId, file);
        setContextFilesMap(prev => ({
          ...prev,
          [activeDiscussionId]: [...(prev[activeDiscussionId] ?? []), resp.file],
        }));
      } catch (e) {
        toast(userError(e), 'error');
      }
    }
    setUploadingFiles(false);
  }, [activeDiscussionId, toast]);

  const handleDeleteContextFile = useCallback(async (fileId: string) => {
    if (!activeDiscussionId) return;
    try {
      await discussionsApi.deleteContextFile(activeDiscussionId, fileId);
      setContextFilesMap(prev => ({
        ...prev,
        [activeDiscussionId]: (prev[activeDiscussionId] ?? []).filter(f => f.id !== fileId),
      }));
    } catch (e) {
      toast(userError(e), 'error');
    }
  }, [activeDiscussionId, toast]);

  // Split the discussion's files into "pending" (still in the composer, no
  // message_id) and "attached" (pinned to a message, grouped by message id).
  // The composer shows only pending; each message bubble shows its own (0.8.8).
  const activeDiscFiles = contextFilesMap[activeDiscussionId ?? ''] ?? [];
  const pendingContextFiles = useMemo(
    () => activeDiscFiles.filter(f => !f.message_id),
    [activeDiscFiles],
  );
  const attachmentsByMessageId = useMemo(() => {
    const map: Record<string, ContextFile[]> = {};
    for (const f of activeDiscFiles) {
      if (f.message_id) (map[f.message_id] ??= []).push(f);
    }
    return map;
  }, [activeDiscFiles]);

  const handleRetry = async () => {
    if (!activeDiscussionId || sending) return;
    const discId = activeDiscussionId;
    await discussionsApi.deleteLastAgentMessages(discId);
    await refetchDiscussions();
    reloadDiscussion(discId);
    const controller = new AbortController();
    abortControllers.current[discId] = controller;
    setSendingMap(prev => ({ ...prev, [discId]: true }));
    setStreamingMap(prev => ({ ...prev, [discId]: '' }));
    resetAgentLogs();
    await discussionsApi.runAgent(
      discId,
      (text) => appendStreamChunk(discId, text),
      () => cleanupStream(discId),
      (error) => {
        console.error('Agent error:', error);
        const errStr = userError(error);
        if (errStr.includes('checked out') || errStr.includes('worktree')) {
          setWorktreeError(errStr);
        } else {
          toast(errStr, 'error');
        }
        cleanupStream(discId);
      },
      controller.signal,
        onAgentLog,
    );
  };

  // Stable MessageBubble callbacks (avoid breaking memo)
  const handleMsgCopy = useCallback((msgId: string, content: string) => {
    navigator.clipboard.writeText(content);
    setCopiedMsgId(msgId);
    setTimeout(() => setCopiedMsgId(prev => prev === msgId ? null : prev), 1500);
  }, []);
  const handleMsgTts = useCallback(async (msgId: string, content: string, lang: string) => {
    const isThisMsg = ttsPlayingMsgId === msgId;
    if (isThisMsg && ttsState === 'paused') { resumeTts(); setTtsState('playing'); }
    else if (isThisMsg && (ttsState === 'playing' || ttsState === 'loading')) { pauseTts(); setTtsState('paused'); }
    else {
      setTtsPlayingMsgId(msgId); setTtsState('loading'); setTtsEnabled(true);
      await speakText(getTtsWorker, content, lang, () => setTtsState('playing'));
      setTtsPlayingMsgId(cur => { if (cur === msgId && !isTtsPaused()) { setTtsState('idle'); return null; } return cur; });
    }
  }, [ttsPlayingMsgId, ttsState]);
  const handleMsgEditStart = useCallback((msgId: string, content: string) => {
    setEditingMsgId(msgId); setEditingText(content);
  }, []);
  const handleMsgEditCancel = useCallback(() => { setEditingMsgId(null); setEditingText(''); }, []);
  const handleMsgExpandSummary = useCallback((msgId: string) => {
    setExpandedSummaryMsgId(prev => prev === msgId ? null : msgId);
  }, []);

  // Stable sidebar callbacks (avoid breaking SwipeableDiscItem memo)
  const handleDiscSelect = useCallback((discId: string, msgCount: number) => {
    setActiveDiscussionId(discId);
    markDiscussionSeen(discId, msgCount);
    if (isMobile) setSidebarOpen(false);
  }, [isMobile, markDiscussionSeen]);
  const handleDiscArchive = useCallback(async (discId: string) => {
    await discussionsApi.update(discId, { archived: true });
    setActiveDiscussionId(prev => prev === discId ? null : prev);
    refetchDiscussions();
  }, [refetchDiscussions]);
  const handleDiscDelete = useCallback(async (discId: string) => {
    if (!confirm(t('disc.confirmDelete'))) return;
    // Abort any in-flight stream + clear lifted streaming state BEFORE the
    // delete request so a slow agent reply doesn't try to write back to a
    // disc that's about to vanish from the DB.
    try { abortControllers.current[discId]?.abort(); } catch { /* noop */ }
    cleanupStreamBase(discId);
    await discussionsApi.delete(discId);
    setActiveDiscussionId(prev => prev === discId ? null : prev);
    refetchDiscussions();
  }, [refetchDiscussions, abortControllers, cleanupStreamBase, t]);
  const handleDiscUnarchive = useCallback(async (discId: string) => {
    await discussionsApi.update(discId, { archived: false });
    refetchDiscussions();
  }, [refetchDiscussions]);

  const handleToggleGroup = useCallback((key: string) => {
    setCollapsedDiscGroups(prev => {
      const n = new Set(prev);
      if (prev.has(key)) n.delete(key); else n.add(key);
      return n;
    });
  }, []);

  // ─── Test mode (worktree swap) ───────────────────────────────────────────
  // Enter retries with options set by the preflight modal (phase 5).
  // Initial call has no opts; a blocker response opens the modal, which
  // invokes this again with stash_dirty / force once the user acknowledges.
  const handleRequestTestMode = useCallback(async (
    discId: string,
    opts: { stash_dirty?: boolean; force?: boolean } = {},
  ) => {
    if (testModeBusy) return;
    setTestModeBusy(true);
    try {
      const res = await discussionsApi.testModeEnter(discId, opts);
      if (res.status === 'blocked') {
        // Open the preflight modal — it reads `res.kind` to pick the
        // right set of buttons (stash, commit first, proceed anyway).
        setTestModeBlocker(res);
        setTestModePendingDiscId(discId);
        return;
      }
      setTestModeBlocker(null);
      setTestModePendingDiscId(null);
      refetchDiscussions();
      toast(t('testMode.enterSuccess', res.tested_branch), 'success');
    } catch (e) {
      toast(t('testMode.enterError', userError(e)), 'error');
    } finally {
      setTestModeBusy(false);
    }
  }, [testModeBusy, refetchDiscussions, toast, t]);

  const handleExitTestMode = useCallback(async (discId: string) => {
    if (testModeBusy) return;
    setTestModeBusy(true);
    try {
      const res = await discussionsApi.testModeExit(discId);
      refetchDiscussions();
      // Success path: branch restored. If the backend surfaced a
      // post-checkout `warning` (typically a stash-pop conflict — the
      // exit itself succeeded but the user has work left in the
      // stash), show it as a warning toast so the operator can act on
      // it. Pre-fix this came back as `ApiResponse::err`, which the
      // catch arm below painted as a hard failure.
      if (res.warning) {
        toast(`${t('testMode.exitSuccess', res.restored_branch)} — ${res.warning}`, 'warning');
      } else {
        toast(t('testMode.exitSuccess', res.restored_branch), 'success');
      }
    } catch (e) {
      // Hard failure (checkout itself failed, DB error, …). The server
      // never cleared `test_mode_*` fields in that case, so the disc
      // is still considered "in test mode" — surface as error.
      toast(t('testMode.exitError', userError(e)), 'error');
    } finally {
      setTestModeBusy(false);
    }
  }, [testModeBusy, refetchDiscussions, toast, t]);

  // Find the (single) discussion currently in test mode. Only one can be
  // active at a time — the main repo can only be on one branch. If the DB
  // ever contains more than one (shouldn't happen), we show the first;
  // the `exit` call will clear its fields and `refetchDiscussions` will
  // surface the next one if any.
  const discussionInTestMode = allDiscussions.find(d => !!d.test_mode_restore_branch);

  const handleContactAdd = useCallback(async (code: string) => {
    const result = await contactsApi.add(code);
    setContactsList(prev => [...prev, result.contact]);
    if (result.warning) {
      const warningKey = `contacts.warn.${result.warning}`;
      const msg = t(warningKey);
      toast(msg !== warningKey ? msg : t('contacts.warnGeneric'), 'warning');
    } else {
      toast(t('contacts.added'), 'success');
    }
  }, [toast, t]);

  // Unified "join by code": paste a kr-join token → the backend resolves it
  // local OR cross-instance (asks our contacts, mirrors the disc back over WS),
  // then we refresh the list and open the (possibly just-mirrored) disc. The
  // single await covers the ~0.5–8 s remote resolution; the sidebar shows a
  // "resolving…" state meanwhile. Throws on failure so the sidebar surfaces the
  // backend message (expired / not found).
  // Click a contact → open (or create) a 1:1 human↔human shared discussion with
  // them. Reuses an existing shared disc with that contact if one exists, else
  // creates one + shares it (mirrors on the peer). (The "no agent reply"
  // guarantee for instances that DO have an agent installed is F9 backend —
  // here, posting only triggers a local agent where one is installed.)
  const handleStartChatWithContact = useCallback(async (contact: Contact) => {
    const existing = allDiscussions.find(d => !d.archived && d.shared_with?.includes(contact.id));
    if (existing) {
      setActiveDiscussionId(existing.id);
      if (isMobile) setSidebarOpen(false);
      return;
    }
    try {
      const disc = await discussionsApi.create({
        project_id: null,
        title: `Chat — ${contact.pseudo}`,
        agent: 'ClaudeCode',
        language: configLanguage ?? 'fr',
        initial_prompt: '',
        // F9 (WSL backend 6ddf25b): mark the disc human-only so send_message
        // never spawns the local runner — a true human↔human chat even on an
        // instance that has an agent installed.
        no_agent: true,
      });
      await discussionsApi.share(disc.id, [contact.id]);
      await refetchDiscussions();
      setActiveDiscussionId(disc.id);
      if (isMobile) setSidebarOpen(false);
      toast(t('contacts.chatStarted', contact.pseudo), 'success');
    } catch {
      toast(t('contacts.chatStartError'), 'error');
    }
  }, [allDiscussions, refetchDiscussions, isMobile, configLanguage, toast, t]);

  const handleJoinByCode = useCallback(async (code: string) => {
    const res = await discussionsApi.peerJoin(code);
    await refetchDiscussions();
    setActiveDiscussionId(res.disc_id);
    // Mobile: close the sidebar so the freshly-joined disc is actually shown
    // (mirrors handleDiscSelect — without this the join "succeeds" but the user
    // stays on the contact list and sees nothing happen).
    if (isMobile) setSidebarOpen(false);
    toast(t('contacts.joinSuccess', res.disc_title), 'success');
  }, [refetchDiscussions, toast, t, isMobile]);

  const handleContactDelete = useCallback(async (id: string) => {
    // Pre-fix the X button on a contact pill in the sidebar fired
    // delete with no confirmation. Removing a contact tears down the
    // shared-discussion bridge and revokes every shared message —
    // costly to recreate. Now a confirm is required.
    if (!confirm(t('contacts.deleteConfirm'))) return;
    await contactsApi.delete(id);
    setContactsList(prev => prev.filter(x => x.id !== id));
    toast(t('contacts.deleted'), 'success');
  }, [toast, t]);

  const handleEditMessage = async () => {
    if (!activeDiscussionId || !editingMsgId || !editingText.trim()) return;
    if (editingMsgInFlightRef.current) return;
    if (sending || abortControllers.current[activeDiscussionId]) return;
    editingMsgInFlightRef.current = true;
    const discId = activeDiscussionId;
    try {
      await discussionsApi.deleteLastAgentMessages(discId);
      await discussionsApi.editLastUserMessage(discId, editingText.trim());
      setEditingMsgId(null);
      setEditingText('');
      await refetchDiscussions();
      reloadDiscussion(discId);
      const controller = new AbortController();
      abortControllers.current[discId] = controller;
      setSendingMap(prev => ({ ...prev, [discId]: true }));
      setStreamingMap(prev => ({ ...prev, [discId]: '' }));
      resetAgentLogs();
      await discussionsApi.runAgent(
        discId,
        (text) => appendStreamChunk(discId, text),
        () => cleanupStream(discId),
        (error) => { console.error('Agent error:', error); const e = userError(error); if (e.includes('checked out') || e.includes('worktree')) { setWorktreeError(e); } else { toast(e, 'error'); } cleanupStream(discId); },
        controller.signal,
          onAgentLog,
      );
    } finally {
      editingMsgInFlightRef.current = false;
    }
  };

  const handleOrchestrate = async (orchAgents: AgentType[], orchRounds: number, orchSkillIds: string[], orchDirectiveIds: string[]) => {
    if (!activeDiscussionId || orchAgents.length < 2) return;
    const discId = activeDiscussionId;
    const controller = new AbortController();
    abortControllers.current[discId] = controller;
    setSendingMap(prev => ({ ...prev, [discId]: true }));
    setOrchState(prev => ({
      ...prev,
      [discId]: { active: true, round: 0, totalRounds: 3, currentAgent: null, agentStreams: [], systemMessages: [] },
    }));

    await discussionsApi.orchestrate(discId, { agents: orchAgents, max_rounds: orchRounds, skill_ids: orchSkillIds, ...(orchDirectiveIds.length > 0 ? { directive_ids: orchDirectiveIds } : {}) }, {
      onSystem: (text) => {
        setOrchState(prev => {
          const s = prev[discId];
          return s ? { ...prev, [discId]: { ...s, systemMessages: [...s.systemMessages, text] } } : prev;
        });
      },
      onRound: (round, total) => {
        setOrchState(prev => {
          const s = prev[discId];
          return s ? { ...prev, [discId]: { ...s, round, totalRounds: total } } : prev;
        });
      },
      onAgentStart: (agent, agentType, round) => {
        setOrchState(prev => {
          const s = prev[discId];
          if (!s) return prev;
          return { ...prev, [discId]: {
            ...s, currentAgent: agent,
            agentStreams: [...s.agentStreams, { agent, agentType, round, text: '', done: false }],
          }};
        });
      },
      onChunk: (text, agent, _agentType, _round) => {
        // Buffer chunks and flush via rAF (same pattern as regular streaming)
        const key = `${discId}:${agent}`;
        orchChunkBuffer.current[key] = (orchChunkBuffer.current[key] ?? '') + text;
        if (orchRafId.current === null) {
          orchRafId.current = requestAnimationFrame(() => {
            orchRafId.current = null;
            const buf = { ...orchChunkBuffer.current };
            orchChunkBuffer.current = {};
            setOrchState(prev => {
              const s = prev[discId];
              if (!s) return prev;
              const streams = s.agentStreams.map(st => {
                const buffered = buf[`${discId}:${st.agent}`];
                return buffered && !st.done ? { ...st, text: (st.text ?? '') + buffered } : st;
              });
              return { ...prev, [discId]: { ...s, agentStreams: streams } };
            });
          });
        }
      },
      onAgentDone: (agent) => {
        setOrchState(prev => {
          const s = prev[discId];
          if (!s) return prev;
          const streams = s.agentStreams.map(st =>
            st.agent === agent && !st.done ? { ...st, done: true } : st
          );
          return { ...prev, [discId]: { ...s, currentAgent: null, agentStreams: streams } };
        });
      },
      onDone: () => {
        setSendingMap(prev => ({ ...prev, [discId]: false }));
        delete abortControllers.current[discId];
        setOrchState(prev => {
          const s = prev[discId];
          return s ? { ...prev, [discId]: { ...s, active: false, currentAgent: null } } : prev;
        });
        refetchDiscussions();
      },
      onError: (error) => {
        console.error('Orchestration error:', error);
        toast(userError(error), 'error');
        setSendingMap(prev => ({ ...prev, [discId]: false }));
        delete abortControllers.current[discId];
        setOrchState(prev => {
          const s = prev[discId];
          return s ? { ...prev, [discId]: { ...s, active: false } } : prev;
        });
        refetchDiscussions();
      },
    }, controller.signal);
  };

  // ─── Render ──────────────────────────────────────────────────────────────
  return (
    <div className="disc-root">
      {/* Sidebar — collapsed mode shows a thin rail with expand button */}
      {!isMobile && sidebarCollapsed ? (
        <div className="disc-sidebar-rail" onClick={() => setSidebarCollapsed(false)} title="Expand sidebar">
          <ChevronRight size={16} />
        </div>
      ) : (!isMobile || sidebarOpen) ? (
        <DiscussionSidebar
          discussions={allDiscussions}
          projects={projects}
          activeId={activeDiscussionId}
          sendingMap={sendingMap}
          lastSeenMsgCount={lastSeenMsgCount}
          onMarkAllRead={markAllDiscussionsSeen}
          contacts={contactsList}
          contactsOnline={contactsOnline}
          wsConnected={wsConnected}
          isMobile={isMobile}
          onSelect={handleDiscSelect}
          onArchive={handleDiscArchive}
          onUnarchive={handleDiscUnarchive}
          onDelete={handleDiscDelete}
          onTogglePin={async (discId, pinned) => {
            try {
              await discussionsApi.update(discId, { pinned });
              refetchDiscussions();
            } catch (e) {
              toast(t('disc.pinError', userError(e)), 'error');
            }
          }}
          onNewDiscussion={() => setShowNewDiscussion(true)}
          onClose={() => setSidebarOpen(false)}
          onContactAdd={handleContactAdd}
          onJoinByCode={handleJoinByCode}
          onStartChat={handleStartChatWithContact}
          onContactDelete={handleContactDelete}
          toast={toast}
          t={t}
          lang={configLanguage ?? 'fr'}
          onStopDiscussion={async (discId) => {
            try {
              const res = await discussionsApi.stop(discId);
              if (res.cancelled) {
                toast(t('disc.stopAgentToast'), 'success');
                // Don't manually clear sendingMap — the backend's cancel
                // path in make_agent_stream finishes its finally-block,
                // saves the partial message, then the WS batch_run_progress
                // (or the normal done event) will tick sendingMap for us.
                // Refetch to pick up the partial response promptly.
                setTimeout(() => refetchDiscussions(), 500);
              } else {
                toast(t('disc.stopAgentNothing'), 'info');
              }
            } catch (e) {
              toast(t('disc.stopAgentError', userError(e)), 'error');
            }
          }}
          batchSummaries={batchSummaries}
          onNavigateWorkflow={(workflowId) => onNavigate('workflows', { workflowId })}
          onDeleteBatch={async (runId, count) => {
            try {
              const res = await workflowsApi.deleteBatchRun(runId);
              toast(t('disc.batchDeletedToast', res.discussions_deleted), 'success');
              refetchDiscussions();
              refetchBatchSummaries();
            } catch (e) {
              toast(t('disc.batchDeleteError', userError(e)), 'error');
              // Touch `count` so the linter accepts it — useful for future
              // optimistic-UI fallbacks if we want to roll back a fake removal.
              void count;
            }
          }}
          onRetryBatch={async (oldRunId, qpId, discIds) => {
            // Rebuild the batch payload from the existing children's
            // title + initial user prompt, then fire a fresh batch via
            // the QP endpoint. The OLD batch is left alone (with its
            // history) — the user can delete it manually if they want.
            // Tya's audit on 2026-05-09 flagged the missing retry surface.
            try {
              const items: { title: string; prompt: string }[] = [];
              for (const did of discIds) {
                const disc = allDiscussions.find(d => d.id === did);
                if (!disc) continue;
                // Need the full disc to read messages[0]. Fetch on demand.
                const full = await discussionsApi.get(did).catch(() => null);
                const firstUser = full?.messages.find(m => m.role === 'User');
                if (firstUser) {
                  items.push({ title: disc.title, prompt: firstUser.content });
                }
              }
              if (items.length === 0) {
                toast(t('disc.batchRetryEmpty'), 'error');
                return;
              }
              const batchName = `Retry · ${items.length} items`;
              await quickPromptsApi.batchRun(qpId, {
                batch_name: batchName,
                items,
                workspace_mode: 'Direct',
              });
              toast(t('disc.batchRetryToast', items.length), 'success');
              refetchDiscussions();
              refetchBatchSummaries();
              void oldRunId; // logged + reserved for future "side-by-side" UI
            } catch (e) {
              toast(t('disc.batchRetryError', userError(e)), 'error');
            }
          }}
          onReviewBatch={openBatchReview}
          collapsedGroups={collapsedDiscGroups}
          onToggleGroup={handleToggleGroup}
          onCollapse={() => setSidebarCollapsed(true)}
        />
      ) : null}

      {/* Main area */}
      <div className="disc-chat-area">
        {/* Global test-mode banner — pinned at the top of the chat area
            regardless of which discussion is active, so switching discs
            doesn't hide the "your main repo is on branch X" state. */}
        {discussionInTestMode && (
          <TestModeBanner
            discussion={discussionInTestMode}
            busy={testModeBusy}
            onExit={() => { void handleExitTestMode(discussionInTestMode.id); }}
            t={t}
          />
        )}
        {/* Preflight modal — opens when `test-mode/enter` responds with a
            blocker. Shows the offending files + lets the user pick the
            remediation path. */}
        {testModeBlocker && testModePendingDiscId && (
          <TestModeModal
            blocker={testModeBlocker}
            busy={testModeBusy}
            onRetry={(opts) => { void handleRequestTestMode(testModePendingDiscId, opts); }}
            onGoCommit={() => {
              setTestModeBlocker(null);
              setTestModePendingDiscId(null);
              setShowGitPanel(true);
            }}
            onCancel={() => { setTestModeBlocker(null); setTestModePendingDiscId(null); }}
            t={t}
          />
        )}
        {/* New discussion form */}
        {showNewDiscussion && (
          <NewDiscussionForm
            projects={projects}
            agents={agents}
            configLanguage={configLanguage}
            agentAccess={agentAccess}
            prefill={prefill}
            onSubmit={handleCreateDiscussion}
            onClose={() => setShowNewDiscussion(false)}
            onPrefillConsumed={onPrefillConsumed}
            onNavigate={(page) => { setShowNewDiscussion(false); onNavigate(page); }}
            t={t}
          />
        )}

        {batchReview && (
          <div className="disc-batch-review-backdrop" role="presentation" onClick={() => setBatchReview(null)}>
            <section
              className="disc-batch-review-panel"
              role="dialog"
              aria-modal="true"
              aria-labelledby="disc-batch-review-title"
              onClick={(e) => e.stopPropagation()}
            >
              <header className="disc-batch-review-head">
                <div>
                  <h2 id="disc-batch-review-title">{t('disc.batchReviewTitle')}</h2>
                  <p>{batchReview.label} · {t('disc.batchReviewCount', batchReview.discIds.length)}</p>
                </div>
                <button type="button" className="btn btn-ghost btn-icon" onClick={() => setBatchReview(null)} aria-label={t('common.close')}>
                  <X size={16} />
                </button>
              </header>

              {batchReviewLoading && (
                <div className="disc-batch-review-state">
                  <Loader2 size={16} className="spin" />
                  {t('disc.batchReviewLoading')}
                </div>
              )}

              {batchReviewError && (
                <div className="disc-batch-review-state" data-kind="error">
                  <AlertTriangle size={16} />
                  {t('disc.batchReviewError', batchReviewError)}
                </div>
              )}

              {!batchReviewLoading && !batchReviewError && (
                <div className="disc-batch-review-table">
                  {batchReviewRows.map((row) => {
                    const result = row.result;
                    const verdict = result?.verdict ?? t('disc.batchReviewNoJson');
                    const confidence = result?.confidence ?? 'n/a';
                    const summary = result?.human_summary ?? row.parseError ?? '';
                    const questions = result?.open_questions?.length ?? 0;
                    return (
                      <article key={row.discussion.id} className="disc-batch-review-row">
                        <div className="disc-batch-review-main">
                          <button type="button" className="disc-batch-review-ticket" onClick={() => setActiveDiscussionId(row.discussion.id)}>
                            {result?.ticket_id || row.discussion.title}
                          </button>
                          <p>{summary}</p>
                        </div>
                        <div className="disc-batch-review-meta">
                          <span className="badge">{verdict}</span>
                          <span className="badge badge-muted">{confidence}</span>
                          <span className="badge badge-muted">{t('disc.batchReviewQuestions', questions)}</span>
                        </div>
                        <div className="disc-batch-review-actions">
                          <button type="button" className="btn btn-sm btn-ghost" onClick={() => setActiveDiscussionId(row.discussion.id)}>
                            <ExternalLink size={14} />
                            {t('disc.batchReviewOpen')}
                          </button>
                          <button type="button" className="btn btn-sm" onClick={() => continueBatchTriage(row)}>
                            <Play size={14} />
                            {t('disc.batchReviewContinue')}
                          </button>
                        </div>
                      </article>
                    );
                  })}
                </div>
              )}
            </section>
          </div>
        )}

        {/* Active discussion chat */}
        {activeDiscussion && !showNewDiscussion ? (
          <>
            <ChatHeader
              discussion={activeDiscussion}
              projects={projects}
              agents={agents}
              availableSkills={availableSkills}
              availableProfiles={availableProfiles}
              availableDirectives={availableDirectives}
              mcpConfigs={mcpConfigs}
              mcpIncompatibilities={mcpIncompatibilities}
              showGitPanel={showGitPanel}
              isMobile={isMobile}
              sending={sending}
              pendingFilesCount={pendingFilesCount}
              onRequestTestMode={() => { void handleRequestTestMode(activeDiscussion.id); }}
              onToggleGitPanel={() => setShowGitPanel(prev => !prev)}
              onToggleSidebar={() => setSidebarOpen(true)}
              onDelete={async (discId) => {
                if (!confirm(t('disc.confirmDelete'))) return;
                await discussionsApi.delete(discId);
                setActiveDiscussionId(null);
                refetchDiscussions();
              }}
              onDiscussionUpdated={handleDiscussionUpdated}
              onAgentSwitch={handleAgentSwitch}
              contacts={contactsList}
              onShare={async (contactIds) => {
                try {
                  await discussionsApi.share(activeDiscussion.id, contactIds);
                  toast(t('contacts.added'), 'success');
                  reloadDiscussion(activeDiscussion.id);
                } catch {
                  toast(t('contacts.addError'), 'error');
                }
              }}
              toast={toast}
              t={t}
            />

            {/* Messages + Git Panel side by side */}
            <div className="disc-messages-git-row">
            <div className="disc-messages-col">

            {/* 0.8.3 (#280) — Audit-running warning. When an audit
                is in progress on the same project, Kronn has filtered
                `.mcp.json` down to the audit allowlist (~5 servers)
                for perf. Discussions opened during this window see
                the filtered subset; this banner explains why so the
                user doesn't think their MCPs vanished. Auto-hides
                when the audit finishes (8 s poll). Skipped for
                system-managed discs (briefing/bootstrap/validation)
                whose own CTAs already drive the flow. */}
            {auditRunningOnActiveProject && activeDiscussion.project_id &&
              !isBriefingDisc(activeDiscussion.title) &&
              !isBootstrapDisc(activeDiscussion.title) &&
              !isValidationDisc(activeDiscussion.title) && (
              <div className="disc-cta-banner" data-variant="info">
                <p className="disc-cta-text" data-variant="info">
                  <Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} />
                  {t('audit.runningMcpFilterBanner')}
                </p>
              </div>
            )}

            {/* 0.8.3 (#276) — Unaudited-project warning banner.
                Persistent banner shown at the top of any discussion on
                a project whose audit hasn't been completed and validated.
                New users discovering Kronn typically don't know there's
                an AI audit step that pre-fills the agent's context — so
                they spend tokens re-explaining their project on every
                turn. This banner guides them through the right flow
                (briefing → audit → validation) BEFORE they burn budget
                on a context-less conversation.

                Hidden for system-managed discussions (briefing /
                bootstrap / validation), which have their own dedicated
                CTAs further down and already drive the user toward the
                next step. Also hidden once the project is `Audited` or
                `Validated` — at that point the context is loaded into
                the agent and the warning is just noise. */}
            {(() => {
              if (!activeDiscussion.project_id) return null;
              if (
                isBriefingDisc(activeDiscussion.title) ||
                isBootstrapDisc(activeDiscussion.title) ||
                isValidationDisc(activeDiscussion.title)
              ) return null;
              const proj = projects.find(p => p.id === activeDiscussion.project_id);
              if (!proj) return null;
              // Only fire on pre-audit states. Once `Audited` or
              // `Validated`, the docs/ tree is loaded into context and
              // the warning is obsolete.
              const unaudited = proj.audit_status === 'NoTemplate'
                || proj.audit_status === 'TemplateInstalled'
                || proj.audit_status === 'Bootstrapped';
              if (!unaudited) return null;

              // Adaptive CTA based on briefing_notes presence.
              // Empty briefing → push toward briefing first (multiplies
              // audit quality by giving the agent business context).
              // Briefing present → user already gave context, can jump
              // straight to launching the audit from the project page.
              const hasBriefing = (proj.briefing_notes ?? '').trim().length > 0;
              const projectId = activeDiscussion.project_id;
              return (
                <div className="disc-cta-banner" data-variant="warning">
                  <p className="disc-cta-text" data-variant="warning">
                    <AlertTriangle size={14} />
                    {hasBriefing
                      ? t('audit.unauditedWarningWithBriefing')
                      : t('audit.unauditedWarning')}
                  </p>
                  {/* Primary CTA adapts. With briefing → invite to
                      launch the audit; without → invite to do the
                      briefing first (which is a 10 min interactive
                      flow that significantly improves audit quality). */}
                  <button
                    className="disc-cta-btn"
                    data-variant="warning"
                    onClick={() => onNavigate('projects', { projectId })}
                  >
                    {hasBriefing ? (
                      <><Play size={12} /> {t('audit.unauditedCtaLaunch')}</>
                    ) : (
                      <><MessageSquare size={12} /> {t('audit.unauditedCtaBriefing')}</>
                    )}
                  </button>
                </div>
              );
            })()}

            {/* Vibe API mode notice */}
            {activeDiscussion.agent === 'Vibe' && (
              <div className="disc-agent-notice" data-agent="Vibe">
                <span>⚠</span>
                <span>Mode API directe — les outils MCP ne sont pas disponibles. Vibe répond en chat uniquement.</span>
              </div>
            )}

            {/* Kiro output notice */}
            {activeDiscussion.agent === 'Kiro' && (
              <div className="disc-agent-notice" data-agent="Kiro">
                <span>ℹ</span>
                <span>Kiro CLI: output may include tool logs. <a href="https://github.com/kirodotdev/Kiro/issues/5006" target="_blank" rel="noopener noreferrer">Tracking issue</a></span>
              </div>
            )}

            {/* Messages */}
            <div
              className="disc-messages"
              ref={messagesContainerRef}
              onScroll={handleMessagesScroll}
            >
              {(() => {
                const msgs = activeDiscussion.messages;
                // Pre-compute indices and timestamps in O(n) instead of O(n²)
                let lastUserIdx = -1;
                for (let i = msgs.length - 1; i >= 0; i--) { if (msgs[i].role === 'User') { lastUserIdx = i; break; } }
                const lastAgentIdx = msgs.length - 1;
                // Pre-compute previous user timestamp per message (for response duration display)
                const prevUserTs: (string | null)[] = [];
                let lastSeenUserTs: string | null = null;
                for (let i = 0; i < msgs.length; i++) {
                  prevUserTs.push(lastSeenUserTs);
                  if (msgs[i].role === 'User') lastSeenUserTs = msgs[i].timestamp;
                }
                // Hide the initial system prompt for automated discussions (briefing, validation, bootstrap).
                // Uses locale-aware detectors — `Briefing` is localized
                // (EN `Project Briefing`, ES `Briefing del proyecto`,
                // FR `Briefing projet`) so a `startsWith('Briefing')`
                // here missed EN and leaked the system prompt as the
                // first visible message for English users.
                const isAutoPrompt = (idx: number) => idx === 0 && msgs[0]?.role === 'User' && (
                  isBriefingDisc(activeDiscussion.title) ||
                  isValidationDisc(activeDiscussion.title) ||
                  isBootstrapDisc(activeDiscussion.title)
                );

                // 0.8.6 phase 4 — group consecutive `[kronn-internal: …]`
                // / `[agent-native: …]` System messages into ONE collapsible
                // banner above the next non-tool message. Pre-fix, a QP
                // that fired 8 tool calls produced 8 separate bubbles
                // between the user prompt and the agent reply ; with this
                // fold the user sees a single "🔧 Outils appelés (8)"
                // line, click to expand. Q1 answer 2026-05-22.
                //
                // The algorithm itself lives in `groupMessagesWithToolFold`
                // (pure fn, unit-tested in discussionMessageGrouping.test.ts) ;
                // here we just map render items to React elements.
                const items = groupMessagesWithToolFold(msgs, { isAutoPrompt });
                return items.map(item => {
                  if (item.kind === 'tool-group') {
                    return (
                      <ToolCallsGroup
                        key={`tools-${item.messages[0].id}`}
                        messages={item.messages}
                        t={t}
                      />
                    );
                  }
                  const { msg, idx } = item;
                  return (
                    <MessageBubble
                      key={msg.id}
                      msg={msg}
                      idx={idx}
                      attachments={attachmentsByMessageId[msg.id] ?? EMPTY_ATTACHMENTS}
                      pendingAttachment={pendingFileMsgIds.has(msg.id)}
                      isLastUser={msg.role === 'User' && idx === lastUserIdx}
                      isLastAgent={msg.role === 'Agent' && idx === lastAgentIdx}
                      isEditing={editingMsgId === msg.id}
                      isCopied={copiedMsgId === msg.id}
                      isTtsActive={ttsPlayingMsgId === msg.id}
                      ttsState={ttsState}
                      isExpandedSummary={expandedSummaryMsgId === msg.id}
                      prevUserTs={prevUserTs[idx]}
                      defaultAgent={activeDiscussion.agent}
                      summaryCache={activeDiscussion.summary_cache ?? null}
                      language={activeDiscussion.language || 'fr'}
                      sending={sending}
                      editingText={editingMsgId === msg.id ? editingText : ''}
                      hasFullAccess={hasFullAccess(msg.agent_type ?? activeDiscussion.agent)}
                      onCopy={handleMsgCopy}
                      onTts={handleMsgTts}
                      onEditStart={handleMsgEditStart}
                      onEditCancel={handleMsgEditCancel}
                      onEditSubmit={handleEditMessage}
                      onEditTextChange={setEditingText}
                      onRetry={handleRetry}
                      onExpandSummary={handleMsgExpandSummary}
                      onNavigate={onNavigate}
                      discussionId={activeDiscussion.id}
                      projectId={activeDiscussion.project_id ?? null}
                      chainableQPs={chainableQPs}
                      onLaunchQp={qp => handleSendMessage(qp.prompt_template)}
                      t={t}
                    />
                  );
                });
              })()}

              {/* Streaming: single agent mode */}
              {sending && !orchState[activeDiscussion.id]?.active && (() => {
                // The pinged agent (e.g. @codex) takes precedence over the
                // discussion's default — without this, the streaming
                // placeholder lies about who's actually responding.
                const streamingAgent = streamingTargetMap[activeDiscussion.id] ?? activeDiscussion.agent;
                return (
                <div className="disc-msg-row" data-role="agent" aria-live="polite">
                  <div className="disc-msg-bubble" data-role="agent">
                    <div className="disc-msg-agent-label" style={{ color: agentColor(streamingAgent), justifyContent: 'space-between' }}>
                      <span style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
                        <Cpu size={10} /> {AGENT_LABELS[streamingAgent] ?? streamingAgent}
                        <Loader2 size={10} style={{ animation: 'spin 1s linear infinite', marginLeft: 4 }} />
                      </span>
                      <span className="disc-streaming-elapsed">
                        {sendingElapsed >= 60
                          ? `${Math.floor(sendingElapsed / 60)}m${String(sendingElapsed % 60).padStart(2, '0')}s`
                          : `${sendingElapsed}s`}
                      </span>
                    </div>
                    {streamingText ? (
                      // Render the streamed buffer as markdown so headings,
                      // tables, code blocks, etc. show progressively instead
                      // of as raw `#` and `**` until the stream finishes.
                      // The renderer is tolerant of half-finished syntax —
                      // an unclosed `**` or code fence just renders the
                      // partial state and snaps into place when the closer
                      // arrives in the next chunk.
                      <div className="disc-streaming-md">
                        <MarkdownContent content={deferredStreamingText} />
                      </div>
                    ) : (
                      <div className="disc-streaming-waiting" aria-live="assertive">
                        <span className="disc-pulse-dot" />
                        {t('disc.running')}
                        {agentLogs.length > 0 && (
                          <span className="disc-streaming-log-hint">
                            — {agentLogs[agentLogs.length - 1]?.slice(0, 60)}
                          </span>
                        )}
                      </div>
                    )}
                    {/* Agent logs panel */}
                    {agentLogs.length > 0 && (
                      <div style={{ marginTop: 6 }}>
                        <button
                          className="disc-logs-toggle"
                          onClick={() => setShowLogs(v => !v)}
                        >
                          <ChevronRight size={10} className="disc-chevron" data-expanded={showLogs} />
                          {t('disc.logs')} ({agentLogs.length})
                        </button>
                        {showLogs && (
                          <div className="disc-logs-panel">
                            {agentLogs.map((log, i) => (
                              <div key={i}>{log}</div>
                            ))}
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                </div>
                );
              })()}

              {/* Streaming: orchestration mode */}
              {orchState[activeDiscussion.id] && (() => {
                const orch = orchState[activeDiscussion.id];
                return (
                  <>
                    {/* Pre-roll placeholder. Between "Lancer le débat" and
                        the first agent's first chunk, agentStreams is
                        empty and the rest of the loop renders nothing —
                        the user sees a frozen UI for several seconds
                        (agent CLI cold-start, especially Codex). Show a
                        clear "debate launching" pulse + any system
                        messages we've already received so the user knows
                        it's actually running. The placeholder hides as
                        soon as the first agent stream lands. */}
                    {orch.active && orch.agentStreams.length === 0 && (
                      <div className="disc-msg-row" data-role="agent" aria-live="polite">
                        <div className="disc-msg-bubble" data-role="agent">
                          <div className="disc-streaming-waiting">
                            <span className="disc-pulse-dot" />
                            {t('disc.debateLaunching')}
                          </div>
                          {orch.systemMessages.length > 0 && (
                            <ul className="disc-orch-systems" style={{ marginTop: 6, paddingLeft: 18, fontSize: 11, color: 'var(--kr-text-muted)' }}>
                              {orch.systemMessages.slice(-5).map((m, i) => (
                                <li key={i}>{m}</li>
                              ))}
                            </ul>
                          )}
                        </div>
                      </div>
                    )}
                    {/* When agent streams have started but new system
                        messages keep arriving (round transitions, status
                        notes), show them as a compact strip above the
                        latest stream so the user has continuous feedback. */}
                    {orch.systemMessages.length > 0 && orch.agentStreams.length > 0 && (
                      <div className="disc-orch-systems-strip" style={{ fontSize: 11, color: 'var(--kr-text-muted)', padding: '4px 0', borderLeft: '2px solid var(--kr-border-faint)', paddingLeft: 8, marginBottom: 4 }}>
                        {orch.systemMessages[orch.systemMessages.length - 1]}
                      </div>
                    )}
                    {orch.agentStreams.map((as_, i) => (
                      <div key={i} className="disc-msg-row" data-role="agent">
                        <div
                          className="disc-msg-bubble" data-role="agent"
                          style={{ borderLeft: `3px solid ${agentColor(as_.agentType || as_.agent)}` }}
                        >
                          <div className="disc-orch-stream-agent" style={{ color: agentColor(as_.agentType || as_.agent) }}>
                            <Cpu size={10} /> {as_.agent}
                            <span className="disc-orch-round">
                              {as_.round === 'synthesis' ? t('disc.synthesis') : `Round ${as_.round}`}
                            </span>
                            {!as_.done && <Loader2 size={9} style={{ animation: 'spin 1s linear infinite', marginLeft: 4 }} />}
                          </div>
                          {as_.text ? (
                            <MarkdownContent content={as_.text} />
                          ) : !as_.done ? (
                            <div className="disc-orch-thinking">
                              {t('disc.thinking', as_.agent)}
                            </div>
                          ) : (
                            // Agent finished with NO chunks — typically a CLI
                            // that crashed before printing or returned empty.
                            // Without an explicit message the user sees a
                            // ghost bubble and can't tell whether to retry.
                            <div className="disc-orch-empty" style={{ fontSize: 12, color: 'var(--kr-warning)', fontStyle: 'italic' }}>
                              ⚠️ {t('disc.debateAgentEmpty', as_.agent)}
                            </div>
                          )}
                        </div>
                      </div>
                    ))}
                  </>
                );
              })()}

              {/* Briefing complete banner — CTA adapts to project audit state */}
              {(() => {
                if (!isBriefingDisc(activeDiscussion.title) || !activeDiscussion.project_id) return null;
                // Check ONLY agent messages (not the prompt at index 0 which contains the marker text as instructions)
                const agentMsgs = activeDiscussion.messages.filter((m, idx) => m.role === 'Agent' && idx > 0);
                const lastAgentMsg = agentMsgs.length > 0 ? agentMsgs[agentMsgs.length - 1] : null;
                const isComplete = lastAgentMsg && lastAgentMsg.content.toUpperCase().includes('KRONN:BRIEFING_COMPLETE');
                if (!isComplete) return null;

                const proj = projects.find(p => p.id === activeDiscussion.project_id);
                const projDiscs = allDiscussions.filter(d => d.project_id === activeDiscussion.project_id && d.id !== activeDiscussion.id);
                const validationDisc = projDiscs.find(d => d.title === 'Validation audit AI');
                const auditDone = proj && (proj.audit_status === 'Audited' || proj.audit_status === 'Validated');

                // State 3: Audit done + validation discussion exists → go to validation
                if (auditDone && validationDisc) {
                  return (
                    <div className="disc-cta-banner" data-variant="accent">
                      <p className="disc-cta-text" data-variant="accent">
                        <ShieldCheck size={14} /> {t('audit.auditDoneResume')}
                      </p>
                      <button className="disc-cta-btn" data-variant="accent" onClick={() => { setActiveDiscussionId(validationDisc.id); }}>
                        <MessageSquare size={12} /> {t('audit.resumeValidation')}
                      </button>
                    </div>
                  );
                }

                // State 2: Audit done but no validation yet (just finished, validation being created)
                if (auditDone && !validationDisc) {
                  return (
                    <div className="disc-cta-banner" data-variant="warning">
                      <p className="disc-cta-text" data-variant="warning">
                        <Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} /> {t('audit.auditInProgress')}
                      </p>
                      <button className="disc-cta-btn" data-variant="warning" onClick={() => { if (activeDiscussion.project_id) onNavigate('projects', { projectId: activeDiscussion.project_id }); }}>
                        <Play size={12} /> {t('audit.goToProject')}
                      </button>
                    </div>
                  );
                }

                // State 1: Briefing done, no audit yet → go launch audit
                return (
                  <div className="disc-cta-banner" data-variant="info">
                    <p className="disc-cta-text" data-variant="info">
                      <Check size={14} /> {t('audit.briefingDone')}
                    </p>
                    <button className="disc-cta-btn" data-variant="info" onClick={() => { if (activeDiscussion.project_id) onNavigate('projects', { projectId: activeDiscussion.project_id }); }}>
                      <Play size={12} /> {t('audit.goToProject')}
                    </button>
                  </div>
                );
              })()}

              {/* Validation complete banner */}
              {(() => {
                if (activeDiscussion.title !== 'Validation audit AI' || !activeDiscussion.project_id) return null;
                const proj = projects.find(p => p.id === activeDiscussion.project_id);
                if (!proj || proj.audit_status !== 'Audited') return null;
                const valAgentMsgs = activeDiscussion.messages.filter((m, idx) => m.role === 'Agent' && idx > 0);
                const lastAgentMsg = valAgentMsgs.length > 0 ? valAgentMsgs[valAgentMsgs.length - 1] : null;
                const isComplete = lastAgentMsg && lastAgentMsg.content.toUpperCase().includes('KRONN:VALIDATION_COMPLETE');
                if (!isComplete) return null;
                return (
                  <div className="disc-cta-banner" data-variant="accent">
                    <p className="disc-cta-text" data-variant="accent">
                      <ShieldCheck size={14} /> {t('audit.validationComplete')}
                    </p>
                    <button className="disc-cta-btn" data-variant="accent" onClick={async () => { await projectsApi.validateAudit(proj.id); refetchProjects(); refetchDiscussions(); }}>
                      <Check size={12} /> {t('audit.markValid')}
                    </button>
                  </div>
                );
              })()}

              {/* 0.8.2 — AutoPilot CTA. Surfaces after VALIDATION_COMPLETE
                  regardless of audit_status: the user may not have hit
                  "Mark valid" yet, but the agent's already done the work.
                  Closes the loop: audit → questions → tickets → AutoPilot.
                  Always offered (no team-size gate) — solo devs are the
                  biggest beneficiaries of mobile-issue → AutoPilot. */}
              {(() => {
                if (activeDiscussion.title !== 'Validation audit AI' || !activeDiscussion.project_id) return null;
                const valAgentMsgs = activeDiscussion.messages.filter((m, idx) => m.role === 'Agent' && idx > 0);
                const lastAgentMsg = valAgentMsgs.length > 0 ? valAgentMsgs[valAgentMsgs.length - 1] : null;
                const isComplete = lastAgentMsg && lastAgentMsg.content.toUpperCase().includes('KRONN:VALIDATION_COMPLETE');
                if (!isComplete) return null;
                if (!onLaunchWorkflowFromPreset) return null;
                const projectId = activeDiscussion.project_id;
                return (
                  <div className="disc-cta-banner" data-variant="accent">
                    <p className="disc-cta-text" data-variant="accent">
                      <Zap size={14} /> {t('audit.autopilotCtaText')}
                    </p>
                    <button
                      className="disc-cta-btn"
                      data-variant="accent"
                      onClick={() => {
                        // 0.8.3 — Switched from `ticket-to-pr` to
                        // `feasibility-autopilot`. The new preset adds
                        // a triage step that classifies every sub-task
                        // into doable / decided / mocked / blocked
                        // BEFORE any code is written, plus a
                        // drift_check Exec that surfaces every freedom
                        // the agent took. Required for big tickets
                        // where silent improvisation breaks the
                        // implementation (cf. EW-7247 stress test).
                        onLaunchWorkflowFromPreset('feasibility-autopilot', projectId);
                        onNavigate('workflows');
                      }}
                    >
                      <Rocket size={12} /> {t('audit.autopilotCtaBtn')}
                    </button>
                  </div>
                );
              })()}

              {/* Bootstrap complete banner */}
              {(() => {
                if (!isBootstrapDisc(activeDiscussion.title) || !activeDiscussion.project_id) return null;
                const proj = projects.find(p => p.id === activeDiscussion.project_id);
                if (!proj || proj.audit_status !== 'TemplateInstalled') return null;
                const bootAgentMsgs = activeDiscussion.messages.filter((m, idx) => m.role === 'Agent' && idx > 0);
                const lastAgentMsg = bootAgentMsgs.length > 0 ? bootAgentMsgs[bootAgentMsgs.length - 1] : null;
                // Also check streamingMap for the signal (may not be in messages yet)
                const streamedText = streamingMap[activeDiscussion.id] ?? '';
                const isComplete = (lastAgentMsg && lastAgentMsg.content.toUpperCase().includes('KRONN:BOOTSTRAP_COMPLETE'))
                  || streamedText.toUpperCase().includes('KRONN:BOOTSTRAP_COMPLETE');
                if (!isComplete) return null;
                return (
                  <div className="disc-cta-banner" data-variant="accent">
                    <p className="disc-cta-text" data-variant="accent">
                      <Rocket size={14} /> {t('audit.bootstrapComplete')}
                    </p>
                    <button className="disc-cta-btn" data-variant="accent" onClick={async () => { await projectsApi.markBootstrapped(proj.id); refetchProjects(); refetchDiscussions(); }}>
                      <Check size={12} /> {t('audit.markBootstrapped')}
                    </button>
                  </div>
                );
              })()}

              {/* 0.8.3 — Bundle ready banner (KRONN:BUNDLE_READY).
                  Atomic creation of N Quick Prompts + N Quick APIs +
                  N Custom APIs + 1 Workflow. Takes precedence over
                  the legacy WORKFLOW_READY block below — they're
                  mutually exclusive in well-formed agent outputs. */}
              {(() => {
                const agentMsgs = activeDiscussion.messages.filter((m, idx) => m.role === 'Agent' && idx > 0);
                const wfStreamedText = streamingMap[activeDiscussion.id] ?? '';
                const readyMsg = [...agentMsgs].reverse().find(m => m.content.toUpperCase().includes('KRONN:BUNDLE_READY'))
                  || (wfStreamedText.toUpperCase().includes('KRONN:BUNDLE_READY') ? { content: wfStreamedText } : null);
                if (!readyMsg) return null;
                const jsonMatch = readyMsg.content.match(/```json\s*\n([\s\S]*?)\n```/);
                if (!jsonMatch) return null;
                let parsedPayload: unknown;
                try { parsedPayload = JSON.parse(jsonMatch[1]); } catch { return null; }
                if (!parsedPayload || typeof parsedPayload !== 'object' || !('workflow' in (parsedPayload as Record<string, unknown>))) return null;
                const payload = parsedPayload as Record<string, unknown>;
                // Inject project_id from discussion context on the
                // wrapped workflow if the agent left it null. We do
                // NOT inject on each QP/QA — those default to
                // global-scope when unspecified, which is what an
                // agent-emitted bundle typically wants.
                const inner = (payload.workflow ?? {}) as Record<string, unknown>;
                if (!inner.project_id && activeDiscussion.project_id) {
                  inner.project_id = activeDiscussion.project_id;
                  payload.workflow = inner;
                }
                const qpCount = Array.isArray(payload.quick_prompts) ? payload.quick_prompts.length : 0;
                const qaCount = Array.isArray(payload.quick_apis) ? payload.quick_apis.length : 0;
                const caCount = Array.isArray(payload.custom_apis) ? payload.custom_apis.length : 0;
                const extras = qpCount + qaCount + caCount;
                return (
                  <div className="disc-cta-banner" data-variant="accent">
                    <p className="disc-cta-text" data-variant="accent">
                      <Zap size={14} /> {t('wf.aiBundleReady', qpCount, qaCount, caCount)}
                    </p>
                    <button className="disc-cta-btn" data-variant="accent" onClick={async () => {
                      try {
                        const resp = await workflowsApi.createBundle(payload);
                        console.info('Bundle created:', resp);
                        onNavigate('workflows');
                      } catch (e) {
                        console.warn('Failed to create bundle:', e);
                      }
                    }}>
                      <Check size={12} /> {t('wf.createBundleBtn', extras)}
                    </button>
                  </div>
                );
              })()}

              {/* Workflow ready banner (legacy single-workflow path) */}
              {(() => {
                const agentMsgs = activeDiscussion.messages.filter((m, idx) => m.role === 'Agent' && idx > 0);
                const wfStreamedText = streamingMap[activeDiscussion.id] ?? '';
                // If a BUNDLE_READY is also present, the bundle path
                // above wins — skip rendering the legacy button so
                // the user doesn't see two buttons.
                const hasBundle = [...agentMsgs].some(m => m.content.toUpperCase().includes('KRONN:BUNDLE_READY'))
                  || wfStreamedText.toUpperCase().includes('KRONN:BUNDLE_READY');
                if (hasBundle) return null;
                const readyMsg = [...agentMsgs].reverse().find(m => m.content.toUpperCase().includes('KRONN:WORKFLOW_READY'))
                  || (wfStreamedText.toUpperCase().includes('KRONN:WORKFLOW_READY') ? { content: wfStreamedText } : null);
                if (!readyMsg) return null;
                const jsonMatch = readyMsg.content.match(/```json\s*\n([\s\S]*?)\n```/);
                if (!jsonMatch) return null;
                let parsedPayload: unknown;
                try { parsedPayload = JSON.parse(jsonMatch[1]); } catch { return null; }
                if (!parsedPayload || typeof parsedPayload !== 'object' || !('steps' in (parsedPayload as Record<string, unknown>))) return null;
                // Inject project_id from discussion context if the agent left it null
                const payload = parsedPayload as Record<string, unknown>;
                if (!payload.project_id && activeDiscussion.project_id) {
                  payload.project_id = activeDiscussion.project_id;
                }
                return (
                  <div className="disc-cta-banner" data-variant="accent">
                    <p className="disc-cta-text" data-variant="accent">
                      <Zap size={14} /> {t('wf.aiWorkflowReady')}
                    </p>
                    <button className="disc-cta-btn" data-variant="accent" onClick={async () => {
                      try {
                        await workflowsApi.create(payload as unknown as Parameters<typeof workflowsApi.create>[0]);
                        onNavigate('workflows');
                      } catch (e) {
                        console.warn('Failed to create workflow:', e);
                      }
                    }}>
                      <Check size={12} /> {t('wf.createThisWorkflow')}
                    </button>
                  </div>
                );
              })()}

              {/* 0.8.5 — QP AI Improver banner.
                  Triggered when the agent emits `KRONN:QP_IMPROVED`
                  inside a discussion whose title starts with the
                  `[Improve QP <id>]` prefix WorkflowsPage.handleImproveQP
                  uses. Recovers the target QP id from the title, parses
                  the JSON the agent emitted, and offers a one-click
                  PUT /api/quick-prompts/:id. We deliberately do NOT
                  trust the JSON's `id` field — the agent could hallucinate
                  it; the title is the source of truth. */}
              {(() => {
                const titleMatch = activeDiscussion.title.match(/^\[Improve QP (qp-[^\]]+|[0-9a-f-]+)\]/i);
                if (!titleMatch) return null;
                const qpTargetId = titleMatch[1];

                // Persisted "already deployed" marker (0.8.4 follow-up).
                // Without this, the banner re-derives from the agent
                // message content on every render — and that content
                // still has the KRONN:QP_IMPROVED signal + JSON block
                // forever, so the CTA stayed active after a successful
                // deploy. Now we stash the resulting version index in
                // localStorage and render the disabled "✅ déployé"
                // state instead. Cf. [[project_qp_ai_improver]].
                const deployedVersion = getDeployedVersion(activeDiscussion.id);
                if (deployedVersion != null) {
                  return (
                    <div className="disc-cta-banner" data-variant="accent">
                      <p className="disc-cta-text" data-variant="accent">
                        {t('qp.deployedAtVersion', String(deployedVersion))}
                      </p>
                    </div>
                  );
                }

                const agentMsgs = activeDiscussion.messages.filter((m, idx) => m.role === 'Agent' && idx > 0);
                const wfStreamedText = streamingMap[activeDiscussion.id] ?? '';
                const readyMsg = [...agentMsgs].reverse().find(m => m.content.toUpperCase().includes('KRONN:QP_IMPROVED'))
                  || (wfStreamedText.toUpperCase().includes('KRONN:QP_IMPROVED') ? { content: wfStreamedText } : null);
                if (!readyMsg) return null;
                const jsonMatch = readyMsg.content.match(/```json\s*\n([\s\S]*?)\n```/);
                if (!jsonMatch) return null;
                let parsedPayload: Record<string, unknown> | null = null;
                try {
                  const v: unknown = JSON.parse(jsonMatch[1]);
                  if (v && typeof v === 'object') parsedPayload = v as Record<string, unknown>;
                } catch { return null; }
                if (!parsedPayload) return null;
                // 0.8.6 — sanitize the agent-emitted payload before
                // the PUT. Strips `id`, coerces `null` to backend
                // defaults on non-Option fields, normalises `tier`.
                // Cf. lib/qp-improver-sanitize.ts for the full
                // contract + unit tests.
                sanitizeQpImproverPayload(parsedPayload);

                const isDeploying = deployingQpDiscId === activeDiscussion.id;

                return (
                  <div className="disc-cta-banner" data-variant="accent">
                    <p className="disc-cta-text" data-variant="accent">
                      <Zap size={14} /> {t('qp.improvedReady')}
                    </p>
                    <button
                      className="disc-cta-btn"
                      data-variant="accent"
                      disabled={isDeploying}
                      onClick={async () => {
                        // useRef guard against a fast double-click — the
                        // `disabled` prop is closure-stale for ~1 paint
                        // after setState. Cf. [[feedback_race_guards]].
                        if (deployingQpRef.current) return;
                        deployingQpRef.current = true;
                        setDeployingQpDiscId(activeDiscussion.id);
                        try {
                          await quickPromptsApi.update(qpTargetId, parsedPayload as unknown as Parameters<typeof quickPromptsApi.update>[1]);
                          // Look up the freshly-snapshotted version index.
                          // `history` returns versions newest-first; the
                          // first entry IS the one we just wrote. If for
                          // any reason the history call fails (network
                          // flake mid-deploy), we still claim success and
                          // mark with `?` rather than blocking the nav.
                          let newVersion: number | null = null;
                          try {
                            const versions = await quickPromptsApi.history(qpTargetId);
                            if (versions.length > 0) newVersion = versions[0].version_index;
                          } catch { /* non-fatal — version display falls back */ }
                          if (newVersion != null) {
                            setDeployedVersion(activeDiscussion.id, newVersion);
                            toast(t('qp.deploySuccess', String(newVersion)), 'success');
                          }
                          // 0.8.5 follow-up — deep-link to the QP card on
                          // the Quick Prompts tab. WorkflowsPage reads
                          // this key on mount: switches tab + scrolls
                          // to the matching card + flashes a highlight.
                          try {
                            sessionStorage.setItem('kronn:postQpImproved', qpTargetId);
                          } catch { /* private-mode / quota — fall through */ }
                          onNavigate('workflows');
                        } catch (e) {
                          // 0.8.4 follow-up — pre-fix this was a silent
                          // `console.warn`, so a 400 from the backend
                          // (agent-emitted JSON missing required fields,
                          // invalid agent enum, etc.) looked to the user
                          // like "click does nothing".
                          //
                          // 0.8.6 — also log the FULL error to DevTools
                          // for power-user debug. userError now surfaces
                          // "Server error (HTTP 422) — …" verbatim in
                          // the toast, but the agent-emitted payload
                          // itself is worth keeping in the console too.
                          console.error('Deploy QP improver failed:', e, 'payload:', parsedPayload);
                          toast(t('qp.deployFailed', userError(e)), 'error');
                        } finally {
                          deployingQpRef.current = false;
                          setDeployingQpDiscId(null);
                        }
                      }}
                    >
                      {isDeploying ? (
                        <>
                          <Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} /> {t('qp.deployInProgress')}
                        </>
                      ) : (
                        <>
                          <Check size={12} /> {t('qp.deployImproved')}
                        </>
                      )}
                    </button>
                  </div>
                );
              })()}

              {/* Bootstrap++ gated validation banners */}
              {(() => {
                if (!isBootstrapDisc(activeDiscussion.title)) return null;
                const bAgentMsgs = activeDiscussion.messages.filter((m, idx) => m.role === 'Agent' && idx > 0);
                const lastBMsg = bAgentMsgs.length > 0 ? bAgentMsgs[bAgentMsgs.length - 1] : null;
                const bStreamedText = streamingMap[activeDiscussion.id] ?? '';
                // While the user is waiting for the agent's NEW reply (just clicked
                // a "Validate" CTA → sendingMap is set OR streamed text starts
                // arriving), we must ignore the previous saved message — otherwise
                // the OLD signal in lastBMsg.content keeps the OLD banner visible
                // even after the user moved on. The right "lastContent" during a
                // run is the new streaming buffer alone; once the run ends and
                // sendingMap clears, the fresh message in DB takes over.
                const isAwaitingReply = !!sendingMap[activeDiscussion.id] || bStreamedText.length > 0;
                const lastContent = isAwaitingReply
                  ? bStreamedText
                  : (lastBMsg?.content ?? '');
                if (!lastContent) return null;
                const upper = lastContent.toUpperCase();

                if (upper.includes('KRONN:REPO_READY')) {
                  return (
                    <div className="disc-cta-banner" data-variant="info">
                      <p className="disc-cta-text" data-variant="info">
                        <Check size={14} /> {t('bootstrap.repoReady')}
                      </p>
                      <button className="disc-cta-btn" data-variant="info" onClick={() => {
                        handleSendMessage(t('bootstrap.repoValidated'));
                      }}>
                        <Play size={12} /> {t('bootstrap.analyzeArchitecture')}
                      </button>
                    </div>
                  );
                }

                if (upper.includes('KRONN:ARCHITECTURE_READY')) {
                  return (
                    <div className="disc-cta-banner" data-variant="info">
                      <p className="disc-cta-text" data-variant="info">
                        <Check size={14} /> {t('bootstrap.architectureReady')}
                      </p>
                      <button className="disc-cta-btn" data-variant="info" onClick={() => {
                        handleSendMessage(t('bootstrap.architectureValidated'));
                      }}>
                        <Play size={12} /> {t('bootstrap.generatePlan')}
                      </button>
                    </div>
                  );
                }

                // STRUCTURE_READY is treated as a PLAN_READY alias — LLM
                // hallucinates it when Stage 2 produces a structural breakdown
                // (e.g. "modules Core/Dilem/Shared, 15 chantiers") rather than
                // an explicit "plan" header. Same CTA fires the issue creation.
                if (upper.includes('KRONN:PLAN_READY') || upper.includes('KRONN:STRUCTURE_READY')) {
                  return (
                    <div className="disc-cta-banner" data-variant="accent">
                      <p className="disc-cta-text" data-variant="accent">
                        <Check size={14} /> {t('bootstrap.planReady')}
                      </p>
                      <button className="disc-cta-btn" data-variant="accent" onClick={() => {
                        handleSendMessage(t('bootstrap.planValidated'));
                      }}>
                        <Play size={12} /> {t('bootstrap.createIssues')}
                      </button>
                    </div>
                  );
                }

                // Accept both ISSUES_READY (canonical, *_READY family) and
                // ISSUES_CREATED (legacy / what older skill versions used).
                // Claude regularly hallucinates one when the skill says the
                // other — covering both gives us a stable banner regardless.
                if (upper.includes('KRONN:ISSUES_READY') || upper.includes('KRONN:ISSUES_CREATED')) {
                  const proj = projects.find(p => p.id === activeDiscussion.project_id);
                  return (
                    <div className="disc-cta-banner" data-variant="accent">
                      <p className="disc-cta-text" data-variant="accent">
                        <Check size={14} /> {t('bootstrap.issuesCreated')}
                      </p>
                      {/* Dev kickoff (0.7.1) — closes the bootstrap_plus flow.
                       *  Pre-fix the user had to manually create a new
                       *  discussion or fire a Batch QP to start coding on
                       *  the first issue. This CTA opens NewDiscussionForm
                       *  prefilled with a localized "start coding on
                       *  issue #1" prompt, scoped to the same project,
                       *  unlocked so the user can tweak before sending. */}
                      {proj && onSetDiscPrefill && (
                        <button
                          className="disc-cta-btn"
                          data-variant="accent"
                          onClick={() => {
                            onSetDiscPrefill({
                              projectId: proj.id,
                              title: t('bootstrap.devKickoffTitle'),
                              prompt: t('bootstrap.devKickoffPrompt'),
                              // Unlocked: the user often wants to edit (pick
                              // a different issue number, add constraints,
                              // etc.) before firing.
                              locked: false,
                            });
                            onNavigate('discussions', { projectId: proj.id });
                          }}
                        >
                          <Play size={12} /> {t('bootstrap.startDev')}
                        </button>
                      )}
                      <button className="disc-cta-btn" data-variant="accent" onClick={() => {
                        if (proj) onNavigate('projects', { projectId: proj.id });
                      }}>
                        <Check size={12} /> {t('bootstrap.viewProject')}
                      </button>
                    </div>
                  );
                }

                return null;
              })()}

              <div ref={chatEndRef} />
            </div>

            {/* Floating "↓ New messages" pill — appears when the user is
                scrolled up while new content (a new message OR streaming
                chunks) is arriving. Click jumps back to the bottom and
                re-engages auto-scroll. */}
            {!stickToBottom && hasNewWhileScrolledUp && (
              <button
                className="disc-jump-to-bottom"
                onClick={() => {
                  setStickToBottom(true);
                  setHasNewWhileScrolledUp(false);
                  chatEndRef.current?.scrollIntoView({ behavior: 'smooth' });
                }}
                aria-label="Jump to latest message"
              >
                ↓ {t('disc.newContent')}
              </button>
            )}

            {/* Persistent jump arrows. Render as a small vertical stack in
             *  the bottom-right of the message list. They appear only when
             *  there's actually scroll room in that direction — short
             *  discussions stay clean. User feedback on 2026-05-09:
             *  "quand on est au début ou à la fin, on doit scroller à
             *  mort pour aller à l'autre bout, pas moyen d'avoir une
             *  petite flèche pour y aller direct". */}
            <div className="disc-scroll-jump-stack" aria-hidden={!scrolledFromTop && stickToBottom}>
              {scrolledFromTop && (
                <button
                  type="button"
                  className="disc-scroll-jump-btn"
                  title={t('disc.scrollToTop')}
                  aria-label={t('disc.scrollToTop')}
                  onClick={() => {
                    const el = messagesContainerRef.current;
                    if (el) el.scrollTo({ top: 0, behavior: 'smooth' });
                  }}
                >
                  ↑
                </button>
              )}
              {!stickToBottom && (
                <button
                  type="button"
                  className="disc-scroll-jump-btn"
                  data-has-new={hasNewWhileScrolledUp}
                  title={t('disc.scrollToBottom')}
                  aria-label={t('disc.scrollToBottom')}
                  onClick={() => {
                    setStickToBottom(true);
                    setHasNewWhileScrolledUp(false);
                    chatEndRef.current?.scrollIntoView({ behavior: 'smooth' });
                  }}
                >
                  ↓
                </button>
              )}
            </div>

            {/* Disabled agent banner */}
            {activeAgentDisabled && activeDiscussion && (
              <div className="disc-agent-disabled-banner">
                <AlertTriangle size={12} style={{ color: 'var(--kr-warning)', flexShrink: 0 }} />
                <span className="disc-agent-disabled-text">
                  {t('disc.agentDisabled', AGENT_LABELS[activeDiscussion.agent] ?? activeDiscussion.agent)}
                  {' — '}
                  <span
                    style={{ cursor: 'pointer', textDecoration: 'underline' }}
                    onClick={() => onNavigate('settings')}
                  >{t('disc.agentDisabledLink')}</span>
                </span>
              </div>
            )}

            {/* Structured agent questions form (0.3.5) — surfaced when the
                latest agent message contains `{{var}}: question` entries.
                Hidden while the agent is still streaming (the pattern might
                not be complete yet) and while a send is in flight. */}
            {(() => {
              if (!activeDiscussion) return null;
              if (sendingMap[activeDiscussion.id]) return null;
              const streaming = streamingMap[activeDiscussion.id];
              if (streaming && streaming.length > 0) return null;
              const agentMsgs = activeDiscussion.messages.filter(m => m.role === 'Agent');
              const lastAgent = agentMsgs.length > 0 ? agentMsgs[agentMsgs.length - 1] : null;
              const lastMsg = activeDiscussion.messages[activeDiscussion.messages.length - 1];
              // Only show if the VERY last message is the agent one (user
              // hasn't replied yet — otherwise the form is stale).
              if (!lastAgent || lastMsg !== lastAgent) return null;
              const questions = parseAgentQuestions(lastAgent.content);
              if (questions.length === 0) return null;
              return (
                <AgentQuestionForm
                  questions={questions}
                  discussionId={activeDiscussion.id}
                  onSubmit={(reply) => handleSendMessage(reply)}
                  t={t}
                />
              );
            })()}

            {/* Queued follow-ups — messages the user typed while the agent is
                still streaming. Rendered as ghost "outbox" bubbles just above
                the composer; they auto-fire one-by-one as each response
                completes (useMessageQueue). Each can be cancelled via its ✕. */}
            {queuedMessages.length > 0 && (
              <div className="disc-queued-msgs" aria-label={t('disc.queuedAria')}>
                {/* ONE growing "outbox" bubble — each queued part on its own
                    line, all sent together as a single merged message. */}
                <div className="disc-queued-bubble" title={t('disc.queuedHint')}>
                  <div className="disc-queued-head">
                    <Clock size={11} className="disc-queued-clock" />
                    <span className="disc-queued-label">{t('disc.queuedAria')}</span>
                    {queuedMessages.length > 1 && (
                      <button
                        type="button"
                        className="disc-queued-clear"
                        onClick={clearMessageQueue}
                        aria-label={t('disc.queuedClearAll')}
                        title={t('disc.queuedClearAll')}
                      >
                        {t('disc.queuedClearAll')}
                      </button>
                    )}
                  </div>
                  {queuedMessages.map(qm => (
                    <div key={qm.id} className="disc-queued-line">
                      {qm.targetAgent && <span className="disc-queued-agent">@{qm.targetAgent}</span>}
                      <span className="disc-queued-text">{qm.content}</span>
                      <button
                        type="button"
                        className="disc-queued-cancel"
                        onClick={() => removeQueuedMessage(qm.id)}
                        aria-label={t('disc.queuedCancel')}
                        title={t('disc.queuedCancel')}
                      >
                        <X size={11} />
                      </button>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Input — unified composer.
                `key={activeDiscussion.id}` forces a fresh mount whenever the
                user switches discussions. Without this, React re-uses the
                same ChatInput instance and the non-controlled textarea keeps
                its DOM value from the previous discussion — reported as the
                "même message dans toutes les discussions" bug on 2026-04-15.
                Remount is cheap here and also gives us a clean reset of
                mention popover / emoji popover / voice mode / draft hydration. */}
            <ChatInput
              key={activeDiscussion.id}
              discussion={activeDiscussion}
              agents={agents}
              sending={sending}
              disabled={activeAgentDisabled}
              ttsEnabled={ttsEnabled}
              ttsState={ttsState}
              worktreeError={worktreeError}
              availableSkills={availableSkills}
              availableDirectives={availableDirectives}
              onSend={handleSendMessage}
              onStop={handleStop}
              onOrchestrate={handleOrchestrate}
              onTtsToggle={handleTtsToggle}
              onWorktreeErrorDismiss={() => setWorktreeError(null)}
              onWorktreeRetry={handleWorktreeRetry}
              isAgentRestricted={isAgentRestricted}
              contextFiles={pendingContextFiles}
              onUploadFiles={handleUploadFiles}
              onDeleteContextFile={handleDeleteContextFile}
              uploadingFiles={uploadingFiles}
              chainableQPs={chainableQPs}
              queuedQP={queuedQP}
              onQueueQP={queueQP}
              onCancelQueuedQP={cancelQueuedQP}
              toast={toast}
              t={t}
            />

            </div>{/* end messages column */}

            {/* Git Panel (side panel) */}
            {showGitPanel && activeDiscussion.project_id && (
              <GitPanel
                projectId={activeDiscussion.project_id}
                discussionId={activeDiscussion.workspace_mode === 'Isolated' ? activeDiscussion.id : undefined}
                onClose={() => setShowGitPanel(false)}
                terminalEnabled={agentAccess ? Object.values(agentAccess).some(v => (v as { full_access?: boolean } | undefined)?.full_access) : false}
              />
            )}

            </div>{/* end flex row (messages + git panel) */}
          </>
        ) : !showNewDiscussion && (
          <div className="disc-placeholder">
            {isMobile && (
              <button
                className="disc-placeholder-menu-btn"
                onClick={() => setSidebarOpen(true)}
                aria-label="Open sidebar"
              >
                <Menu size={20} />
              </button>
            )}
            <MessageSquare size={48} style={{ marginBottom: 16, opacity: 0.3 }} />
            <p style={{ fontSize: 14 }}>{t('disc.selectOrCreate')}</p>
          </div>
        )}
      </div>
    </div>
  );
}
