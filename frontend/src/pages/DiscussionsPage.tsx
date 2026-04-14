import { useState, useRef, useEffect, useCallback, useMemo } from 'react';
import './DiscussionsPage.css';
import { MessageBubble, MarkdownContent } from '../components/MessageBubble';
import { ChatInput } from '../components/ChatInput';
import { discussions as discussionsApi, projects as projectsApi, skills as skillsApi, profiles as profilesApi, directives as directivesApi, contacts as contactsApi, workflows as workflowsApi } from '../lib/api';
import { GitPanel } from '../components/GitPanel';
import { ChatHeader } from '../components/ChatHeader';
import { DiscussionSidebar } from '../components/DiscussionSidebar';
import { NewDiscussionForm } from '../components/NewDiscussionForm';
import type { NewDiscConfig } from '../components/NewDiscussionForm';
import { AgentQuestionForm } from '../components/AgentQuestionForm';
import { parseAgentQuestions } from '../lib/agent-question-parse';
import { userError } from '../lib/userError';
import type { Project, AgentDetection, Discussion, AgentType, AgentsConfig, Skill, AgentProfile, Directive, McpConfigDisplay, McpIncompatibility, Contact, WsMessage, ContextFile } from '../types/generated';
import { useWebSocket } from '../hooks/useWebSocket';
import { useT } from '../lib/I18nContext';
import { AGENT_LABELS, agentColor, isAgentRestricted as isAgentRestrictedUtil, hasAgentFullAccess, getProjectGroup, isUsable } from '../lib/constants';
import type { ToastFn } from '../hooks/useToast';
import {
  ChevronRight, Cpu, Loader2,
  MessageSquare, AlertTriangle,
  ShieldCheck, Check, Rocket, Play, Zap,
  Menu,
} from 'lucide-react';
import { useIsMobile } from '../hooks/useMediaQuery';

const isBootstrapDisc = (title: string) => title.startsWith('Bootstrap: ');
const isBriefingDisc = (title: string) => title.startsWith('Briefing');

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
  abortControllers: React.MutableRefObject<Record<string, AbortController>>;
  cleanupStream: (discId: string) => void;
  // Lifted unseen tracking (lives in Dashboard for cross-page visibility)
  markDiscussionSeen: (discId: string, msgCount: number) => void;
  onActiveDiscussionChange: (id: string | null) => void;
  lastSeenMsgCount: Record<string, number>;
  mcpConfigs?: McpConfigDisplay[];
  mcpIncompatibilities?: McpIncompatibility[];
}

// ─── TTS imports ──
import { speakText, stopTts, pauseTts, resumeTts, isTtsPaused } from '../lib/tts-engine';

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
  abortControllers,
  cleanupStream: cleanupStreamBase,
  markDiscussionSeen,
  onActiveDiscussionChange,
  lastSeenMsgCount,
  initialActiveDiscussionId,
  mcpConfigs = [],
  mcpIncompatibilities = [],
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
  const [editingMsgId, setEditingMsgId] = useState<string | null>(null);
  const [editingText, setEditingText] = useState('');
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
  // Batch run summaries — feeds the sidebar pastille that links a batch group
  // back to the workflow run that spawned it. Refetched on batch WS events
  // (see handleWsMessage below) so newly-finished batches pick up their
  // parent_run_sequence label without a full page reload.
  const [batchSummaries, setBatchSummaries] = useState<import('../types/generated').BatchRunSummary[]>([]);
  const refetchBatchSummaries = useCallback(() => {
    workflowsApi.listBatchRunSummaries()
      .then(setBatchSummaries)
      .catch((e) => {
        // Log so silent API/network failures stop manifesting as
        // "batch groups have no parent pastille" without any signal.
        console.warn('Failed to load batch run summaries:', e);
      });
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

  // Batched streaming: accumulate chunks in a ref, flush to state via rAF
  const streamBufferRef = useRef<Record<string, string>>({});
  const rafIdRef = useRef<number | null>(null);
  const flushStreamBuffer = useCallback(() => {
    rafIdRef.current = null;
    const buf = streamBufferRef.current;
    if (Object.keys(buf).length === 0) return;
    const snapshot = { ...buf };
    streamBufferRef.current = {};
    setStreamingMap(prev => {
      const next = { ...prev };
      for (const [k, v] of Object.entries(snapshot)) {
        next[k] = (next[k] ?? '') + v;
      }
      return next;
    });
  }, [setStreamingMap]);
  const appendStreamChunk = useCallback((discId: string, text: string) => {
    streamBufferRef.current[discId] = (streamBufferRef.current[discId] ?? '') + text;
    if (rafIdRef.current === null) {
      rafIdRef.current = requestAnimationFrame(flushStreamBuffer);
    }
  }, [flushStreamBuffer]);

  // Cache of fully-loaded discussions (with messages)
  const [loadedDiscussions, setLoadedDiscussions] = useState<Record<string, Discussion>>({});

  // Fetch full discussion (with messages) when active discussion changes
  // or when sending finishes (to pick up the agent's response)
  const activeSending = activeDiscussionId ? !!sendingMap[activeDiscussionId] : false;
  useEffect(() => {
    if (!activeDiscussionId) return;
    let cancelled = false;
    discussionsApi.get(activeDiscussionId).then(disc => {
      if (!cancelled && disc) {
        setLoadedDiscussions(prev => ({ ...prev, [disc.id]: disc }));
      }
    }).catch(() => { /* ignore fetch errors */ });
    return () => { cancelled = true; };
  }, [activeDiscussionId, activeSending]);

  // Clear worktree error when switching discussions
  useEffect(() => { setWorktreeError(null); }, [activeDiscussionId]);

  // ─── Derived data ────────────────────────────────────────────────────────
  const activeDiscussion = (activeDiscussionId && loadedDiscussions[activeDiscussionId])
    ? loadedDiscussions[activeDiscussionId]
    : allDiscussions.find(d => d.id === activeDiscussionId) ?? null;

  const activeAgentDisabled = useMemo(() => {
    if (!activeDiscussion || agents.length === 0) return false;
    const agentDet = agents.find(a => a.agent_type === activeDiscussion.agent);
    return !agentDet || !isUsable(agentDet);
  }, [activeDiscussion, agents]);

  const sending = activeDiscussionId ? !!sendingMap[activeDiscussionId] : false;
  const streamingText = activeDiscussionId ? (streamingMap[activeDiscussionId] ?? '') : '';

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
      const lastAgent = [...newMsgs].reverse().find(m => m.role === 'Agent');
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

  // Fetch available skills, profiles, directives & contacts
  useEffect(() => {
    skillsApi.list().then(setAvailableSkills).catch(() => {});
    profilesApi.list().then(setAvailableProfiles).catch(() => {});
    directivesApi.list().then(setAvailableDirectives).catch(() => {});
    contactsApi.list().then(setContactsList).catch(() => {});
    refetchBatchSummaries();
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
    // Remote peer shared a discussion with us → refresh list
    if (msg.type === 'discussion_invite') {
      refetchDiscussions();
      toast(`${msg.from_pseudo} shared "${msg.title}"`, 'info');
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
      reloadDiscussion(msg.discussion_id);
      const name = msg.batch_name ?? 'Batch';
      if (msg.batch_failed === 0) {
        toast(t('qp.batch.toast.ok', name, msg.batch_completed), 'success');
      } else {
        // No 'warning' variant in useToast — use 'info' so the toast still
        // shows distinctively without crashing the type check.
        toast(t('qp.batch.toast.partial', name, msg.batch_completed, msg.batch_failed), 'info');
      }
    }
    // Batch progress tick — clear the spinner for the disc that just finished
    // and refresh the list so the pill ticks live.
    if (msg.type === 'batch_run_progress') {
      refetchDiscussions();
      setSendingMap(prev => ({ ...prev, [msg.discussion_id]: false }));
      reloadDiscussion(msg.discussion_id);
    }
    // Backend boot recovered in-flight agent partials — refresh the affected
    // discs + tell the user so they don't resend on top of the recovered run.
    if (msg.type === 'partial_response_recovered') {
      refetchDiscussions();
      for (const id of msg.discussion_ids) {
        reloadDiscussion(id);
        // Drop any stale "sending" indicator left over from before the restart.
        setSendingMap(prev => ({ ...prev, [id]: false }));
      }
      toast(t('disc.partialRecoveredToast', msg.discussion_ids.length), 'info');
    }
  // NOTE: reloadDiscussion is defined later in the component and referenced
  // here only inside the callback body (closure). Do NOT add it to the dep
  // array — it would be in the temporal dead zone at this point in render
  // and throw a ReferenceError.
  }, [contactsList, activeDiscussionId, refetchDiscussions, setSendingMap, toast, t]);
  const { connected: wsConnected } = useWebSocket(handleWsMessage);


  // Mark active discussion as seen + sync activeDiscussionId to parent
  useEffect(() => {
    onActiveDiscussionChange(activeDiscussionId);
  }, [activeDiscussionId, onActiveDiscussionChange]);

  useEffect(() => {
    if (activeDiscussionId && activeDiscussion) {
      markDiscussionSeen(activeDiscussionId, activeDiscussion.messages.length);
    }
  }, [activeDiscussionId, activeDiscussion?.messages.length, markDiscussionSeen]);

  // Timer for agent activity duration — uses lifted startMap to survive page switches
  useEffect(() => {
    if (sending && activeDiscussionId) {
      // Record start time if not already set
      if (!sendingStartMap[activeDiscussionId]) {
        setSendingStartMap(prev => ({ ...prev, [activeDiscussionId!]: Date.now() }));
      }
      // Update elapsed every second from the persistent start time
      const tick = () => {
        const start = sendingStartMap[activeDiscussionId!] || Date.now();
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
  useEffect(() => {
    setStickToBottom(true);
    setHasNewWhileScrolledUp(false);
    // Defer to next frame so the new messages have rendered.
    requestAnimationFrame(() => {
      chatEndRef.current?.scrollIntoView({ behavior: 'instant' as ScrollBehavior });
    });
  }, [activeDiscussionId]);

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
    cleanupStreamBase(discId);
    refetchDiscussions();
    refetchProjects(); // Refresh project audit_status for CTA updates
    reloadDiscussion(discId);
  }, [cleanupStreamBase, refetchDiscussions, refetchProjects, reloadDiscussion]);

  // Called by ChatHeader after any inline API update (title, skills, profiles, etc.)
  const handleDiscussionUpdated = useCallback(() => {
    refetchDiscussions();
    if (activeDiscussionId) reloadDiscussion(activeDiscussionId);
  }, [refetchDiscussions, activeDiscussionId, reloadDiscussion]);

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
      (error) => { console.error('Agent error:', error); const e = String(error); if (e.includes('checked out') || e.includes('worktree')) { setWorktreeError(e); } else { toast(e, 'error'); } cleanupStream(discId); },
      controller.signal,
      onAgentLog,
    );
  }, [activeDiscussionId, reloadDiscussion, refetchDiscussions, abortControllers, setSendingMap, setSendingStartMap, setStreamingMap, resetAgentLogs, appendStreamChunk, cleanupStream, toast, onAgentLog]);

  // Refetch projects when viewing a briefing/validation discussion to get fresh audit_status
  useEffect(() => {
    if (!activeDiscussionId) return;
    const disc = allDiscussions.find(d => d.id === activeDiscussionId);
    if (disc && (isBriefingDisc(disc.title) || disc.title === 'Validation audit AI')) {
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
        (error) => { console.error('Agent error:', error); const e = String(error); if (e.includes('checked out') || e.includes('worktree')) { setWorktreeError(e); } else { toast(e, 'error'); } cleanupStream(discId); },
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
    }

    const discId = disc.id;
    const controller = new AbortController();
    abortControllers.current[discId] = controller;
    setSendingMap(prev => ({ ...prev, [discId]: true }));
    setStreamingMap(prev => ({ ...prev, [discId]: '' }));
    resetAgentLogs();
    await discussionsApi.runAgent(
      discId,
      (text) => appendStreamChunk(discId, text),
      () => cleanupStream(discId),
      (error) => { console.error('Agent error:', error); const e = String(error); if (e.includes('checked out') || e.includes('worktree')) { setWorktreeError(e); } else { toast(e, 'error'); } cleanupStream(discId); },
      controller.signal,
        onAgentLog,
    );
  };

  const handleSendMessage = async (msg: string, targetAgent?: AgentType) => {
    if (!activeDiscussionId || !msg.trim() || sending) return;
    stopTts();
    const discId = activeDiscussionId;

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
        },
      };
    });

    const controller = new AbortController();
    abortControllers.current[discId] = controller;
    setStreamingMap(prev => ({ ...prev, [discId]: '' }));

    resetAgentLogs();
    await discussionsApi.sendMessageStream(
      discId,
      { content: msg, target_agent: targetAgent },
      (text) => appendStreamChunk(discId, text),
      () => cleanupStream(discId),
      (error) => {
        console.error('Agent error:', error);
        const errStr = String(error);
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
        markDiscussionSeen(discId, (activeDiscussion?.messages.length ?? 0) + 1);
      },
      onAgentLog,
    );
  };

  const handleStop = () => {
    if (!activeDiscussionId) return;
    const discId = activeDiscussionId;
    const controller = abortControllers.current[discId];
    if (controller) controller.abort();
    cleanupStream(discId);
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
        const errStr = String(error);
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
    if (!confirm('Supprimer cette discussion et tous ses messages ?')) return;
    await discussionsApi.delete(discId);
    setActiveDiscussionId(prev => prev === discId ? null : prev);
    refetchDiscussions();
  }, [refetchDiscussions]);
  const handleDiscUnarchive = useCallback(async (discId: string) => {
    await discussionsApi.update(discId, { archived: false });
    refetchDiscussions();
  }, [refetchDiscussions]);

  const handleToggleGroup = useCallback((key: string) => {
    setCollapsedDiscGroups(prev => {
      const n = new Set(prev);
      prev.has(key) ? n.delete(key) : n.add(key);
      return n;
    });
  }, []);

  const handleContactAdd = useCallback(async (code: string) => {
    const result = await contactsApi.add(code);
    setContactsList(prev => [...prev, result.contact]);
    if (result.warning) {
      const warningKey = `contacts.warn.${result.warning}`;
      const msg = t(warningKey);
      toast(msg !== warningKey ? msg : t('contacts.warnGeneric'), 'info');
    } else {
      toast(t('contacts.added'), 'success');
    }
  }, [toast, t]);

  const handleContactDelete = useCallback(async (id: string) => {
    await contactsApi.delete(id);
    setContactsList(prev => prev.filter(x => x.id !== id));
    toast(t('contacts.deleted'), 'success');
  }, [toast, t]);

  const handleEditMessage = async () => {
    if (!activeDiscussionId || !editingMsgId || !editingText.trim() || sending) return;
    const discId = activeDiscussionId;
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
      (error) => { console.error('Agent error:', error); const e = String(error); if (e.includes('checked out') || e.includes('worktree')) { setWorktreeError(e); } else { toast(e, 'error'); } cleanupStream(discId); },
      controller.signal,
        onAgentLog,
    );
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
          contacts={contactsList}
          contactsOnline={contactsOnline}
          wsConnected={wsConnected}
          isMobile={isMobile}
          onSelect={handleDiscSelect}
          onArchive={handleDiscArchive}
          onUnarchive={handleDiscUnarchive}
          onDelete={handleDiscDelete}
          onNewDiscussion={() => setShowNewDiscussion(true)}
          onClose={() => setSidebarOpen(false)}
          onContactAdd={handleContactAdd}
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
          collapsedGroups={collapsedDiscGroups}
          onToggleGroup={handleToggleGroup}
          onCollapse={() => setSidebarCollapsed(true)}
        />
      ) : null}

      {/* Main area */}
      <div className="disc-chat-area">
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
                // Hide the initial system prompt for automated discussions (briefing, validation, bootstrap)
                const isAutoPrompt = (idx: number) => idx === 0 && msgs[0]?.role === 'User' && (
                  activeDiscussion.title.startsWith('Briefing') ||
                  activeDiscussion.title.startsWith('Validation audit') ||
                  activeDiscussion.title.startsWith('Bootstrap:')
                );
                return msgs.map((msg, idx) => {
                if (isAutoPrompt(idx)) return null;
                return (
                  <MessageBubble
                    key={msg.id}
                    msg={msg}
                    idx={idx}
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
                    t={t}
                  />
                );
              });
              })()}

              {/* Streaming: single agent mode */}
              {sending && !orchState[activeDiscussion.id]?.active && (
                <div className="disc-msg-row" data-role="agent" aria-live="polite">
                  <div className="disc-msg-bubble" data-role="agent">
                    <div className="disc-msg-agent-label" style={{ color: agentColor(activeDiscussion.agent), justifyContent: 'space-between' }}>
                      <span style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
                        <Cpu size={10} /> {activeDiscussion.agent}
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
                        <MarkdownContent content={streamingText} />
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
              )}

              {/* Streaming: orchestration mode */}
              {orchState[activeDiscussion.id] && (() => {
                const orch = orchState[activeDiscussion.id];
                return (
                  <>
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
                          ) : null}
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
                      <button className="disc-cta-btn" data-variant="warning" onClick={() => { onNavigate('projects', { projectId: activeDiscussion.project_id! }); }}>
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
                    <button className="disc-cta-btn" data-variant="info" onClick={() => { onNavigate('projects', { projectId: activeDiscussion.project_id! }); }}>
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

              {/* Workflow ready banner */}
              {(() => {
                const agentMsgs = activeDiscussion.messages.filter((m, idx) => m.role === 'Agent' && idx > 0);
                const wfStreamedText = streamingMap[activeDiscussion.id] ?? '';
                const readyMsg = [...agentMsgs].reverse().find(m => m.content.toUpperCase().includes('KRONN:WORKFLOW_READY'))
                  || (wfStreamedText.toUpperCase().includes('KRONN:WORKFLOW_READY') ? { content: wfStreamedText } : null);
                if (!readyMsg) return null;
                const jsonMatch = readyMsg.content.match(/```json\s*\n([\s\S]*?)\n```/);
                if (!jsonMatch) return null;
                let parsedPayload: unknown = null;
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

            {/* Disabled agent banner */}
            {activeAgentDisabled && activeDiscussion && (
              <div className="disc-agent-disabled-banner">
                <AlertTriangle size={12} style={{ color: '#ffc800', flexShrink: 0 }} />
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

            {/* Input — unified composer */}
            <ChatInput
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
              contextFiles={contextFilesMap[activeDiscussionId ?? ''] ?? []}
              onUploadFiles={handleUploadFiles}
              onDeleteContextFile={handleDeleteContextFile}
              uploadingFiles={uploadingFiles}
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
                terminalEnabled={agentAccess ? Object.values(agentAccess).some((v: any) => v?.full_access) : false}
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

