import { useState, useRef, useEffect, useCallback, useMemo, memo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { discussions as discussionsApi, projects as projectsApi, skills as skillsApi, profiles as profilesApi, directives as directivesApi } from '../lib/api';
import { GitPanel } from '../components/GitPanel';
import type { Project, AgentDetection, Discussion, DiscussionMessage, AgentType, AgentsConfig, Skill, AgentProfile, Directive, McpConfigDisplay, McpIncompatibility } from '../types/generated';
import { useT } from '../lib/I18nContext';
import { AGENT_LABELS, agentColor, isAgentRestricted as isAgentRestrictedUtil, hasAgentFullAccess, getProjectGroup, isHiddenPath, isUsable, isValidationDisc } from '../lib/constants';
import type { ToastFn } from '../hooks/useToast';
import {
  Folder, ChevronRight, Cpu, GitBranch, Server,
  Plus, Trash2, Loader2,
  MessageSquare, Send, X, Key, AlertTriangle, Users,
  StopCircle, RotateCcw, Pencil, ShieldCheck, Check, Archive, Zap, UserCircle, FileText, Settings, Rocket, Play, Pause,
  Volume2, VolumeX, Mic, MicOff, Phone, PhoneOff, Menu, Lock, Unlock, Copy, Clock, RefreshCw,
} from 'lucide-react';
import { useIsMobile } from '../hooks/useMediaQuery';

const isBootstrapDisc = (title: string) => title.startsWith('Bootstrap: ');
const isBriefingDisc = (title: string) => title.startsWith('Briefing');

// Hoisted regexes (avoid creating new RegExp objects per message per render)
const RE_AUTH_ERROR = /api.?key|invalid.*key|key.*not.*config|authenticat|unauthori|login|sign.?in/i;
const RE_PARTIAL_RESPONSE = /Réponse partielle.*interrompu|Timeout d'inactivité/i;

const ALL_AGENT_MENTIONS: { trigger: string; type: AgentType; label: string }[] = [
  { trigger: '@claude', type: 'ClaudeCode', label: 'Claude Code' },
  { trigger: '@codex', type: 'Codex', label: 'Codex' },
  { trigger: '@vibe', type: 'Vibe', label: 'Vibe' },
  { trigger: '@gemini', type: 'GeminiCli', label: 'Gemini CLI' },
  { trigger: '@kiro', type: 'Kiro', label: 'Kiro' },
];

const SWIPE_THRESHOLD = 80;

const SwipeableDiscItem = memo(function SwipeableDiscItem({ disc, isActive, lastSeenCount, isSending, onSelect, onArchive, onDelete, t, archiveLabel }: {
  disc: Discussion;
  isActive: boolean;
  lastSeenCount: number;
  isSending: boolean;
  onSelect: (discId: string, msgCount: number) => void;
  onArchive: (discId: string) => void;
  onDelete: (discId: string) => void;
  t: (key: string, ...args: any[]) => string;
  archiveLabel?: string;
}) {
  const [offsetX, setOffsetX] = useState(0);
  const [swiping, setSwiping] = useState(false);
  const startX = useRef(0);
  const currentX = useRef(0);

  const handlePointerDown = (e: React.PointerEvent) => {
    startX.current = e.clientX;
    currentX.current = e.clientX;
    setSwiping(true);
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  };

  const handlePointerMove = (e: React.PointerEvent) => {
    if (!swiping) return;
    currentX.current = e.clientX;
    const delta = currentX.current - startX.current;
    // Clamp to [-120, 120] with resistance
    const clamped = Math.sign(delta) * Math.min(Math.abs(delta) * 0.7, 120);
    setOffsetX(clamped);
  };

  const handlePointerUp = () => {
    if (!swiping) return;
    setSwiping(false);
    if (offsetX > SWIPE_THRESHOLD) {
      onArchive(disc.id);
    } else if (offsetX < -SWIPE_THRESHOLD) {
      onDelete(disc.id);
    } else if (Math.abs(offsetX) < 5) {
      onSelect(disc.id, disc.message_count ?? disc.messages.length);
    }
    setOffsetX(0);
  };

  const unseen = (disc.message_count ?? disc.messages.length) - lastSeenCount;
  const showBadge = unseen > 0 && !isActive;
  const bgColor = offsetX > 30 ? `rgba(59,130,246,${Math.min(Math.abs(offsetX) / 120, 0.4)})`
                 : offsetX < -30 ? `rgba(239,68,68,${Math.min(Math.abs(offsetX) / 120, 0.4)})`
                 : 'transparent';
  const label = offsetX > 30 ? (archiveLabel ?? t('disc.archive')) : offsetX < -30 ? t('disc.delete') : '';

  return (
    <div style={{ position: 'relative', overflow: 'hidden' }}>
      {/* Background revealed by swipe */}
      <div style={{
        position: 'absolute', inset: 0, display: 'flex', alignItems: 'center',
        justifyContent: offsetX > 0 ? 'flex-start' : 'flex-end',
        padding: '0 16px', background: bgColor, transition: swiping ? 'none' : 'background 0.2s',
      }}>
        {label && (
          <span style={{ fontSize: 11, fontWeight: 700, color: '#fff', textTransform: 'uppercase', letterSpacing: '0.05em' }}>
            {label}
          </span>
        )}
      </div>
      {/* Foreground item */}
      <div
        style={{
          ...ds.discItem(isActive),
          transform: `translateX(${offsetX}px)`,
          transition: swiping ? 'none' : 'transform 0.25s ease-out',
          touchAction: 'pan-y',
          userSelect: 'none',
          position: 'relative',
          zIndex: 1,
          background: isActive ? 'rgba(200,255,0,0.06)' : '#0e1117',
        }}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={() => { setSwiping(false); setOffsetX(0); }}
      >
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 12, fontWeight: 500, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', display: 'flex', alignItems: 'center', gap: 4 }}>
            {isValidationDisc(disc.title) && <ShieldCheck size={10} style={{ color: '#c8ff00', flexShrink: 0 }} />}
            {isBriefingDisc(disc.title) && <Zap size={10} style={{ color: '#60a5fa', flexShrink: 0 }} />}
            {isBootstrapDisc(disc.title) && <Rocket size={10} style={{ color: '#c8ff00', flexShrink: 0 }} />}
            {disc.workspace_mode === 'Isolated' && <GitBranch size={10} style={{ color: '#60a5fa', flexShrink: 0 }} />}
            {disc.title}
            {showBadge && (
              <span style={{
                background: '#c8ff00', color: '#0a0c10', fontSize: 8, fontWeight: 800,
                borderRadius: 6, padding: '1px 5px', minWidth: 14, textAlign: 'center', flexShrink: 0,
              }}>{unseen}</span>
            )}
          </div>
          <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.4)', marginTop: 2, display: 'flex', alignItems: 'center', gap: 4 }}>
            {isSending && <Loader2 size={8} style={{ animation: 'spin 1s linear infinite', color: '#c8ff00' }} />}
            {(disc.participants?.length ?? 0) > 1 && (
              <Users size={8} style={{ color: '#8b5cf6' }} />
            )}
            {disc.message_count ?? disc.messages.length} msg · {disc.agent}
          </div>
        </div>
      </div>
    </div>
  );
});

export interface DiscussionsPageProps {
  projects: Project[];
  agents: AgentDetection[];
  allDiscussions: Discussion[];
  configLanguage: string | null;
  agentAccess: AgentsConfig | null;
  refetchDiscussions: () => void;
  refetchProjects: () => void;
  onNavigate: (page: string, opts?: { projectId?: string; scrollTo?: string }) => void;
  prefill?: { projectId: string; title: string; prompt: string; locked?: boolean } | null;
  initialActiveDiscussionId?: string | null;
  onPrefillConsumed?: () => void;
  /** Auto-open an existing discussion and trigger agent run (used after full audit) */
  autoRunDiscussionId?: string | null;
  onAutoRunConsumed?: () => void;
  /** Open a specific discussion without triggering agent (e.g. Resume Validation) */
  openDiscussionId?: string | null;
  onOpenDiscConsumed?: () => void;
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

// ─── TTS / STT imports ──
import { speakText, stopTts, pauseTts, resumeTts, isTtsPaused } from '../lib/tts-engine';
import { audioBufferToFloat32, transcribeAudio } from '../lib/stt-engine';

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

let sttWorker: Worker | null = null;
function getSttWorker(): Worker {
  if (!sttWorker) {
    sttWorker = new Worker(
      new URL('../lib/stt-worker.ts', import.meta.url),
      { type: 'module' }
    );
  }
  return sttWorker;
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
  const [activeDiscussionId, setActiveDiscussionId] = useState<string | null>(initialActiveDiscussionId ?? null);
  const [showNewDiscussion, setShowNewDiscussion] = useState(false);
  const [newDiscTitle, setNewDiscTitle] = useState('');
  const [newDiscAgent, setNewDiscAgent] = useState<AgentType | ''>('');
  const [newDiscProjectId, setNewDiscProjectId] = useState<string>('');
  const [newDiscPrompt, setNewDiscPrompt] = useState('');
  const [newDiscPrefilled, setNewDiscPrefilled] = useState(false);
  const [showAdvancedOptions, setShowAdvancedOptions] = useState(false);
  const [showGitPanel, setShowGitPanel] = useState(false);
  const [showMcpPopover, setShowMcpPopover] = useState(false);
  const [mcpSearchFilter, setMcpSearchFilter] = useState('');
  const [showProfileEditor, setShowProfileEditor] = useState(false);
  const [chatInput, setChatInput] = useState('');
  const chatInputValueRef = useRef('');
  const chatInputHasText = chatInput.trim().length > 0;
  const updateChatInput = useCallback((val: string) => {
    chatInputValueRef.current = val;
    setChatInput(val);
    if (chatInputRef.current) chatInputRef.current.value = val;
  }, []);
  const [editingMsgId, setEditingMsgId] = useState<string | null>(null);
  const [editingText, setEditingText] = useState('');
  const [collapsedDiscGroups, setCollapsedDiscGroups] = useState<Set<string>>(() => {
    try {
      const saved = localStorage.getItem('kronn:discCollapsedGroups');
      return saved ? new Set(JSON.parse(saved) as string[]) : new Set();
    } catch { return new Set(); }
  });
  const [showDebatePopover, setShowDebatePopover] = useState(false);
  const [debateAgents, setDebateAgents] = useState<AgentType[]>([]);
  const [debateRounds, setDebateRounds] = useState(2);
  const [debateSkillIds, setDebateSkillIds] = useState<string[]>(['token-saver', 'devils-advocate']);
  const [debateDirectiveIds, setDebateDirectiveIds] = useState<string[]>([]);
  const [orchState, setOrchState] = useState<Record<string, {
    active: boolean;
    round: number | string;
    totalRounds: number;
    currentAgent: string | null;
    agentStreams: { agent: string; agentType: string; round: number | string; text: string; done: boolean }[];
    systemMessages: string[];
  }>>({});
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [showArchives, setShowArchives] = useState(false);
  const [editingTitleId, setEditingTitleId] = useState<string | null>(null);
  const [editingTitleText, setEditingTitleText] = useState('');
  const [availableSkills, setAvailableSkills] = useState<Skill[]>([]);
  const [newDiscSkillIds, setNewDiscSkillIds] = useState<string[]>([]);
  const [availableProfiles, setAvailableProfiles] = useState<AgentProfile[]>([]);
  const [newDiscProfileIds, setNewDiscProfileIds] = useState<string[]>([]);
  const [newDiscDirectiveIds, setNewDiscDirectiveIds] = useState<string[]>([]);
  const [availableDirectives, setAvailableDirectives] = useState<Directive[]>([]);
  const [newDiscWorkspaceMode, setNewDiscWorkspaceMode] = useState<'Direct' | 'Isolated'>('Direct');
  const [newDiscTier, setNewDiscTier] = useState<'economy' | 'default' | 'reasoning'>('default');
  const [newDiscBranchName, setNewDiscBranchName] = useState('');
  const [newDiscBaseBranch, setNewDiscBaseBranch] = useState('main');
  const [expandedSummaryMsgId, setExpandedSummaryMsgId] = useState<string | null>(null);
  const [worktreeError, setWorktreeError] = useState<string | null>(null);
  const [showAgentSwitch, setShowAgentSwitch] = useState(false);
  const [copiedMsgId, setCopiedMsgId] = useState<string | null>(null);
  const [ttsEnabled, setTtsEnabled] = useState<boolean>(() => {
    try { return localStorage.getItem('kronn:ttsEnabled') === 'true'; } catch { return false; }
  });
  const [ttsState, setTtsState] = useState<'idle' | 'loading' | 'playing' | 'paused'>('idle');
  const [ttsPlayingMsgId, setTtsPlayingMsgId] = useState<string | null>(null);
  const [sttState, setSttState] = useState<'idle' | 'recording' | 'transcribing'>('idle');
  const [voiceMode, setVoiceMode] = useState(false);
  const [voiceCountdown, setVoiceCountdown] = useState<number | null>(null);
  const voiceCountdownRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const voiceAutoSendRef = useRef(false);
  const handleSendMessageRef = useRef<(() => void) | null>(null);
  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const [sendingElapsed, setSendingElapsed] = useState(0);
  const sendingTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const [agentLogs, setAgentLogs] = useState<string[]>([]);
  const [showLogs, setShowLogs] = useState(false);
  const onAgentLog = useCallback((log: string) => setAgentLogs(prev => [...prev.slice(-50), log]), []);
  const resetAgentLogs = useCallback(() => { setAgentLogs([]); setShowLogs(false); }, []);
  const audioChunksRef = useRef<Blob[]>([]);
  const sttCancelledRef = useRef(false);

  const chatInputRef = useRef<HTMLTextAreaElement>(null);
  const chatEndRef = useRef<HTMLDivElement>(null);

  // Persist sidebar collapse state to localStorage
  useEffect(() => {
    localStorage.setItem('kronn:discCollapsedGroups', JSON.stringify([...collapsedDiscGroups]));
  }, [collapsedDiscGroups]);

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

  // Clear worktree error and close popovers when switching discussions
  useEffect(() => { setWorktreeError(null); setShowAgentSwitch(false); }, [activeDiscussionId]);

  // ─── Derived data ────────────────────────────────────────────────────────
  const activeDiscussion = (activeDiscussionId && loadedDiscussions[activeDiscussionId])
    ? loadedDiscussions[activeDiscussionId]
    : allDiscussions.find(d => d.id === activeDiscussionId) ?? null;

  const activeAgentDisabled = useMemo(() => {
    if (!activeDiscussion || agents.length === 0) return false;
    const agentDet = agents.find(a => a.agent_type === activeDiscussion.agent);
    return !agentDet || !isUsable(agentDet);
  }, [activeDiscussion, agents]);

  const installedAgentsList = useMemo(() => agents.filter(isUsable), [agents]);

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
    if ((ttsEnabled || voiceMode) && msgs.length > prevMsgCountRef.current) {
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

  const AGENT_MENTIONS = useMemo(() => {
    const activeAgentTypes = new Set(installedAgentsList.map(a => a.agent_type));
    return ALL_AGENT_MENTIONS.filter(m => activeAgentTypes.has(m.type));
  }, [installedAgentsList]);

  // Group discussions by project (null = global), separating archived
  const { activeDiscByProject, archivedDiscussions } = useMemo(() => {
    const activeMap = new Map<string | null, Discussion[]>();
    const archived: Discussion[] = [];
    for (const d of allDiscussions) {
      if (d.archived) {
        archived.push(d);
      } else {
        const key = d.project_id ?? null;
        const list = activeMap.get(key) ?? [];
        list.push(d);
        activeMap.set(key, list);
      }
    }
    return { activeDiscByProject: activeMap, archivedDiscussions: archived };
  }, [allDiscussions]);

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

  // Fetch available skills & profiles
  useEffect(() => {
    skillsApi.list().then(setAvailableSkills).catch(() => {});
    profilesApi.list().then(setAvailableProfiles).catch(() => {});
    directivesApi.list().then(setAvailableDirectives).catch(() => {});
  }, []);

  // Auto-select first installed agent if current selection is invalid
  useEffect(() => {
    if (installedAgentsList.length > 0 && !installedAgentsList.some(a => a.agent_type === newDiscAgent)) {
      setNewDiscAgent(installedAgentsList[0].agent_type);
    }
  }, [installedAgentsList.length, newDiscAgent]);

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

  // Auto-scroll on new messages (instant) and streaming (throttled to avoid 60fps layout thrashing)
  const lastScrollRef = useRef(0);
  useEffect(() => {
    chatEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [activeDiscussion?.messages.length]);
  useEffect(() => {
    if (!streamingText) return;
    const now = Date.now();
    if (now - lastScrollRef.current < 250) return;
    lastScrollRef.current = now;
    chatEndRef.current?.scrollIntoView({ behavior: 'instant' as ScrollBehavior });
  }, [streamingText]);

  // Handle prefill from parent (e.g. "validate audit" button on Projects page)
  useEffect(() => {
    if (prefill) {
      setShowNewDiscussion(true);
      // Lock fields only when explicitly requested (validation audit)
      setNewDiscPrefilled(!!prefill.locked);
      setNewDiscProjectId(prefill.projectId);
      setNewDiscTitle(prefill.title);
      setNewDiscPrompt(prefill.prompt);
      // Auto-select mandatory profiles for audit validation
      const validationProfileIds = ['architect', 'tech-lead', 'qa-engineer'];
      setNewDiscProfileIds(validationProfileIds);
      onPrefillConsumed?.();
    }
  }, [prefill, onPrefillConsumed]);

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

  // On mount, ensure the initially active discussion is visible in the sidebar
  useEffect(() => {
    if (initialActiveDiscussionId) {
      ensureDiscussionVisible(initialActiveDiscussionId);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

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
    setActiveDiscussionId(openDiscussionId);
    ensureDiscussionVisible(openDiscussionId);
    onOpenDiscConsumed?.();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openDiscussionId]);

  const handleCreateDiscussion = async () => {
    if (!newDiscPrompt.trim() || !newDiscAgent) return;
    const prompt = newDiscPrompt.trim();
    const title = newDiscTitle.trim() || prompt.slice(0, 60);
    let disc;
    try {
      disc = await discussionsApi.create({
        project_id: newDiscProjectId || null,
        title,
        agent: newDiscAgent as AgentType,
        language: configLanguage ?? 'fr',
        initial_prompt: prompt,
        skill_ids: newDiscSkillIds.length > 0 ? newDiscSkillIds : undefined,
        profile_ids: newDiscProfileIds.length > 0 ? newDiscProfileIds : undefined,
        ...(newDiscDirectiveIds.length > 0 ? { directive_ids: newDiscDirectiveIds } : {}),
        workspace_mode: newDiscWorkspaceMode === 'Isolated' ? 'Isolated' : undefined,
        base_branch: newDiscWorkspaceMode === 'Isolated' ? newDiscBaseBranch : undefined,
        tier: newDiscTier !== 'default' ? newDiscTier : undefined,
      });
    } catch (e) {
      toast(String(e), 'error');
      return;
    }
    setShowNewDiscussion(false);
    setNewDiscTitle('');
    setNewDiscPrompt('');
    setNewDiscPrefilled(false);
    setNewDiscSkillIds([]);
    setNewDiscDirectiveIds([]);
    setNewDiscProfileIds([]);
    setNewDiscWorkspaceMode('Direct');
    setNewDiscTier('default');
    setNewDiscBranchName('');
    setNewDiscBaseBranch('main');
    setActiveDiscussionId(disc.id);
    refetchDiscussions();

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

  const parseMention = (text: string): { targetAgent?: AgentType } => {
    for (const m of AGENT_MENTIONS) {
      if (text.toLowerCase().startsWith(m.trigger + ' ') || text.toLowerCase() === m.trigger) {
        return { targetAgent: m.type };
      }
    }
    return {};
  };

  // Keyboard shortcuts during recording: Enter/Space = stop & send, Escape = cancel
  useEffect(() => {
    if (sttState !== 'recording') return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        e.stopPropagation();
        mediaRecorderRef.current?.stop();
      } else if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        sttCancelledRef.current = true;
        mediaRecorderRef.current?.stop();
        if (voiceMode) { setVoiceMode(false); }
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [sttState]);

  const handleMicToggle = useCallback(async () => {
    if (sttState === 'recording') {
      // Stop recording → triggers ondataavailable → onstop
      mediaRecorderRef.current?.stop();
      return;
    }
    if (sttState === 'transcribing') return; // wait for current transcription

    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const recorder = new MediaRecorder(stream, { mimeType: 'audio/webm;codecs=opus' });
      mediaRecorderRef.current = recorder;
      audioChunksRef.current = [];
      sttCancelledRef.current = false;

      recorder.ondataavailable = (e) => {
        if (e.data.size > 0) audioChunksRef.current.push(e.data);
      };

      recorder.onstop = async () => {
        // Stop mic access
        stream.getTracks().forEach(t => t.stop());

        // If cancelled, discard everything and go idle
        if (sttCancelledRef.current) {
          sttCancelledRef.current = false;
          audioChunksRef.current = [];
          setSttState('idle');
          return;
        }

        setSttState('transcribing');

        try {
          // Decode recorded audio to Float32Array at 16kHz
          const blob = new Blob(audioChunksRef.current, { type: 'audio/webm' });
          const arrayBuf = await blob.arrayBuffer();
          const audioCtx = new AudioContext({ sampleRate: 16000 });
          let decoded;
          try {
            decoded = await audioCtx.decodeAudioData(arrayBuf);
          } finally {
            await audioCtx.close();
          }
          const float32 = audioBufferToFloat32(decoded);

          const lang = activeDiscussion?.language || 'fr';
          const text = await transcribeAudio(getSttWorker(), float32, lang);

          if (text) {
            // In voice mode, set the text and flag auto-send
            if (voiceMode) {
              voiceAutoSendRef.current = true;
            }
            updateChatInput(chatInputValueRef.current ? chatInputValueRef.current + ' ' + text : text);
            setTimeout(() => {
              if (chatInputRef.current) {
                chatInputRef.current.focus();
                chatInputRef.current.style.height = 'auto';
                chatInputRef.current.style.height = Math.min(chatInputRef.current.scrollHeight, 160) + 'px';
              }
            }, 0);
          }
        } catch (err) {
          console.error('STT transcription failed:', err);
        }
        setSttState('idle');
      };

      recorder.start();
      setSttState('recording');
    } catch (err) {
      console.error('Microphone access denied:', err);
      setSttState('idle');
    }
  }, [sttState, activeDiscussion?.language, voiceMode]);

  // Voice mode: auto-send after STT transcription fills chatInput
  useEffect(() => {
    if (voiceAutoSendRef.current && chatInput.trim() && sttState === 'idle' && !sending) {
      voiceAutoSendRef.current = false;
      // Defer to next tick so chatInput state is committed
      setTimeout(() => handleSendMessageRef.current?.(), 0);
    }
  }, [chatInput, sttState, sending]);

  // Voice mode: after TTS finishes reading agent response → start countdown → auto-record
  const prevTtsStateRef = useRef(ttsState);
  useEffect(() => {
    const wasPlaying = prevTtsStateRef.current === 'playing' || prevTtsStateRef.current === 'loading';
    prevTtsStateRef.current = ttsState;

    // Only trigger when ttsState transitions to 'idle' FROM playing/loading
    if (!wasPlaying || ttsState !== 'idle') return;
    if (!voiceMode || sending || sttState !== 'idle') return;
    if (voiceCountdown !== null) return;

    // Start countdown 3→2→1→record
    setVoiceCountdown(3);
    const interval = setInterval(() => {
      setVoiceCountdown(prev => {
        if (prev === null || prev <= 1) {
          clearInterval(interval);
          voiceCountdownRef.current = null;
          return null;
        }
        return prev - 1;
      });
    }, 1000);
    voiceCountdownRef.current = interval;
  }, [voiceMode, ttsState, sending, sttState]);

  // When countdown reaches null (finished) → start recording
  const prevCountdownRef = useRef<number | null>(null);
  useEffect(() => {
    if (prevCountdownRef.current !== null && prevCountdownRef.current > 0 && voiceCountdown === null && voiceMode) {
      handleMicToggle();
    }
    prevCountdownRef.current = voiceCountdown;
  }, [voiceCountdown, voiceMode, handleMicToggle]);

  // Cancel countdown when voice mode is turned off or conversation changes
  useEffect(() => {
    if (!voiceMode) {
      if (voiceCountdownRef.current) { clearInterval(voiceCountdownRef.current); voiceCountdownRef.current = null; }
      setVoiceCountdown(null);
    }
  }, [voiceMode]);

  useEffect(() => {
    if (voiceCountdownRef.current) { clearInterval(voiceCountdownRef.current); voiceCountdownRef.current = null; }
    setVoiceCountdown(null);
    setVoiceMode(false);
  }, [activeDiscussionId]);

  const handleSendMessage = async () => {
    const inputVal = chatInputValueRef.current;
    if (!activeDiscussionId || !inputVal.trim() || sending) return;
    stopTts();
    const discId = activeDiscussionId;
    const msg = inputVal.trim();
    const { targetAgent } = parseMention(msg);
    updateChatInput('');
    setMentionQuery(null);

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

  handleSendMessageRef.current = handleSendMessage;

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

  const handleOrchestrate = async () => {
    if (!activeDiscussionId || debateAgents.length < 2) return;
    const discId = activeDiscussionId;
    const controller = new AbortController();
    abortControllers.current[discId] = controller;
    setShowDebatePopover(false);
    setSendingMap(prev => ({ ...prev, [discId]: true }));
    setOrchState(prev => ({
      ...prev,
      [discId]: { active: true, round: 0, totalRounds: 3, currentAgent: null, agentStreams: [], systemMessages: [] },
    }));

    await discussionsApi.orchestrate(discId, { agents: debateAgents, max_rounds: debateRounds, skill_ids: debateSkillIds, ...(debateDirectiveIds.length > 0 ? { directive_ids: debateDirectiveIds } : {}) }, {
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
        toast(String(error), 'error');
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
    <div style={{ display: 'flex', height: 'calc(100vh - 56px)', margin: '-24px -20px', overflow: 'hidden' }}>
      {/* Sidebar */}
      {(!isMobile || sidebarOpen) && (
      <div style={isMobile ? { position: 'fixed', inset: 0, zIndex: 100, width: '100%', background: '#12151c', display: 'flex', flexDirection: 'column' as const } : ds.sidebar}>
        <div style={ds.sidebarHeader}>
          <span style={{ fontWeight: 600, fontSize: 13 }}>Discussions</span>
          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            <button style={ls.scanBtn} onClick={() => { setShowNewDiscussion(true); setNewDiscPrefilled(false); }}>
              <Plus size={12} /> {t('disc.new')}
            </button>
            {isMobile && (
              <button style={ls.iconBtn} onClick={() => setSidebarOpen(false)} aria-label="Close sidebar"><X size={16} /></button>
            )}
          </div>
        </div>

        {/* Discussion list grouped by project */}
        <div style={ds.sidebarList}>
          {/* Global discussions (no project) */}
          {(() => {
            const globalDiscs = activeDiscByProject.get(null) ?? [];
            if (globalDiscs.length === 0) return null;
            const isCollapsed = collapsedDiscGroups.has('__global__');
            return (
              <div>
                <button
                  style={{ ...ds.projectGroup, borderTop: 'none', cursor: 'pointer', userSelect: 'none' as const, background: 'none', border: 'none', width: '100%', font: 'inherit', color: 'inherit', textAlign: 'left' as const }}
                  onClick={() => setCollapsedDiscGroups(prev => { const n = new Set(prev); isCollapsed ? n.delete('__global__') : n.add('__global__'); return n; })}
                  aria-expanded={!isCollapsed}
                >
                  <ChevronRight size={10} style={{ transform: isCollapsed ? 'none' : 'rotate(90deg)', transition: 'transform 0.15s' }} />
                  <MessageSquare size={10} /> {t('disc.general')}
                  <span style={{ fontWeight: 400, opacity: 0.5, marginLeft: 'auto' }}>{globalDiscs.length}</span>
                </button>
                {!isCollapsed && globalDiscs.sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
                  <SwipeableDiscItem
                    key={disc.id}
                    disc={disc}
                    isActive={disc.id === activeDiscussionId}
                    lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                    isSending={!!sendingMap[disc.id]}
                    onSelect={handleDiscSelect}
                    onArchive={handleDiscArchive}
                    onDelete={handleDiscDelete}
                    t={t}
                  />
                ))}
              </div>
            );
          })()}

          {/* Project discussions — grouped by org */}
          {(() => {
            const visibleProjects = projects.filter(p => !isHiddenPath(p.path) && (activeDiscByProject.get(p.id) ?? []).length > 0);
            // Build org groups
            const orgMap = new Map<string, typeof visibleProjects>();
            for (const p of visibleProjects) {
              const org = getProjectGroup(p, t('disc.local'), t('disc.local'));
              const list = orgMap.get(org) ?? [];
              list.push(p);
              orgMap.set(org, list);
            }
            // Sort orgs alphabetically, "Local" last
            const localLabel = t('disc.local');
            const sortedOrgs = [...orgMap.entries()].sort(([a], [b]) => {
              if (a === localLabel) return 1;
              if (b === localLabel) return -1;
              return a.localeCompare(b);
            });

            return sortedOrgs.map(([orgName, orgProjects]) => {
              const orgKey = `org::${orgName}`;
              const isOrgCollapsed = collapsedDiscGroups.has(orgKey);
              const orgDiscCount = orgProjects.reduce((sum, p) => sum + (activeDiscByProject.get(p.id) ?? []).length, 0);
              // Color from org name hash (same as Dashboard)
              const orgColor = orgName === localLabel ? 'rgba(255,255,255,0.3)'
                : `hsl(${[...orgName].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 0)}, 50%, 60%)`;

              return (
                <div key={orgKey}>
                  {sortedOrgs.length > 1 && (
                    <button
                      style={{
                        display: 'flex', alignItems: 'center', gap: 6, padding: '6px 12px', width: '100%',
                        background: 'none', border: 'none', borderTop: '1px solid rgba(255,255,255,0.05)',
                        font: 'inherit', color: orgColor, cursor: 'pointer', fontSize: 10, fontWeight: 600,
                        textTransform: 'uppercase', letterSpacing: '0.05em', userSelect: 'none' as const,
                      }}
                      onClick={() => setCollapsedDiscGroups(prev => { const n = new Set(prev); isOrgCollapsed ? n.delete(orgKey) : n.add(orgKey); return n; })}
                      aria-expanded={!isOrgCollapsed}
                    >
                      <ChevronRight size={9} style={{ transform: isOrgCollapsed ? 'none' : 'rotate(90deg)', transition: 'transform 0.15s' }} />
                      {orgName}
                      <span style={{ fontWeight: 400, opacity: 0.5, marginLeft: 'auto' }}>{orgDiscCount}</span>
                    </button>
                  )}
                  {!isOrgCollapsed && orgProjects.map(proj => {
                    const projDiscs = activeDiscByProject.get(proj.id) ?? [];
                    const isCollapsed = collapsedDiscGroups.has(proj.id);
                    return (
                      <div key={proj.id}>
                        <button
                          style={{ ...ds.projectGroup, cursor: 'pointer', userSelect: 'none' as const, background: 'none', border: 'none', width: '100%', font: 'inherit', color: 'inherit', textAlign: 'left' as const }}
                          onClick={() => setCollapsedDiscGroups(prev => { const n = new Set(prev); isCollapsed ? n.delete(proj.id) : n.add(proj.id); return n; })}
                          aria-expanded={!isCollapsed}
                        >
                          <ChevronRight size={10} style={{ transform: isCollapsed ? 'none' : 'rotate(90deg)', transition: 'transform 0.15s' }} />
                          <Folder size={10} /> {proj.name}
                          <span style={{ fontWeight: 400, opacity: 0.5, marginLeft: 'auto' }}>{projDiscs.length}</span>
                        </button>
                        {!isCollapsed && projDiscs.sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
                          <SwipeableDiscItem
                            key={disc.id}
                            disc={disc}
                            isActive={disc.id === activeDiscussionId}
                            lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                            isSending={!!sendingMap[disc.id]}
                            onSelect={handleDiscSelect}
                            onArchive={handleDiscArchive}
                            onDelete={handleDiscDelete}
                            t={t}
                          />
                        ))}
                      </div>
                    );
                  })}
                </div>
              );
            });
          })()}

          {allDiscussions.length === 0 && !showNewDiscussion && (
            <div style={{ padding: 24, textAlign: 'center', color: 'rgba(255,255,255,0.35)', fontSize: 12, whiteSpace: 'pre-line' }}>
              {t('disc.empty')}
            </div>
          )}

          {/* Archives section */}
          {archivedDiscussions.length > 0 && (
            <div>
              <button
                style={{ ...ds.projectGroup, cursor: 'pointer', userSelect: 'none' as const, color: 'rgba(255,255,255,0.55)', background: 'none', border: 'none', width: '100%', font: 'inherit', textAlign: 'left' as const }}
                onClick={() => setShowArchives(!showArchives)}
                aria-expanded={showArchives}
              >
                <ChevronRight size={10} style={{ transform: showArchives ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s' }} />
                <Archive size={10} /> {t('disc.archived')}
                <span style={{ fontWeight: 400, opacity: 0.5, marginLeft: 'auto' }}>{archivedDiscussions.length}</span>
              </button>
              {showArchives && archivedDiscussions.sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
                <SwipeableDiscItem
                  key={disc.id}
                  disc={disc}
                  isActive={disc.id === activeDiscussionId}
                  lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                  isSending={!!sendingMap[disc.id]}
                  onSelect={handleDiscSelect}
                  onArchive={handleDiscUnarchive}
                  onDelete={handleDiscDelete}
                  archiveLabel={t('disc.unarchive')}
                  t={t}
                />
              ))}
            </div>
          )}
        </div>
      </div>
      )}

      {/* Main area */}
      <div style={ds.chatArea}>
        {/* New discussion form */}
        {showNewDiscussion && (
          <div style={ds.newDiscOverlay}>
            <div
              style={ds.newDiscCard}
              onKeyDown={e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && newDiscPrompt.trim()) handleCreateDiscussion(); }}
            >
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 20 }}>
                <span style={{ fontWeight: 700, fontSize: 15, color: '#e8eaed' }}>{t('disc.newTitle')}</span>
                <button style={ls.iconBtn} onClick={() => { setShowNewDiscussion(false); setNewDiscPrefilled(false); setNewDiscWorkspaceMode('Direct'); setNewDiscBranchName(''); setNewDiscBaseBranch('main'); }} aria-label="Close"><X size={14} /></button>
              </div>

              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12, marginBottom: 12 }}>
                <div>
                  <label style={ds.label}>{t('disc.project')}</label>
                  <select style={{ ...ds.selectStyled, ...(newDiscPrefilled ? { opacity: 0.5, pointerEvents: 'none' as const } : {}) }} value={newDiscProjectId} onChange={e => {
                    const pid = e.target.value;
                    setNewDiscProjectId(pid);
                    const proj = projects.find(p => p.id === pid);
                    if (proj?.default_skill_ids?.length) setNewDiscSkillIds(proj.default_skill_ids);
                    // default_profile_id removed — profiles are selected per-discussion
                    setNewDiscWorkspaceMode('Direct');
                    setNewDiscBranchName('');
                    setNewDiscBaseBranch('main');
                  }} disabled={newDiscPrefilled}>
                    <option value="">{t('disc.noProject')}</option>
                    {projects.filter(p => !isHiddenPath(p.path)).map(p => (
                      <option key={p.id} value={p.id}>{p.name}</option>
                    ))}
                  </select>
                </div>
                <div>
                  <label style={ds.label}>{t('disc.agent')}</label>
                  <select style={ds.selectStyled} value={newDiscAgent} onChange={e => setNewDiscAgent(e.target.value as AgentType)}>
                    {installedAgentsList.map(a => (
                      <option key={a.name} value={a.agent_type}>{a.name}</option>
                    ))}
                    {installedAgentsList.length === 0 && (
                      <option value="" disabled>{t('disc.noAgent')}</option>
                    )}
                  </select>
                </div>
              </div>

              {newDiscAgent && isAgentRestricted(newDiscAgent as AgentType) && (
                <div style={{ marginBottom: 12, padding: '8px 10px', borderRadius: 6, background: 'rgba(255,200,0,0.06)', border: '1px solid rgba(255,200,0,0.12)', display: 'flex', alignItems: 'center', gap: 6 }}>
                  <AlertTriangle size={11} style={{ color: '#ffb400', flexShrink: 0 }} />
                  <span style={{ fontSize: 10, color: 'rgba(255,200,0,0.7)', lineHeight: 1.4 }}>
                    {t('config.restrictedAgent', AGENT_LABELS[newDiscAgent] ?? newDiscAgent)}
                    {' — '}
                    <span style={{ cursor: 'pointer', textDecoration: 'underline' }} onClick={() => { setShowNewDiscussion(false); onNavigate('settings'); }}>{t('config.restrictedAgentLink')}</span>
                  </span>
                </div>
              )}

              {/* Workspace mode toggle — only for git projects */}
              {(() => {
                const selectedProj = projects.find(p => p.id === newDiscProjectId);
                if (!newDiscProjectId || !selectedProj?.repo_url) return null;
                return (
                  <div style={{ marginBottom: 12 }}>
                    <label style={ds.label}>{t('disc.workspaceDirect').replace(/.*/, 'Workspace')}</label>
                    <div style={{ display: 'flex', gap: 6 }}>
                      <button
                        type="button"
                        onClick={() => { setNewDiscWorkspaceMode('Direct'); setNewDiscBranchName(''); }}
                        style={{
                          flex: 1, padding: '8px 10px', borderRadius: 8, fontSize: 11, fontFamily: 'inherit',
                          cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6,
                          border: newDiscWorkspaceMode === 'Direct' ? '1px solid rgba(255,255,255,0.2)' : '1px solid rgba(255,255,255,0.06)',
                          background: newDiscWorkspaceMode === 'Direct' ? 'rgba(255,255,255,0.06)' : 'rgba(255,255,255,0.02)',
                          color: newDiscWorkspaceMode === 'Direct' ? '#e8eaed' : 'rgba(255,255,255,0.35)',
                          transition: 'all 0.15s',
                        }}
                      >
                        <Folder size={12} />
                        <div>
                          <div style={{ fontWeight: 600 }}>{t('disc.workspaceDirect')}</div>
                          <div style={{ fontSize: 9, opacity: 0.6, marginTop: 1 }}>{t('disc.workspaceDirectDesc')}</div>
                        </div>
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          setNewDiscWorkspaceMode('Isolated');
                          if (!newDiscBranchName) {
                            const title = newDiscTitle.trim();
                            const slug = title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
                            setNewDiscBranchName(slug || `disc-${Date.now()}`);
                          }
                        }}
                        style={{
                          flex: 1, padding: '8px 10px', borderRadius: 8, fontSize: 11, fontFamily: 'inherit',
                          cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6,
                          border: newDiscWorkspaceMode === 'Isolated' ? '1px solid rgba(96,165,250,0.4)' : '1px solid rgba(255,255,255,0.06)',
                          background: newDiscWorkspaceMode === 'Isolated' ? 'rgba(96,165,250,0.08)' : 'rgba(255,255,255,0.02)',
                          color: newDiscWorkspaceMode === 'Isolated' ? '#60a5fa' : 'rgba(255,255,255,0.35)',
                          transition: 'all 0.15s',
                        }}
                      >
                        <GitBranch size={12} />
                        <div>
                          <div style={{ fontWeight: 600 }}>{t('disc.workspaceIsolated')}</div>
                          <div style={{ fontSize: 9, opacity: 0.6, marginTop: 1 }}>{t('disc.workspaceIsolatedDesc')}</div>
                        </div>
                      </button>
                    </div>
                    {newDiscWorkspaceMode === 'Isolated' && (
                      <div style={{ marginTop: 8, display: 'grid', gridTemplateColumns: '2fr 1fr', gap: 8 }}>
                        <div>
                          <label style={{ ...ds.label, fontSize: 10 }}>{t('disc.branchName')}</label>
                          <input
                            style={ds.inputStyled}
                            value={newDiscBranchName}
                            onChange={e => setNewDiscBranchName(e.target.value)}
                            placeholder="feature/my-branch"
                          />
                        </div>
                        <div>
                          <label style={{ ...ds.label, fontSize: 10 }}>{t('disc.baseBranch')}</label>
                          <input
                            style={ds.inputStyled}
                            value={newDiscBaseBranch}
                            onChange={e => setNewDiscBaseBranch(e.target.value)}
                            placeholder="main"
                          />
                        </div>
                      </div>
                    )}
                  </div>
                );
              })()}

              {/* Advanced options (collapsible) */}
              {(availableSkills.length > 0 || availableProfiles.length > 0 || availableDirectives.length > 0) && (
                <div style={{ marginBottom: 12 }}>
                  <button
                    type="button"
                    onClick={() => setShowAdvancedOptions(prev => !prev)}
                    style={{
                      background: 'none', border: 'none', cursor: 'pointer', padding: '4px 0',
                      color: 'rgba(255,255,255,0.4)', fontSize: 11, fontFamily: 'inherit',
                      display: 'flex', alignItems: 'center', gap: 4,
                    }}
                  >
                    <ChevronRight size={11} style={{ transform: showAdvancedOptions ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s' }} />
                    <Settings size={10} />
                    {t('disc.advancedOptions')}
                    {(newDiscSkillIds.length > 0 || newDiscProfileIds.length > 0 || newDiscDirectiveIds.length > 0 || newDiscTier !== 'default') && (
                      <span style={{ fontSize: 9, color: '#c8ff00', marginLeft: 2 }}>
                        ({newDiscSkillIds.length + newDiscProfileIds.length + newDiscDirectiveIds.length}{newDiscTier !== 'default' ? ` · ${newDiscTier === 'economy' ? '⚡' : '🧠'}` : ''})
                      </span>
                    )}
                  </button>

                  {showAdvancedOptions && (
                    <div style={{ marginTop: 8, padding: '10px 12px', borderRadius: 8, background: 'rgba(255,255,255,0.02)', border: '1px solid rgba(255,255,255,0.06)' }}>

                      {/* Model tier selector */}
                      <div style={{ marginBottom: 10 }}>
                        <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)', fontWeight: 600, marginBottom: 4 }}>{t('disc.modelTier')}</div>
                        <div style={{ display: 'flex', gap: 4 }}>
                          {(['economy', 'default', 'reasoning'] as const).map(tier => (
                            <button key={tier} type="button" onClick={() => setNewDiscTier(tier)} style={{
                              flex: 1, padding: '4px 6px', borderRadius: 6, fontSize: 10, fontFamily: 'inherit',
                              cursor: 'pointer', textAlign: 'center' as const,
                              border: newDiscTier === tier ? '1px solid rgba(255,255,255,0.2)' : '1px solid rgba(255,255,255,0.06)',
                              background: newDiscTier === tier ? 'rgba(255,255,255,0.06)' : 'transparent',
                              color: newDiscTier === tier ? (tier === 'economy' ? '#34d399' : tier === 'reasoning' ? '#f59e0b' : '#e8eaed') : 'rgba(255,255,255,0.35)',
                            }}>
                              {tier === 'economy' ? '⚡' : tier === 'reasoning' ? '🧠' : '⚙️'} {t(`disc.tier.${tier}`)}
                            </button>
                          ))}
                        </div>
                      </div>

                      {/* Skills selector */}
                      {availableSkills.length > 0 && (
                        <div style={{ marginBottom: 10 }}>
                          <label style={{ ...ds.label, marginBottom: 4 }}><Zap size={10} style={{ marginRight: 4 }} />{t('skills.selectSkills')}</label>
                          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                            {availableSkills.map(skill => {
                              const selected = newDiscSkillIds.includes(skill.id);
                              return (
                                <button
                                  key={skill.id}
                                  type="button"
                                  onClick={() => {
                                    setNewDiscSkillIds(prev =>
                                      selected ? prev.filter(id => id !== skill.id) : [...prev, skill.id]
                                    );
                                  }}
                                  style={{
                                    padding: '3px 8px', borderRadius: 10, fontSize: 10, fontFamily: 'inherit',
                                    fontWeight: selected ? 600 : 400, cursor: 'pointer',
                                    border: selected ? '1px solid rgba(200,255,0,0.4)' : '1px solid rgba(255,255,255,0.1)',
                                    background: selected ? 'rgba(200,255,0,0.1)' : 'rgba(255,255,255,0.03)',
                                    color: selected ? '#c8ff00' : 'rgba(255,255,255,0.5)',
                                    display: 'flex', alignItems: 'center', gap: 3,
                                    transition: 'all 0.15s',
                                  }}
                                  title={skill.description || skill.name}
                                >
                                  {selected && <Check size={9} />}
                                  {skill.name}
                                </button>
                              );
                            })}
                          </div>
                        </div>
                      )}

                      {/* Profile selector */}
                      {availableProfiles.length > 0 && (
                        <div style={{ marginBottom: 10 }}>
                          <label style={{ ...ds.label, marginBottom: 4 }}><UserCircle size={10} style={{ marginRight: 4 }} />{t('profiles.select')}</label>
                          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                            <button
                              type="button"
                              onClick={() => setNewDiscProfileIds([])}
                              style={{
                                padding: '3px 8px', borderRadius: 10, fontSize: 10, fontFamily: 'inherit',
                                fontWeight: newDiscProfileIds.length === 0 ? 600 : 400, cursor: 'pointer',
                                border: newDiscProfileIds.length === 0 ? '1px solid rgba(139,92,246,0.4)' : '1px solid rgba(255,255,255,0.1)',
                                background: newDiscProfileIds.length === 0 ? 'rgba(139,92,246,0.1)' : 'rgba(255,255,255,0.03)',
                                color: newDiscProfileIds.length === 0 ? '#a78bfa' : 'rgba(255,255,255,0.5)',
                                transition: 'all 0.15s',
                              }}
                            >
                              {t('profiles.none')}
                            </button>
                            {availableProfiles.map(profile => {
                              const selected = newDiscProfileIds.includes(profile.id);
                              return (
                                <button
                                  key={profile.id}
                                  type="button"
                                  onClick={() => setNewDiscProfileIds(prev => selected ? prev.filter(id => id !== profile.id) : [...prev, profile.id])}
                                  style={{
                                    padding: '3px 8px', borderRadius: 10, fontSize: 10, fontFamily: 'inherit',
                                    fontWeight: selected ? 600 : 400, cursor: 'pointer',
                                    border: selected ? `1px solid ${profile.color || 'rgba(139,92,246,0.4)'}` : '1px solid rgba(255,255,255,0.1)',
                                    background: selected ? `${profile.color}15` : 'rgba(255,255,255,0.03)',
                                    color: selected ? (profile.color || '#a78bfa') : 'rgba(255,255,255,0.5)',
                                    display: 'flex', alignItems: 'center', gap: 3,
                                    transition: 'all 0.15s',
                                  }}
                                  title={profile.role}
                                >
                                  {selected && <Check size={9} />}
                                  {profile.avatar} {profile.persona_name || profile.name}
                                </button>
                              );
                            })}
                          </div>
                        </div>
                      )}

                      {/* Directive selector */}
                      {availableDirectives.length > 0 && (
                        <div>
                          <label style={{ ...ds.label, marginBottom: 4 }}><FileText size={10} style={{ marginRight: 4 }} />{t('directives.title')}</label>
                          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                            {availableDirectives.map(directive => {
                              const selected = newDiscDirectiveIds.includes(directive.id);
                              return (
                                <button
                                  key={directive.id}
                                  type="button"
                                  onClick={() => {
                                    setNewDiscDirectiveIds(prev =>
                                      selected ? prev.filter(id => id !== directive.id) : [...prev, directive.id]
                                    );
                                  }}
                                  style={{
                                    padding: '3px 8px', borderRadius: 10, fontSize: 10, fontFamily: 'inherit',
                                    fontWeight: selected ? 600 : 400, cursor: 'pointer',
                                    border: selected ? '1px solid rgba(245,158,11,0.4)' : '1px solid rgba(255,255,255,0.1)',
                                    background: selected ? 'rgba(245,158,11,0.1)' : 'rgba(255,255,255,0.03)',
                                    color: selected ? '#fbbf24' : 'rgba(255,255,255,0.5)',
                                    display: 'flex', alignItems: 'center', gap: 3,
                                    transition: 'all 0.15s',
                                  }}
                                  title={directive.description || directive.name}
                                >
                                  {selected && <Check size={9} />}
                                  {directive.icon} {directive.name}
                                </button>
                              );
                            })}
                          </div>
                        </div>
                      )}
                    </div>
                  )}
                </div>
              )}

              <label style={ds.label}>{t('disc.title')}</label>
              <input
                style={{ ...ds.inputStyled, ...(newDiscPrefilled ? { opacity: 0.5, cursor: 'not-allowed' } : {}) }}
                placeholder={t('disc.titlePlaceholder')}
                value={newDiscTitle}
                onChange={e => {
                  if (newDiscPrefilled) return;
                  const val = e.target.value;
                  setNewDiscTitle(val);
                  if (newDiscWorkspaceMode === 'Isolated') {
                    const slug = val.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
                    setNewDiscBranchName(slug || `disc-${Date.now()}`);
                  }
                }}
                readOnly={newDiscPrefilled}
              />

              <label style={{ ...ds.label, marginTop: 12 }}>{t('disc.prompt')}</label>
              <textarea
                style={{ ...ds.textareaStyled, ...(newDiscPrefilled ? { opacity: 0.5, cursor: 'not-allowed' } : {}) }}
                placeholder={t('disc.promptPlaceholder')}
                value={newDiscPrompt}
                onChange={e => !newDiscPrefilled && setNewDiscPrompt(e.target.value)}
                readOnly={newDiscPrefilled}
                rows={4}
                autoFocus={!newDiscPrefilled}
              />

              {/* Warnings for validation discussion */}
              {newDiscPrefilled && (
                <div style={{ marginTop: 12, padding: '10px 12px', borderRadius: 8, background: 'rgba(255,200,0,0.06)', border: '1px solid rgba(255,200,0,0.12)' }}>
                  <p style={{ fontSize: 11, color: 'rgba(255,200,0,0.7)', margin: '0 0 6px', display: 'flex', alignItems: 'center', gap: 4 }}>
                    <AlertTriangle size={11} /> {t('disc.auditWarn')}
                  </p>
                  <p style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', margin: 0 }}>
                    {t('disc.auditHint')}
                  </p>
                </div>
              )}

              <button
                style={{
                  marginTop: 16, width: '100%', padding: '11px 16px', borderRadius: 8,
                  border: 'none', background: newDiscPrompt.trim() ? '#c8ff00' : 'rgba(255,255,255,0.06)',
                  color: newDiscPrompt.trim() ? '#0a0c10' : 'rgba(255,255,255,0.25)',
                  fontWeight: 700, fontSize: 13, fontFamily: 'inherit', cursor: 'pointer',
                  display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 8,
                  transition: 'all 0.15s',
                }}
                onClick={handleCreateDiscussion}
                disabled={!newDiscPrompt.trim() || !newDiscAgent}
              >
                <MessageSquare size={14} /> {t('disc.start')}
                <span style={{ fontSize: 10, opacity: 0.6, marginLeft: 4 }}>Ctrl+Enter</span>
              </button>
            </div>
          </div>
        )}

        {/* Active discussion chat */}
        {activeDiscussion && !showNewDiscussion ? (
          <>
            {/* Chat header */}
            <div style={ds.chatHeader}>
              {isMobile && (
                <button
                  style={{ background: 'rgba(200,255,0,0.08)', border: '1px solid rgba(200,255,0,0.2)', borderRadius: 6, color: '#c8ff00', cursor: 'pointer', padding: 6, display: 'flex', alignItems: 'center', flexShrink: 0 }}
                  onClick={() => setSidebarOpen(true)}
                  aria-label="Open sidebar"
                >
                  <Menu size={18} />
                </button>
              )}
              <div style={{ flex: 1 }}>
                <div style={{ fontWeight: 600, fontSize: 14, display: 'flex', alignItems: 'center', gap: 6 }}>
                  {isValidationDisc(activeDiscussion.title) && <ShieldCheck size={14} style={{ color: '#c8ff00', flexShrink: 0 }} />}
                  {isBriefingDisc(activeDiscussion.title) && <Zap size={14} style={{ color: '#60a5fa', flexShrink: 0 }} />}
                  {isBootstrapDisc(activeDiscussion.title) && <Rocket size={14} style={{ color: '#c8ff00', flexShrink: 0 }} />}
                  {editingTitleId === activeDiscussion.id && !isValidationDisc(activeDiscussion.title) && !isBootstrapDisc(activeDiscussion.title) && !isBriefingDisc(activeDiscussion.title) ? (
                    <input
                      autoFocus
                      style={{
                        background: 'rgba(255,255,255,0.06)', border: '1px solid rgba(200,255,0,0.3)',
                        borderRadius: 4, padding: '2px 6px', color: '#e8eaed', fontSize: 14,
                        fontWeight: 600, fontFamily: 'inherit', width: 260,
                      }}
                      value={editingTitleText}
                      onChange={e => setEditingTitleText(e.target.value)}
                      onKeyDown={async e => {
                        if (e.key === 'Enter' && editingTitleText.trim()) {
                          const newTitle = editingTitleText.trim();
                          await discussionsApi.update(activeDiscussion.id, { title: newTitle });
                          setEditingTitleId(null);
                          setLoadedDiscussions(prev => {
                            const d = prev[activeDiscussion.id];
                            if (!d) return prev;
                            return { ...prev, [activeDiscussion.id]: { ...d, title: newTitle } };
                          });
                          refetchDiscussions();
                        }
                        if (e.key === 'Escape') setEditingTitleId(null);
                      }}
                      onBlur={async () => {
                        if (editingTitleText.trim() && editingTitleText.trim() !== activeDiscussion.title) {
                          const newTitle = editingTitleText.trim();
                          await discussionsApi.update(activeDiscussion.id, { title: newTitle });
                          setLoadedDiscussions(prev => {
                            const d = prev[activeDiscussion.id];
                            if (!d) return prev;
                            return { ...prev, [activeDiscussion.id]: { ...d, title: newTitle } };
                          });
                          refetchDiscussions();
                        }
                        setEditingTitleId(null);
                      }}
                    />
                  ) : (
                    <span
                      style={{ cursor: (isValidationDisc(activeDiscussion.title) || isBootstrapDisc(activeDiscussion.title) || isBriefingDisc(activeDiscussion.title)) ? 'default' : 'pointer' }}
                      onDoubleClick={() => {
                        if (isValidationDisc(activeDiscussion.title) || isBootstrapDisc(activeDiscussion.title) || isBriefingDisc(activeDiscussion.title)) return;
                        setEditingTitleId(activeDiscussion.id);
                        setEditingTitleText(activeDiscussion.title);
                      }}
                      title={(isValidationDisc(activeDiscussion.title) || isBootstrapDisc(activeDiscussion.title) || isBriefingDisc(activeDiscussion.title)) ? undefined : t('disc.editTitle')}
                    >
                      {activeDiscussion.title}
                    </span>
                  )}
                  {!isValidationDisc(activeDiscussion.title) && !isBootstrapDisc(activeDiscussion.title) && !isBriefingDisc(activeDiscussion.title) && (
                  <button
                    style={{ ...ls.iconBtn, padding: '2px 4px', border: 'none', background: 'none', color: 'rgba(255,255,255,0.2)' }}
                    onClick={() => {
                      if (editingTitleId === activeDiscussion.id) {
                        setEditingTitleId(null);
                      } else {
                        setEditingTitleId(activeDiscussion.id);
                        setEditingTitleText(activeDiscussion.title);
                      }
                    }}
                    title={t('disc.editTitle')}
                    aria-label={t('disc.editTitle')}
                  >
                    <Pencil size={10} />
                  </button>
                  )}
                </div>
                <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.6)', marginTop: 2, display: 'flex', alignItems: 'center', gap: 4, flexWrap: 'wrap' }}>
                  <span>{activeDiscussion.project_id ? (projects.find(p => p.id === activeDiscussion.project_id)?.name ?? '?') : t('disc.general')} · </span>
                  <span style={{ position: 'relative', display: 'inline-flex', alignItems: 'center', gap: 3 }}>
                    <button
                      onClick={() => setShowAgentSwitch(prev => !prev)}
                      disabled={sending}
                      title={t('disc.switchAgent')}
                      style={{
                        background: 'none', border: 'none', padding: '1px 4px', cursor: sending ? 'default' : 'pointer',
                        color: agentColor(activeDiscussion.agent), fontFamily: 'inherit', fontSize: 11, fontWeight: 600,
                        display: 'inline-flex', alignItems: 'center', gap: 3, opacity: sending ? 0.5 : 1,
                      }}
                    >
                      {activeDiscussion.agent} <RefreshCw size={8} style={{ opacity: 0.5 }} />
                    </button>
                    {showAgentSwitch && (
                      <div style={{
                        position: 'absolute', top: '100%', left: 0, marginTop: 4, zIndex: 20,
                        background: '#1a1d26', border: '1px solid rgba(200,255,0,0.2)',
                        borderRadius: 8, overflow: 'hidden', boxShadow: '0 4px 16px rgba(0,0,0,0.4)',
                        minWidth: 160,
                      }}>
                        {installedAgentsList.map(a => (
                          <button
                            key={a.agent_type}
                            disabled={a.agent_type === activeDiscussion.agent}
                            style={{
                              display: 'flex', alignItems: 'center', gap: 8,
                              width: '100%', padding: '8px 12px', border: 'none', cursor: 'pointer',
                              background: a.agent_type === activeDiscussion.agent ? 'rgba(200,255,0,0.08)' : 'transparent',
                              color: a.agent_type === activeDiscussion.agent ? '#c8ff00' : '#e8eaed',
                              fontFamily: 'inherit', fontSize: 11, textAlign: 'left',
                              opacity: a.agent_type === activeDiscussion.agent ? 0.5 : 1,
                            }}
                            onClick={async () => {
                              setShowAgentSwitch(false);
                              try {
                                const discId = activeDiscussion.id;
                                await discussionsApi.update(discId, { agent: a.agent_type });
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
                              } catch (err) {
                                toast(String(err), 'error');
                              }
                            }}
                          >
                            <Cpu size={10} style={{ color: agentColor(a.agent_type) }} />
                            {a.name}
                            {a.agent_type === activeDiscussion.agent && <Check size={10} style={{ marginLeft: 'auto', color: '#c8ff00' }} />}
                          </button>
                        ))}
                      </div>
                    )}
                  </span>
                  {activeDiscussion.workspace_mode === 'Isolated' && activeDiscussion.worktree_branch && (
                    <span style={{ fontSize: 10, padding: '1px 6px', borderRadius: 6, background: activeDiscussion.workspace_path ? 'rgba(96,165,250,0.1)' : 'rgba(250,204,21,0.1)', color: activeDiscussion.workspace_path ? '#60a5fa' : '#facc15', border: `1px solid ${activeDiscussion.workspace_path ? 'rgba(96,165,250,0.2)' : 'rgba(250,204,21,0.2)'}`, display: 'inline-flex', alignItems: 'center', gap: 3 }}>
                      <GitBranch size={8} /> {activeDiscussion.worktree_branch}
                      <span style={{ opacity: 0.5, fontSize: 9 }}>{activeDiscussion.workspace_path ? 'worktree' : t('disc.worktreeUnlocked')}</span>
                      <button
                        title={activeDiscussion.workspace_path ? t('disc.worktreeUnlock') : t('disc.worktreeLock')}
                        onClick={async (e) => {
                          e.stopPropagation();
                          try {
                            if (activeDiscussion.workspace_path) {
                              await discussionsApi.worktreeUnlock(activeDiscussion.id);
                            } else {
                              await discussionsApi.worktreeLock(activeDiscussion.id);
                            }
                            reloadDiscussion(activeDiscussion.id);
                          } catch (err) {
                            toast(String(err), 'error');
                          }
                        }}
                        style={{ background: 'none', border: 'none', padding: 0, cursor: 'pointer', color: 'inherit', display: 'inline-flex', alignItems: 'center', opacity: 0.7 }}
                      >
                        {activeDiscussion.workspace_path ? <Unlock size={9} /> : <Lock size={9} />}
                      </button>
                    </span>
                  )}
                  {(activeDiscussion.profile_ids?.length ?? 0) > 0 && (
                    <>
                      <span style={{ color: 'rgba(255,255,255,0.15)' }}>·</span>
                      {activeDiscussion.profile_ids?.map((pid: string) => {
                        const p = availableProfiles.find(p => p.id === pid);
                        return p ? (
                          <span key={pid} style={{ fontSize: 10, padding: '1px 6px', borderRadius: 8, background: `${p.color}15`, color: p.color, border: `1px solid ${p.color}30` }}>
                            {p.avatar} {p.persona_name || p.name}
                          </span>
                        ) : null;
                      })}
                    </>
                  )}
                  {(activeDiscussion.skill_ids ?? []).length > 0 && (
                    <>
                      <span style={{ color: 'rgba(255,255,255,0.15)' }}>·</span>
                      {(activeDiscussion.skill_ids ?? []).map(sid => {
                        const skill = availableSkills.find(s => s.id === sid);
                        return (
                          <span key={sid} style={{
                            padding: '1px 7px', borderRadius: 8, fontSize: 9, fontWeight: 600,
                            background: 'rgba(200,255,0,0.08)', border: '1px solid rgba(200,255,0,0.2)',
                            color: 'rgba(200,255,0,0.7)',
                          }}>
                            {skill?.name ?? sid}
                          </span>
                        );
                      })}
                    </>
                  )}
                  {(activeDiscussion.directive_ids ?? []).length > 0 && (
                    <>
                      <span style={{ color: 'rgba(255,255,255,0.15)' }}>·</span>
                      {(activeDiscussion.directive_ids ?? []).map(id => {
                        const d = availableDirectives.find(dd => dd.id === id);
                        return (
                          <span key={id} style={{
                            fontSize: 9, padding: '1px 6px', borderRadius: 6,
                            background: 'rgba(245,158,11,0.08)', color: 'rgba(245,158,11,0.6)',
                            border: '1px solid rgba(245,158,11,0.15)',
                            display: 'inline-flex', alignItems: 'center', gap: 2,
                          }}>
                            <FileText size={7} style={{ marginRight: 2 }} />
                            {d ? `${d.icon} ${d.name}` : id}
                          </span>
                        );
                      })}
                    </>
                  )}
                </div>
              </div>
              <div style={{ display: 'flex', gap: 4, alignItems: 'center' }}>
                {/* MCP info button */}
                <div style={{ position: 'relative' }}>
                  <button
                    style={{ ...ls.iconBtn, color: showMcpPopover ? '#00d4ff' : 'rgba(255,255,255,0.4)' }}
                    onClick={() => { setShowMcpPopover(prev => { if (prev) setMcpSearchFilter(''); return !prev; }); setShowProfileEditor(false); }}
                    title={t('disc.mcps')}
                    aria-label={t('disc.mcps')}
                  >
                    <Server size={13} />
                  </button>
                  {showMcpPopover && (() => {
                    const discMcps = activeDiscussion.project_id
                      ? mcpConfigs.filter(c => c.is_global || c.project_ids.includes(activeDiscussion.project_id!))
                      : mcpConfigs.filter(c => c.include_general);
                    // Agents running via direct API (no CLI) cannot use MCP tools
                    const apiOnlyAgents: AgentType[] = ['Vibe' as AgentType];
                    const isApiOnly = apiOnlyAgents.includes(activeDiscussion.agent);
                    const filterLower = mcpSearchFilter.toLowerCase();
                    const filteredMcps = filterLower
                      ? discMcps.filter(c => c.label.toLowerCase().includes(filterLower) || c.server_name.toLowerCase().includes(filterLower))
                      : discMcps;
                    return (
                      <div style={{
                        position: 'absolute', right: 0, top: '100%', marginTop: 4, zIndex: 100,
                        background: '#161b22', border: '1px solid rgba(0,212,255,0.2)', borderRadius: 8,
                        padding: '8px 0', minWidth: 220, boxShadow: '0 8px 24px rgba(0,0,0,0.4)',
                      }}>
                        <div style={{ padding: '4px 12px 6px', fontSize: 10, color: 'rgba(255,255,255,0.35)', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
                          {t('disc.mcps')}
                          <span style={{ fontWeight: 400, textTransform: 'none', letterSpacing: 'normal' }}>{discMcps.length}</span>
                        </div>
                        {discMcps.length > 6 && (
                          <div style={{ padding: '0 8px 6px' }}>
                            <input
                              type="text"
                              value={mcpSearchFilter}
                              onChange={e => setMcpSearchFilter(e.target.value)}
                              placeholder={t('disc.mcpSearch')}
                              style={{
                                width: '100%', padding: '4px 8px', fontSize: 11, background: 'rgba(255,255,255,0.06)',
                                border: '1px solid rgba(255,255,255,0.1)', borderRadius: 4, color: '#e8eaed',
                                outline: 'none', boxSizing: 'border-box',
                              }}
                              autoFocus
                            />
                          </div>
                        )}
                        {isApiOnly && (
                          <div style={{ padding: '3px 12px 6px', fontSize: 10, color: '#f0a020', display: 'flex', alignItems: 'center', gap: 4 }}>
                            <span style={{ fontSize: 10 }}>⚡</span>
                            Mode API — MCPs indisponibles
                          </div>
                        )}
                        <div style={{ maxHeight: 200, overflowY: 'auto' }}>
                          {filteredMcps.length === 0 ? (
                            <div style={{ padding: '4px 12px', fontSize: 11, color: 'rgba(255,255,255,0.4)' }}>{mcpSearchFilter ? t('disc.noMcps') : t('disc.noMcps')}</div>
                          ) : filteredMcps.map(c => {
                            const incomp = mcpIncompatibilities.find(
                              i => i.server_id === c.server_id && i.agent === activeDiscussion.agent
                            );
                            return (
                              <div
                                key={c.id}
                                title={incomp ? `⚠ ${incomp.reason}` : isApiOnly ? 'Non disponible en mode API' : undefined}
                                style={{
                                  padding: '3px 12px', fontSize: 11, display: 'flex', alignItems: 'center', gap: 6,
                                  color: incomp ? '#ff6b6b' : isApiOnly ? 'rgba(255,255,255,0.25)' : '#e8eaed',
                                  opacity: incomp ? 0.7 : isApiOnly ? 0.5 : 1,
                                }}
                              >
                                <Server size={9} style={{ color: incomp ? '#ff6b6b' : isApiOnly ? 'rgba(255,255,255,0.2)' : '#00d4ff', flexShrink: 0 }} />
                                {c.label}
                                {incomp && <span style={{ fontSize: 8, color: '#ff6b6b', fontStyle: 'italic' }}>incompatible</span>}
                                <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.15)', marginLeft: 'auto' }}>{c.server_name}</span>
                              </div>
                            );
                          })}
                        </div>
                      </div>
                    );
                  })()}
                </div>

                {/* Edit profiles/skills button */}
                <div style={{ position: 'relative' }}>
                  <button
                    style={{ ...ls.iconBtn, color: showProfileEditor ? '#a78bfa' : 'rgba(255,255,255,0.4)' }}
                    onClick={() => { setShowProfileEditor(prev => !prev); setShowMcpPopover(false); }}
                    title={t('disc.editConfig')}
                    aria-label={t('disc.editConfig')}
                  >
                    <Settings size={13} />
                  </button>
                  {showProfileEditor && (
                    <div style={{
                      position: 'absolute', right: 0, top: '100%', marginTop: 4, zIndex: 100,
                      background: '#161b22', border: '1px solid rgba(139,92,246,0.2)', borderRadius: 8,
                      padding: 10, minWidth: 240, maxWidth: 320, boxShadow: '0 8px 24px rgba(0,0,0,0.4)',
                    }}>
                      {/* Project */}
                      <div style={{ marginBottom: 8 }}>
                        <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)', fontWeight: 600, marginBottom: 4 }}>{t('disc.project')}</div>
                        <select
                          style={{
                            width: '100%', padding: '4px 6px', borderRadius: 6, fontSize: 11, fontFamily: 'inherit',
                            background: '#1a1d26', border: '1px solid rgba(255,255,255,0.1)', color: '#e8eaed',
                          }}
                          value={activeDiscussion.project_id ?? ''}
                          onChange={async (e) => {
                            const newPid = e.target.value || null;
                            await discussionsApi.update(activeDiscussion.id, { project_id: newPid });
                            refetchDiscussions();
                            setLoadedDiscussions(prev => {
                              const d = prev[activeDiscussion.id];
                              if (!d) return prev;
                              return { ...prev, [activeDiscussion.id]: { ...d, project_id: newPid } };
                            });
                          }}
                        >
                          <option value="">{t('disc.general')}</option>
                          {projects.filter(p => !isHiddenPath(p.path)).map(p => (
                            <option key={p.id} value={p.id}>{p.name}</option>
                          ))}
                        </select>
                      </div>

                      {/* Profiles */}
                      {availableProfiles.length > 0 && (
                        <div style={{ marginBottom: 8 }}>
                          <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)', fontWeight: 600, marginBottom: 4 }}>{t('profiles.select')}</div>
                          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                            {availableProfiles.map(profile => {
                              const active = (activeDiscussion.profile_ids ?? []).includes(profile.id);
                              return (
                                <button key={profile.id} title={profile.role} style={{
                                  padding: '2px 7px', borderRadius: 8, fontSize: 10, fontFamily: 'inherit', cursor: 'pointer',
                                  border: `1px solid ${active ? (profile.color || 'rgba(139,92,246,0.4)') : 'rgba(255,255,255,0.08)'}`,
                                  background: active ? `${profile.color}15` : 'transparent',
                                  color: active ? (profile.color || '#a78bfa') : 'rgba(255,255,255,0.4)',
                                  display: 'flex', alignItems: 'center', gap: 3,
                                }} onClick={async () => {
                                  const current = activeDiscussion.profile_ids ?? [];
                                  const next = active ? current.filter((id: string) => id !== profile.id) : [...current, profile.id];
                                  await discussionsApi.update(activeDiscussion.id, { profile_ids: next });
                                  refetchDiscussions();
                                  // Optimistic update
                                  setLoadedDiscussions(prev => {
                                    const d = prev[activeDiscussion.id];
                                    if (!d) return prev;
                                    return { ...prev, [activeDiscussion.id]: { ...d, profile_ids: next } };
                                  });
                                }}>
                                  {active && <Check size={8} />}
                                  {profile.avatar} {profile.persona_name || profile.name}
                                </button>
                              );
                            })}
                          </div>
                        </div>
                      )}

                      {/* Skills */}
                      {availableSkills.length > 0 && (
                        <div style={{ marginBottom: 8 }}>
                          <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)', fontWeight: 600, marginBottom: 4 }}>{t('skills.selectSkills')}</div>
                          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                            {availableSkills.map(skill => {
                              const active = (activeDiscussion.skill_ids ?? []).includes(skill.id);
                              return (
                                <button key={skill.id} style={{
                                  padding: '2px 7px', borderRadius: 8, fontSize: 10, fontFamily: 'inherit', cursor: 'pointer',
                                  border: `1px solid ${active ? 'rgba(200,255,0,0.3)' : 'rgba(255,255,255,0.08)'}`,
                                  background: active ? 'rgba(200,255,0,0.08)' : 'transparent',
                                  color: active ? '#c8ff00' : 'rgba(255,255,255,0.4)',
                                  display: 'flex', alignItems: 'center', gap: 3,
                                }} onClick={async () => {
                                  const current = activeDiscussion.skill_ids ?? [];
                                  const next = active ? current.filter((id: string) => id !== skill.id) : [...current, skill.id];
                                  await discussionsApi.update(activeDiscussion.id, { skill_ids: next });
                                  refetchDiscussions();
                                  setLoadedDiscussions(prev => {
                                    const d = prev[activeDiscussion.id];
                                    if (!d) return prev;
                                    return { ...prev, [activeDiscussion.id]: { ...d, skill_ids: next } };
                                  });
                                }}>
                                  {active && <Check size={8} />}
                                  {skill.name}
                                </button>
                              );
                            })}
                          </div>
                        </div>
                      )}

                      {/* Model Tier */}
                      <div style={{ marginBottom: 8 }}>
                        <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)', fontWeight: 600, marginBottom: 4 }}>{t('disc.modelTier')}</div>
                        <div style={{ display: 'flex', gap: 4 }}>
                          {(['economy', 'default', 'reasoning'] as const).map(tier => {
                            const active = (activeDiscussion.tier ?? 'default') === tier;
                            return (
                              <button key={tier} style={{
                                padding: '2px 7px', borderRadius: 8, fontSize: 10, fontFamily: 'inherit', cursor: 'pointer',
                                border: `1px solid ${active ? 'rgba(255,255,255,0.2)' : 'rgba(255,255,255,0.08)'}`,
                                background: active ? 'rgba(255,255,255,0.06)' : 'transparent',
                                color: active
                                  ? (tier === 'economy' ? '#34d399' : tier === 'reasoning' ? '#f59e0b' : '#e8eaed')
                                  : 'rgba(255,255,255,0.4)',
                              }} onClick={async () => {
                                await discussionsApi.update(activeDiscussion.id, { tier });
                                refetchDiscussions();
                                setLoadedDiscussions(prev => {
                                  const d = prev[activeDiscussion.id];
                                  if (!d) return prev;
                                  return { ...prev, [activeDiscussion.id]: { ...d, tier } };
                                });
                              }}>
                                {tier === 'economy' ? '⚡' : tier === 'reasoning' ? '🧠' : '⚙️'} {t(`disc.tier.${tier}`)}
                              </button>
                            );
                          })}
                        </div>
                      </div>

                      {/* Directives */}
                      {availableDirectives.length > 0 && (
                        <div>
                          <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)', fontWeight: 600, marginBottom: 4 }}>{t('directives.title')}</div>
                          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                            {availableDirectives.map(directive => {
                              const active = (activeDiscussion.directive_ids ?? []).includes(directive.id);
                              return (
                                <button key={directive.id} style={{
                                  padding: '2px 7px', borderRadius: 8, fontSize: 10, fontFamily: 'inherit', cursor: 'pointer',
                                  border: `1px solid ${active ? 'rgba(245,158,11,0.3)' : 'rgba(255,255,255,0.08)'}`,
                                  background: active ? 'rgba(245,158,11,0.08)' : 'transparent',
                                  color: active ? '#fbbf24' : 'rgba(255,255,255,0.4)',
                                  display: 'flex', alignItems: 'center', gap: 3,
                                }} onClick={async () => {
                                  const current = activeDiscussion.directive_ids ?? [];
                                  const next = active ? current.filter((id: string) => id !== directive.id) : [...current, directive.id];
                                  await discussionsApi.update(activeDiscussion.id, { directive_ids: next });
                                  refetchDiscussions();
                                  setLoadedDiscussions(prev => {
                                    const d = prev[activeDiscussion.id];
                                    if (!d) return prev;
                                    return { ...prev, [activeDiscussion.id]: { ...d, directive_ids: next } };
                                  });
                                }}>
                                  {active && <Check size={8} />}
                                  {directive.icon} {directive.name}
                                </button>
                              );
                            })}
                          </div>
                        </div>
                      )}
                    </div>
                  )}
                </div>

                {activeDiscussion.project_id && (
                  <button
                    style={{ ...ls.iconBtn, color: showGitPanel ? '#c8ff00' : 'rgba(255,255,255,0.4)' }}
                    onClick={() => setShowGitPanel(prev => !prev)}
                    title={t('git.filesBtn')}
                    aria-label={t('git.filesBtn')}
                  >
                    <GitBranch size={13} />
                  </button>
                )}
                <button
                  style={{ ...ls.iconBtn, color: '#ff4d6a' }}
                  onClick={async () => {
                    if (!confirm(t('disc.confirmDelete'))) return;
                    await discussionsApi.delete(activeDiscussion.id);
                    setActiveDiscussionId(null);
                    refetchDiscussions();
                  }}
                  aria-label="Delete discussion"
                >
                  <Trash2 size={12} />
                </button>
              </div>
            </div>

            {/* Messages + Git Panel side by side */}
            <div style={{ display: 'flex', flex: 1, overflow: 'hidden' }}>
            <div style={{ flex: 1, display: 'flex', flexDirection: 'column', overflow: 'hidden' }}>

            {/* Vibe API mode notice */}
            {activeDiscussion.agent === 'Vibe' && (
              <div style={{
                padding: '6px 16px', fontSize: 10, color: 'rgba(240,160,32,0.8)',
                background: 'rgba(240,160,32,0.06)', borderBottom: '1px solid rgba(240,160,32,0.12)',
                display: 'flex', alignItems: 'center', gap: 6,
              }}>
                <span>⚠</span>
                <span>Mode API directe — les outils MCP ne sont pas disponibles. Vibe répond en chat uniquement.</span>
              </div>
            )}

            {/* Kiro output notice */}
            {activeDiscussion.agent === 'Kiro' && (
              <div style={{
                padding: '6px 16px', fontSize: 10, color: 'rgba(123,97,255,0.6)',
                background: 'rgba(123,97,255,0.04)', borderBottom: '1px solid rgba(123,97,255,0.08)',
                display: 'flex', alignItems: 'center', gap: 6,
              }}>
                <span>ℹ</span>
                <span>Kiro CLI: output may include tool logs. <a href="https://github.com/kirodotdev/Kiro/issues/5006" target="_blank" rel="noopener noreferrer" style={{ color: 'rgba(123,97,255,0.8)', textDecoration: 'underline' }}>Tracking issue</a></span>
              </div>
            )}

            {/* Messages */}
            <div style={ds.messages}>
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
                <div style={ds.msgRow(false)} aria-live="polite">
                  <div style={ds.msgBubble(false)}>
                    <div style={{ ...ds.msgAgent, color: agentColor(activeDiscussion.agent), justifyContent: 'space-between' }}>
                      <span style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
                        <Cpu size={10} /> {activeDiscussion.agent}
                        <Loader2 size={10} style={{ animation: 'spin 1s linear infinite', marginLeft: 4 }} />
                      </span>
                      <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.35)', fontVariantNumeric: 'tabular-nums', fontWeight: 400 }}>
                        {sendingElapsed >= 60
                          ? `${Math.floor(sendingElapsed / 60)}m${String(sendingElapsed % 60).padStart(2, '0')}s`
                          : `${sendingElapsed}s`}
                      </span>
                    </div>
                    {streamingText ? (
                      <pre style={{ fontSize: 13, lineHeight: 1.55, whiteSpace: 'pre-wrap', wordBreak: 'break-word', fontFamily: 'inherit', margin: 0, color: '#e8eaed' }}>{streamingText}</pre>
                    ) : (
                      <div style={{ fontSize: 12, color: 'rgba(255,255,255,0.6)', fontStyle: 'italic', display: 'flex', alignItems: 'center', gap: 6 }} aria-live="assertive">
                        <span style={{
                          width: 6, height: 6, borderRadius: '50%', background: '#c8ff00',
                          animation: 'pulse 2s ease-in-out infinite',
                        }} />
                        {t('disc.running')}
                        {agentLogs.length > 0 && (
                          <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)', marginLeft: 4 }}>
                            — {agentLogs[agentLogs.length - 1]?.slice(0, 60)}
                          </span>
                        )}
                      </div>
                    )}
                    {/* Agent logs panel */}
                    {agentLogs.length > 0 && (
                      <div style={{ marginTop: 6 }}>
                        <button
                          onClick={() => setShowLogs(v => !v)}
                          style={{
                            background: 'none', border: 'none', cursor: 'pointer', fontFamily: 'inherit',
                            fontSize: 10, color: 'rgba(255,255,255,0.3)', display: 'flex', alignItems: 'center', gap: 4,
                            padding: 0,
                          }}
                        >
                          <ChevronRight size={10} style={{ transform: showLogs ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s' }} />
                          {t('disc.logs')} ({agentLogs.length})
                        </button>
                        {showLogs && (
                          <div style={{
                            marginTop: 4, padding: '6px 8px', borderRadius: 6,
                            background: 'rgba(0,0,0,0.3)', maxHeight: 150, overflowY: 'auto',
                            fontFamily: 'JetBrains Mono, monospace', fontSize: 10, lineHeight: 1.6,
                            color: 'rgba(255,255,255,0.4)',
                          }}>
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
                      <div key={i} style={ds.msgRow(false)}>
                        <div style={{
                          ...ds.msgBubble(false),
                          borderLeft: `3px solid ${agentColor(as_.agentType || as_.agent)}`,
                        }}>
                          <div style={{
                            display: 'flex', alignItems: 'center', gap: 4, fontSize: 10,
                            fontWeight: 600, color: agentColor(as_.agentType || as_.agent), marginBottom: 4,
                          }}>
                            <Cpu size={10} /> {as_.agent}
                            <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.2)', marginLeft: 4 }}>
                              {as_.round === 'synthesis' ? t('disc.synthesis') : `Round ${as_.round}`}
                            </span>
                            {!as_.done && <Loader2 size={9} style={{ animation: 'spin 1s linear infinite', marginLeft: 4 }} />}
                          </div>
                          {as_.text ? (
                            <MarkdownContent content={as_.text} />
                          ) : !as_.done ? (
                            <div style={{ fontSize: 12, color: 'rgba(255,255,255,0.4)', fontStyle: 'italic' }}>
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
                    <div style={{ margin: '12px 16px', padding: '12px 16px', borderRadius: 10, background: 'rgba(200,255,0,0.06)', border: '1px solid rgba(200,255,0,0.15)' }}>
                      <p style={{ fontSize: 12, color: 'rgba(200,255,0,0.8)', margin: '0 0 8px', display: 'flex', alignItems: 'center', gap: 6 }}>
                        <ShieldCheck size={14} /> {t('audit.auditDoneResume')}
                      </p>
                      <button
                        style={{ padding: '8px 16px', borderRadius: 8, border: 'none', background: '#c8ff00', color: '#0a0c10', fontWeight: 700, fontSize: 12, fontFamily: 'inherit', cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6 }}
                        onClick={() => { setActiveDiscussionId(validationDisc.id); }}
                      >
                        <MessageSquare size={12} /> {t('audit.resumeValidation')}
                      </button>
                    </div>
                  );
                }

                // State 2: Audit done but no validation yet (just finished, validation being created)
                if (auditDone && !validationDisc) {
                  return (
                    <div style={{ margin: '12px 16px', padding: '12px 16px', borderRadius: 10, background: 'rgba(255,180,0,0.06)', border: '1px solid rgba(255,180,0,0.15)' }}>
                      <p style={{ fontSize: 12, color: 'rgba(255,180,0,0.8)', margin: '0 0 8px', display: 'flex', alignItems: 'center', gap: 6 }}>
                        <Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} /> {t('audit.auditInProgress')}
                      </p>
                      <button
                        style={{ padding: '8px 16px', borderRadius: 8, border: 'none', background: 'rgba(255,180,0,0.2)', color: '#f0a020', fontWeight: 700, fontSize: 12, fontFamily: 'inherit', cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6 }}
                        onClick={() => { onNavigate('projects', { projectId: activeDiscussion.project_id! }); }}
                      >
                        <Play size={12} /> {t('audit.goToProject')}
                      </button>
                    </div>
                  );
                }

                // State 1: Briefing done, no audit yet → go launch audit
                return (
                  <div style={{ margin: '12px 16px', padding: '12px 16px', borderRadius: 10, background: 'rgba(96,165,250,0.06)', border: '1px solid rgba(96,165,250,0.15)' }}>
                    <p style={{ fontSize: 12, color: 'rgba(96,165,250,0.8)', margin: '0 0 8px', display: 'flex', alignItems: 'center', gap: 6 }}>
                      <Check size={14} /> {t('audit.briefingDone')}
                    </p>
                    <button
                      style={{ padding: '8px 16px', borderRadius: 8, border: 'none', background: '#60a5fa', color: '#0a0c10', fontWeight: 700, fontSize: 12, fontFamily: 'inherit', cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6 }}
                      onClick={() => { onNavigate('projects', { projectId: activeDiscussion.project_id! }); }}
                    >
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
                  <div style={{ margin: '12px 16px', padding: '12px 16px', borderRadius: 10, background: 'rgba(200,255,0,0.06)', border: '1px solid rgba(200,255,0,0.15)' }}>
                    <p style={{ fontSize: 12, color: 'rgba(200,255,0,0.8)', margin: '0 0 8px', display: 'flex', alignItems: 'center', gap: 6 }}>
                      <ShieldCheck size={14} /> {t('audit.validationComplete')}
                    </p>
                    <button
                      style={{
                        padding: '8px 16px', borderRadius: 8, border: 'none',
                        background: '#c8ff00', color: '#0a0c10', fontWeight: 700,
                        fontSize: 12, fontFamily: 'inherit', cursor: 'pointer',
                        display: 'flex', alignItems: 'center', gap: 6,
                      }}
                      onClick={async () => {
                        await projectsApi.validateAudit(proj.id);
                        refetchProjects();
                        refetchDiscussions();
                      }}
                    >
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
                const isComplete = lastAgentMsg && lastAgentMsg.content.toUpperCase().includes('KRONN:BOOTSTRAP_COMPLETE');
                if (!isComplete) return null;
                return (
                  <div style={{ margin: '12px 16px', padding: '12px 16px', borderRadius: 10, background: 'rgba(200,255,0,0.06)', border: '1px solid rgba(200,255,0,0.15)' }}>
                    <p style={{ fontSize: 12, color: 'rgba(200,255,0,0.8)', margin: '0 0 8px', display: 'flex', alignItems: 'center', gap: 6 }}>
                      <Rocket size={14} /> {t('audit.bootstrapComplete')}
                    </p>
                    <button
                      style={{
                        padding: '8px 16px', borderRadius: 8, border: 'none',
                        background: '#c8ff00', color: '#0a0c10', fontWeight: 700,
                        fontSize: 12, fontFamily: 'inherit', cursor: 'pointer',
                        display: 'flex', alignItems: 'center', gap: 6,
                      }}
                      onClick={async () => {
                        await projectsApi.markBootstrapped(proj.id);
                        refetchProjects();
                        refetchDiscussions();
                      }}
                    >
                      <Check size={12} /> {t('audit.markBootstrapped')}
                    </button>
                  </div>
                );
              })()}

              <div ref={chatEndRef} />
            </div>

            {/* Disabled agent banner */}
            {activeAgentDisabled && activeDiscussion && (
              <div style={{
                display: 'flex', alignItems: 'center', gap: 8, padding: '8px 20px',
                background: 'rgba(255,200,0,0.04)', borderTop: '1px solid rgba(255,200,0,0.1)',
              }}>
                <AlertTriangle size={12} style={{ color: '#ffc800', flexShrink: 0 }} />
                <span style={{ fontSize: 11, color: 'rgba(255,200,0,0.7)', lineHeight: 1.4 }}>
                  {t('disc.agentDisabled', AGENT_LABELS[activeDiscussion.agent] ?? activeDiscussion.agent)}
                  {' — '}
                  <span
                    style={{ cursor: 'pointer', textDecoration: 'underline' }}
                    onClick={() => onNavigate('settings')}
                  >{t('disc.agentDisabledLink')}</span>
                </span>
              </div>
            )}

            {/* Input — unified composer */}
            <div style={{
              padding: '10px 16px 12px', borderTop: '1px solid rgba(255,255,255,0.07)',
              background: '#12151c', flexShrink: 0,
              ...(activeAgentDisabled ? { opacity: 0.4, pointerEvents: 'none' as const } : {}),
            }}>
              {/* Voice mode countdown banner */}
              {voiceCountdown !== null && (
                <div style={{
                  display: 'flex', alignItems: 'center', gap: 10, padding: '8px 14px',
                  marginBottom: 8, borderRadius: 8,
                  background: 'rgba(200,255,0,0.06)', border: '1px solid rgba(200,255,0,0.15)',
                }}>
                  <span style={{
                    fontSize: 20, fontWeight: 800, color: '#c8ff00',
                    fontVariantNumeric: 'tabular-nums', minWidth: 24, textAlign: 'center',
                  }}>{voiceCountdown}</span>
                  <span style={{ fontSize: 12, color: 'rgba(200,255,0,0.6)', flex: 1 }}>{t('disc.resumeListening')}</span>
                  <button
                    onClick={() => {
                      if (voiceCountdownRef.current) { clearInterval(voiceCountdownRef.current); voiceCountdownRef.current = null; }
                      setVoiceCountdown(null);
                      setVoiceMode(false);
                    }}
                    style={{
                      background: 'rgba(255,255,255,0.06)', border: '1px solid rgba(255,255,255,0.1)',
                      borderRadius: 6, padding: '3px 10px', cursor: 'pointer',
                      color: 'rgba(255,255,255,0.5)', fontSize: 11, fontFamily: 'inherit',
                    }}
                  >
                    {t('disc.cancelVoice')}
                  </button>
                </div>
              )}
              {/* Recording indicator banner */}
              {sttState === 'recording' && (
                <div style={{
                  display: 'flex', alignItems: 'center', gap: 8, padding: '6px 12px',
                  marginBottom: 8, borderRadius: 8,
                  background: 'rgba(255,77,106,0.08)', border: '1px solid rgba(255,77,106,0.2)',
                }}>
                  <span style={{
                    width: 8, height: 8, borderRadius: '50%', background: '#ff4d6a',
                    animation: 'pulse 1.5s ease-in-out infinite',
                  }} />
                  <span style={{ fontSize: 12, color: '#ff8a9e', flex: 1 }}>{t('disc.recording')}</span>
                  <button
                    onClick={() => {
                      sttCancelledRef.current = true;
                      mediaRecorderRef.current?.stop();
                      if (voiceMode) { setVoiceMode(false); }
                    }}
                    style={{
                      background: 'rgba(255,255,255,0.06)', border: '1px solid rgba(255,255,255,0.1)',
                      borderRadius: 6, padding: '4px 10px', cursor: 'pointer',
                      color: 'rgba(255,255,255,0.5)', fontSize: 11, fontFamily: 'inherit', fontWeight: 600,
                      display: 'flex', alignItems: 'center', gap: 4,
                    }}
                  >
                    <X size={10} /> {t('disc.cancelVoice')}
                  </button>
                  <button
                    onClick={handleMicToggle}
                    style={{
                      background: 'rgba(255,77,106,0.15)', border: '1px solid rgba(255,77,106,0.3)',
                      borderRadius: 6, padding: '4px 10px', cursor: 'pointer',
                      color: '#ff4d6a', fontSize: 11, fontFamily: 'inherit', fontWeight: 600,
                      display: 'flex', alignItems: 'center', gap: 4,
                    }}
                  >
                    <StopCircle size={10} /> {voiceMode ? t('disc.sendVoice') : t('disc.stopRecording')}
                  </button>
                </div>
              )}
              {sttState === 'transcribing' && (
                <div style={{
                  display: 'flex', alignItems: 'center', gap: 8, padding: '6px 12px',
                  marginBottom: 8, borderRadius: 8,
                  background: 'rgba(200,255,0,0.04)', border: '1px solid rgba(200,255,0,0.1)',
                }}>
                  <Loader2 size={12} style={{ color: '#c8ff00', animation: 'spin 1s linear infinite' }} />
                  <span style={{ fontSize: 12, color: 'rgba(200,255,0,0.7)' }}>{t('disc.transcribing')}</span>
                </div>
              )}

              {/* Composer container */}
              <div style={{
                position: 'relative',
                background: 'rgba(255,255,255,0.03)',
                border: sttState === 'recording'
                  ? '1px solid rgba(255,77,106,0.3)'
                  : '1px solid rgba(255,255,255,0.08)',
                borderRadius: 12,
                transition: 'border-color 0.2s',
              }}>
                {/* @mention autocomplete dropdown */}
                {mentionQuery !== null && (() => {
                  const filtered = AGENT_MENTIONS.filter(m => m.trigger.slice(1).startsWith(mentionQuery ?? ''));
                  if (filtered.length === 0) return null;
                  return (
                    <div style={{
                      position: 'absolute', bottom: '100%', left: 0, marginBottom: 4,
                      background: '#1a1d26', border: '1px solid rgba(200,255,0,0.2)',
                      borderRadius: 8, overflow: 'hidden', boxShadow: '0 4px 16px rgba(0,0,0,0.4)',
                      minWidth: 180, zIndex: 10,
                    }}>
                      {filtered.map((m, i) => (
                        <button
                          key={m.trigger}
                          style={{
                            display: 'flex', alignItems: 'center', gap: 8,
                            width: '100%', padding: '8px 12px', border: 'none', cursor: 'pointer',
                            background: i === mentionIndex ? 'rgba(200,255,0,0.1)' : 'transparent',
                            color: '#e8eaed', fontFamily: 'inherit', fontSize: 12, textAlign: 'left',
                          }}
                          onMouseDown={e => {
                            e.preventDefault();
                            updateChatInput(m.trigger + ' ');
                            setMentionQuery(null);
                            chatInputRef.current?.focus();
                          }}
                          onMouseEnter={() => setMentionIndex(i)}
                        >
                          <Cpu size={12} style={{ color: '#c8ff00' }} />
                          <span style={{ fontWeight: 600, color: '#c8ff00' }}>{m.trigger}</span>
                          <span style={{ color: 'rgba(255,255,255,0.4)' }}>{m.label}</span>
                        </button>
                      ))}
                    </div>
                  );
                })()}

                {/* Worktree error banner */}
                {worktreeError && (
                  <div style={{
                    display: 'flex', alignItems: 'center', gap: 8,
                    padding: '8px 12px', margin: '4px 8px 0',
                    background: 'rgba(239,68,68,0.1)', border: '1px solid rgba(239,68,68,0.3)',
                    borderRadius: 8, fontSize: 11, color: '#fca5a5',
                  }}>
                    <AlertTriangle size={14} style={{ color: '#ef4444', flexShrink: 0 }} />
                    <span style={{ flex: 1 }}>{worktreeError}</span>
                    <button
                      onClick={async () => {
                        if (!activeDiscussionId) return;
                        try {
                          await discussionsApi.worktreeLock(activeDiscussionId);
                          setWorktreeError(null);
                          reloadDiscussion(activeDiscussionId);
                          toast(t('disc.worktreeLock') + ' ✓', 'success');
                        } catch (err) {
                          setWorktreeError(String(err));
                        }
                      }}
                      style={{
                        background: 'rgba(239,68,68,0.15)', border: '1px solid rgba(239,68,68,0.3)',
                        borderRadius: 6, padding: '4px 10px', cursor: 'pointer',
                        color: '#fca5a5', fontSize: 10, fontFamily: 'inherit',
                        display: 'flex', alignItems: 'center', gap: 4, flexShrink: 0,
                        whiteSpace: 'nowrap',
                      }}
                    >
                      <RotateCcw size={10} /> Retry
                    </button>
                    <button
                      onClick={() => setWorktreeError(null)}
                      style={{
                        background: 'none', border: 'none', padding: 2, cursor: 'pointer',
                        color: 'rgba(255,255,255,0.3)', display: 'flex', flexShrink: 0,
                      }}
                    >
                      <X size={12} />
                    </button>
                  </div>
                )}

                {/* Textarea */}
                <textarea
                  ref={chatInputRef}
                  style={{
                    width: '100%', padding: '12px 14px 4px', background: 'transparent',
                    border: 'none', borderRadius: '12px 12px 0 0', color: '#e8eaed',
                    fontSize: 13, fontFamily: 'inherit', resize: 'none',
                    minHeight: 42, maxHeight: 160, lineHeight: 1.4,
                    outline: 'none',
                  }}
                  rows={1}
                  placeholder={activeDiscussion && (activeDiscussion.participants?.length ?? 0) > 1 && AGENT_MENTIONS.length > 0
                    ? t('disc.mentionHint', AGENT_MENTIONS.map(m => m.trigger).join(', '))
                    : t('disc.messagePlaceholder')}
                  defaultValue=""
                  onChange={e => {
                    const val = e.target.value;
                    chatInputValueRef.current = val;
                    // Debounce state update — only needed for send button style + voice auto-send
                    const hadText = chatInputHasText;
                    const hasText = val.trim().length > 0;
                    if (hadText !== hasText) setChatInput(val);
                    // Auto-resize (use rAF to avoid layout thrashing)
                    const ta = e.target;
                    requestAnimationFrame(() => { ta.style.height = 'auto'; ta.style.height = Math.min(ta.scrollHeight, 160) + 'px'; });
                    const atMatch = val.match(/^@(\w*)$/);
                    if (atMatch) {
                      setMentionQuery(atMatch[1].toLowerCase());
                      setMentionIndex(0);
                    } else {
                      setMentionQuery(null);
                    }
                  }}
                  onKeyDown={e => {
                    if (mentionQuery !== null) {
                      const filtered = AGENT_MENTIONS.filter(m => m.trigger.slice(1).startsWith(mentionQuery ?? ''));
                      if (e.key === 'ArrowDown') { e.preventDefault(); setMentionIndex(i => Math.min(i + 1, filtered.length - 1)); return; }
                      if (e.key === 'ArrowUp') { e.preventDefault(); setMentionIndex(i => Math.max(i - 1, 0)); return; }
                      if ((e.key === 'Tab' || e.key === 'Enter') && filtered.length > 0) {
                        e.preventDefault();
                        updateChatInput(filtered[mentionIndex].trigger + ' ');
                        setMentionQuery(null);
                        return;
                      }
                      if (e.key === 'Escape') { setMentionQuery(null); return; }
                    }
                    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); handleSendMessage(); }
                  }}
                  disabled={sending || activeAgentDisabled}
                />

                {/* Bottom toolbar inside composer */}
                <div style={{
                  display: 'flex', alignItems: 'center', padding: isMobile ? '4px 4px 8px' : '4px 8px 8px',
                  gap: 2, ...(isMobile ? { flexWrap: 'wrap' as const } : {}),
                }}>
                  {/* Left: secondary actions */}
                  <div style={{ display: 'flex', alignItems: 'center', gap: 2 }}>
                    {/* Mic / STT */}
                    <button
                      onClick={handleMicToggle}
                      disabled={sending || sttState === 'transcribing'}
                      title={sttState === 'recording' ? t('disc.micStop') : t('disc.micDictate')}
                      style={{
                        background: sttState === 'recording' ? 'rgba(255,77,106,0.15)' : 'transparent',
                        border: 'none', borderRadius: 6, padding: '6px 7px', cursor: 'pointer',
                        color: sttState === 'recording' ? '#ff4d6a' : 'rgba(255,255,255,0.3)',
                        display: 'flex', alignItems: 'center',
                        transition: 'color 0.15s, background 0.15s',
                      }}
                    >
                      {sttState === 'recording' ? <MicOff size={15} /> : <Mic size={15} />}
                    </button>

                    {/* Voice conversation mode */}
                    <button
                      onClick={() => {
                        const next = !voiceMode;
                        setVoiceMode(next);
                        if (next) {
                          setTtsEnabled(true);
                        } else {
                          if (voiceCountdownRef.current) { clearInterval(voiceCountdownRef.current); voiceCountdownRef.current = null; }
                          setVoiceCountdown(null);
                        }
                      }}
                      title={voiceMode ? t('disc.voiceModeOff') : t('disc.voiceModeOn')}
                      style={{
                        background: voiceMode ? 'rgba(200,255,0,0.12)' : 'transparent',
                        border: 'none', borderRadius: 6, padding: '6px 7px', cursor: 'pointer',
                        color: voiceMode ? '#c8ff00' : 'rgba(255,255,255,0.3)',
                        display: 'flex', alignItems: 'center',
                        transition: 'color 0.15s, background 0.15s',
                      }}
                    >
                      {voiceMode ? <Phone size={15} /> : <PhoneOff size={15} />}
                    </button>

                    {/* TTS toggle */}
                    <button
                      onClick={() => {
                        setTtsEnabled(prev => {
                          if (prev) { stopTts(); setTtsState('idle'); setTtsPlayingMsgId(null); }
                          return !prev;
                        });
                      }}
                      title={ttsEnabled ? t('disc.ttsDisable') : t('disc.ttsEnable')}
                      style={{
                        background: 'transparent', border: 'none', borderRadius: 6,
                        padding: '6px 7px', cursor: 'pointer',
                        color: ttsEnabled ? '#c8ff00' : 'rgba(255,255,255,0.3)',
                        display: 'flex', alignItems: 'center',
                        transition: 'color 0.15s',
                      }}
                    >
                      {ttsEnabled ? <Volume2 size={15} /> : <VolumeX size={15} />}
                    </button>

                    {/* Debate / multi-agent */}
                    <div style={{ position: 'relative' }}>
                      <button
                        onClick={() => {
                          if (!showDebatePopover) {
                            setDebateAgents(installedAgentsList.map(a => a.agent_type));
                          }
                          setShowDebatePopover(!showDebatePopover);
                        }}
                        disabled={sending}
                        title={t('debate.title')}
                        style={{
                          background: showDebatePopover ? 'rgba(139,92,246,0.12)' : 'transparent',
                          border: 'none', borderRadius: 6, padding: '6px 7px', cursor: 'pointer',
                          color: showDebatePopover ? '#8b5cf6' : 'rgba(255,255,255,0.3)',
                          display: 'flex', alignItems: 'center',
                          transition: 'color 0.15s, background 0.15s',
                        }}
                      >
                        <Users size={15} />
                      </button>
                      {showDebatePopover && (
                        <div style={{
                          position: 'absolute', bottom: '100%', left: 0, marginBottom: 8,
                          width: 260, padding: 14, borderRadius: 10,
                          background: '#1a1d26', border: '1px solid rgba(139,92,246,0.2)',
                          boxShadow: '0 8px 32px rgba(0,0,0,0.5)', zIndex: 10,
                        }}>
                          <div style={{ fontSize: 12, fontWeight: 700, color: '#8b5cf6', marginBottom: 10, display: 'flex', alignItems: 'center', gap: 6 }}>
                            <Users size={12} /> {t('debate.header')}
                          </div>
                          <p style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginBottom: 10, lineHeight: 1.4 }}>
                            {t('debate.instructions')}
                          </p>
                          {installedAgentsList.map(a => {
                            const isPrincipal = a.agent_type === activeDiscussion?.agent;
                            const checked = debateAgents.includes(a.agent_type);
                            return (
                              <label key={a.name} style={{
                                display: 'flex', alignItems: 'center', gap: 8, padding: '6px 0',
                                cursor: isPrincipal ? 'default' : 'pointer', fontSize: 12,
                                color: checked ? '#e8eaed' : 'rgba(255,255,255,0.4)',
                              }}>
                                <input
                                  type="checkbox"
                                  checked={checked}
                                  disabled={isPrincipal}
                                  onChange={() => {
                                    if (isPrincipal) return;
                                    setDebateAgents(prev =>
                                      prev.includes(a.agent_type)
                                        ? prev.filter(t => t !== a.agent_type)
                                        : [...prev, a.agent_type]
                                    );
                                  }}
                                  style={{ accentColor: '#8b5cf6' }}
                                />
                                <Cpu size={11} style={{ color: isPrincipal ? '#c8ff00' : '#8b5cf6' }} />
                                {a.name}
                                {isPrincipal && (
                                  <span style={{ fontSize: 9, color: '#c8ff00', marginLeft: 'auto' }}>{t('debate.main')}</span>
                                )}
                              </label>
                            );
                          })}
                          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 10 }}>
                            <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)' }}>{t('debate.rounds')}</span>
                            {[1, 2, 3].map(n => (
                              <button
                                key={n}
                                style={{
                                  width: 28, height: 28, borderRadius: 6, border: 'none', fontFamily: 'inherit',
                                  fontSize: 12, fontWeight: 700, cursor: 'pointer',
                                  background: debateRounds === n ? '#8b5cf6' : 'rgba(255,255,255,0.06)',
                                  color: debateRounds === n ? '#fff' : 'rgba(255,255,255,0.4)',
                                }}
                                onClick={() => setDebateRounds(n)}
                              >
                                {n}
                              </button>
                            ))}
                          </div>
                          {/* Recommended skills for debate */}
                          {(() => {
                            const DEBATE_SKILL_IDS = ['token-saver', 'devils-advocate'];
                            const discSkillIds = activeDiscussion?.skill_ids ?? [];
                            const relevantIds = [...new Set([...DEBATE_SKILL_IDS, ...discSkillIds])];
                            const relevantSkills = relevantIds
                              .map(id => availableSkills.find(s => s.id === id))
                              .filter((s): s is Skill => !!s);
                            if (relevantSkills.length === 0) return null;
                            return (
                              <div style={{ marginTop: 10, borderTop: '1px solid rgba(255,255,255,0.06)', paddingTop: 8 }}>
                                <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginBottom: 6, display: 'flex', alignItems: 'center', gap: 4 }}>
                                  <Zap size={10} /> Skills
                                </div>
                                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                                  {relevantSkills.map(skill => {
                                    const active = debateSkillIds.includes(skill.id);
                                    return (
                                      <button
                                        key={skill.id}
                                        title={skill.description || skill.name}
                                        onClick={() => setDebateSkillIds(prev =>
                                          prev.includes(skill.id)
                                            ? prev.filter(id => id !== skill.id)
                                            : [...prev, skill.id]
                                        )}
                                        style={{
                                          padding: '3px 8px', borderRadius: 6, fontFamily: 'inherit',
                                          fontSize: 10, cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 4,
                                          background: active ? 'rgba(200,255,0,0.12)' : 'rgba(255,255,255,0.04)',
                                          color: active ? '#c8ff00' : 'rgba(255,255,255,0.3)',
                                          border: active ? '1px solid rgba(200,255,0,0.2)' : '1px solid rgba(255,255,255,0.06)',
                                        }}
                                      >
                                        {active && <Check size={8} />}
                                        {skill.name}
                                      </button>
                                    );
                                  })}
                                </div>
                              </div>
                            );
                          })()}
                          {/* Directives for debate */}
                          {availableDirectives.length > 0 && (
                            <div style={{ marginTop: 10, borderTop: '1px solid rgba(255,255,255,0.06)', paddingTop: 8 }}>
                              <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginBottom: 6, display: 'flex', alignItems: 'center', gap: 4 }}>
                                <FileText size={10} /> {t('directives.title')}
                              </div>
                              <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                                {availableDirectives.map(directive => {
                                  const active = debateDirectiveIds.includes(directive.id);
                                  return (
                                    <button
                                      key={directive.id}
                                      title={directive.description || directive.name}
                                      onClick={() => setDebateDirectiveIds(prev =>
                                        prev.includes(directive.id)
                                          ? prev.filter(id => id !== directive.id)
                                          : [...prev, directive.id]
                                      )}
                                      style={{
                                        padding: '3px 8px', borderRadius: 6, fontFamily: 'inherit',
                                        fontSize: 10, cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 4,
                                        background: active ? 'rgba(245,158,11,0.12)' : 'rgba(255,255,255,0.04)',
                                        color: active ? '#fbbf24' : 'rgba(255,255,255,0.3)',
                                        border: active ? '1px solid rgba(245,158,11,0.2)' : '1px solid rgba(255,255,255,0.06)',
                                      }}
                                    >
                                      {active && <Check size={8} />}
                                      {directive.icon} {directive.name}
                                    </button>
                                  );
                                })}
                              </div>
                            </div>
                          )}
                          {debateAgents.some(a => isAgentRestricted(a)) && (
                            <div style={{ marginTop: 8, padding: '6px 8px', borderRadius: 6, background: 'rgba(255,200,0,0.06)', border: '1px solid rgba(255,200,0,0.12)', display: 'flex', alignItems: 'center', gap: 5 }}>
                              <AlertTriangle size={10} style={{ color: '#ffb400', flexShrink: 0 }} />
                              <span style={{ fontSize: 9, color: 'rgba(255,200,0,0.7)', lineHeight: 1.3 }}>
                                {t('config.restrictedDebate')}
                              </span>
                            </div>
                          )}
                          <button
                            style={{
                              marginTop: 8, width: '100%', padding: '8px 12px', borderRadius: 6,
                              border: 'none', fontFamily: 'inherit', fontSize: 12, fontWeight: 700, cursor: 'pointer',
                              background: debateAgents.length >= 2 ? '#8b5cf6' : 'rgba(255,255,255,0.06)',
                              color: debateAgents.length >= 2 ? '#fff' : 'rgba(255,255,255,0.25)',
                            }}
                            disabled={debateAgents.length < 2}
                            onClick={handleOrchestrate}
                          >
                            {t('debate.launch', debateAgents.length)}
                          </button>
                        </div>
                      )}
                    </div>
                  </div>

                  {/* Spacer */}
                  <div style={{ flex: 1 }} />

                  {/* Right: shortcut hint + primary action */}
                  <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.15)', marginRight: 4, userSelect: 'none' }}>
                    {sending ? '' : 'Enter'}
                  </span>

                  {sending ? (
                    <button
                      onClick={handleStop}
                      title={t('disc.stopThinking')}
                      aria-label={t('disc.stopThinking')}
                      style={{
                        background: 'rgba(255,77,106,0.15)', border: '1px solid rgba(255,77,106,0.3)',
                        borderRadius: 8, padding: '6px 10px', cursor: 'pointer',
                        color: '#ff4d6a', display: 'flex', alignItems: 'center',
                      }}
                    >
                      <StopCircle size={16} />
                    </button>
                  ) : (
                    <button
                      onClick={handleSendMessage}
                      disabled={!chatInputHasText}
                      aria-label="Send message"
                      style={{
                        background: chatInputHasText ? 'rgba(200,255,0,0.15)' : 'transparent',
                        border: chatInputHasText ? '1px solid rgba(200,255,0,0.25)' : '1px solid rgba(255,255,255,0.06)',
                        borderRadius: 8, padding: '6px 10px', cursor: chatInputHasText ? 'pointer' : 'default',
                        color: chatInputHasText ? '#c8ff00' : 'rgba(255,255,255,0.15)',
                        display: 'flex', alignItems: 'center',
                        transition: 'all 0.15s',
                      }}
                    >
                      <Send size={16} />
                    </button>
                  )}
                </div>
              </div>
            </div>

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
          <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', flex: 1, color: 'rgba(255,255,255,0.2)' }}>
            {isMobile && (
              <button
                style={{ position: 'absolute', top: 14, left: 14, background: 'none', border: 'none', color: 'rgba(255,255,255,0.5)', cursor: 'pointer', padding: 4, display: 'flex', alignItems: 'center' }}
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

// ─── MessageBubble component (memo'd to avoid re-rendering all messages) ─────

interface MessageBubbleProps {
  msg: DiscussionMessage;
  idx: number;
  isLastUser: boolean;
  isLastAgent: boolean;
  isEditing: boolean;
  isCopied: boolean;
  isTtsActive: boolean;
  ttsState: string;
  isExpandedSummary: boolean;
  prevUserTs: string | null;
  defaultAgent: AgentType;
  summaryCache: string | null;
  language: string;
  sending: boolean;
  editingText: string;
  hasFullAccess: boolean;
  onCopy: (msgId: string, content: string) => void;
  onTts: (msgId: string, content: string, lang: string) => void;
  onEditStart: (msgId: string, content: string) => void;
  onEditCancel: () => void;
  onEditSubmit: () => void;
  onEditTextChange: (text: string) => void;
  onRetry: () => void;
  onExpandSummary: (msgId: string) => void;
  onNavigate: (page: string, opts?: { scrollTo?: string }) => void;
  t: (key: string, ...args: any[]) => string;
}

const msgBubbleSystemSummary = { borderColor: 'rgba(52,211,153,0.3)', background: 'rgba(52,211,153,0.06)' } as const;
const msgBubbleSystemError = { borderColor: 'rgba(255,77,106,0.3)', background: 'rgba(255,77,106,0.06)' } as const;

const MessageBubble = memo(function MessageBubble(props: MessageBubbleProps) {
  const { msg, isLastUser, isLastAgent, isEditing, isCopied, isTtsActive, ttsState: tts, isExpandedSummary,
    prevUserTs, defaultAgent, summaryCache, language, sending, editingText, hasFullAccess,
    onCopy, onTts, onEditStart, onEditCancel, onEditSubmit, onEditTextChange, onRetry, onExpandSummary, onNavigate, t } = props;
  const isUser = msg.role === 'User';
  const agentType = msg.agent_type ?? defaultAgent;

  const copyBtn = (size: number, showLabel: boolean) => (
    <button
      onClick={() => onCopy(msg.id, msg.content)}
      title={t('disc.copyMessage')}
      style={{
        background: 'none', border: 'none', padding: '1px 4px', cursor: 'pointer',
        color: isCopied ? '#34d399' : 'rgba(255,255,255,0.2)',
        display: 'inline-flex', alignItems: 'center', gap: 3, fontSize: 9,
        transition: 'color 0.15s',
      }}
    >
      {isCopied ? <><Check size={size} /> {t('disc.copied')}</> : <><Copy size={size} /> {showLabel && t('disc.copy')}</>}
    </button>
  );

  // Duration calculation (O(1) — prevUserTs is pre-computed)
  const durationLabel = useMemo(() => {
    if (msg.role !== 'Agent' || !prevUserTs) return null;
    const ms = new Date(msg.timestamp).getTime() - new Date(prevUserTs).getTime();
    if (ms <= 0) return null;
    const s = Math.round(ms / 1000);
    return s >= 60 ? `${Math.floor(s / 60)}m${s % 60 ? ` ${s % 60}s` : ''}` : `${s}s`;
  }, [msg.role, msg.timestamp, prevUserTs]);

  // Memoize formatted time
  const formattedTime = useMemo(() =>
    new Date(msg.timestamp).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' }),
    [msg.timestamp]
  );

  return (
    <div style={ds.msgRow(isUser)}>
      <div style={{
        ...ds.msgBubble(isUser),
        ...(msg.role === 'System'
          ? msg.content.startsWith('summary cached') ? msgBubbleSystemSummary : msgBubbleSystemError
          : {}),
      }}>
        {msg.role === 'Agent' && (
          <div style={{ ...ds.msgAgent, color: agentColor(agentType), display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
            <span style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
              <Cpu size={10} /> {agentType}
            </span>
            {copyBtn(9, false)}
          </div>
        )}
        {msg.role === 'System' && (
          <div style={{ ...ds.msgAgent, color: msg.content.startsWith('summary cached') ? '#34d399' : '#ff4d6a' }}>
            {msg.content.startsWith('summary cached') ? <Zap size={10} /> : <AlertTriangle size={10} />}
            {' '}{msg.content.startsWith('summary cached') ? t('disc.summaryCached') : t('disc.system')}
            {msg.content.startsWith('summary cached') && summaryCache && (
              <button
                aria-label={t('disc.viewSummary')}
                style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'rgba(52,211,153,0.6)', fontSize: 10, fontFamily: 'inherit', marginLeft: 6, textDecoration: 'underline' }}
                onClick={() => onExpandSummary(msg.id)}
              >
                {isExpandedSummary ? t('disc.hideSummary') : t('disc.viewSummary')}
              </button>
            )}
          </div>
        )}
        {msg.role === 'System' && msg.content.startsWith('summary cached') && isExpandedSummary && summaryCache && (
          <div style={{ marginTop: 6, padding: '8px 10px', borderRadius: 6, background: 'rgba(52,211,153,0.04)', border: '1px solid rgba(52,211,153,0.15)', fontSize: 11, color: 'rgba(255,255,255,0.7)', lineHeight: 1.5, whiteSpace: 'pre-wrap' }}>
            {summaryCache}
          </div>
        )}
        {isEditing ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            <textarea
              value={editingText}
              onChange={e => onEditTextChange(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); onEditSubmit(); } }}
              style={{ width: '100%', minHeight: 60, padding: 8, borderRadius: 6, background: 'rgba(255,255,255,0.06)', border: '1px solid rgba(200,255,0,0.3)', color: '#e8eaed', fontFamily: 'inherit', fontSize: 12, resize: 'vertical' }}
              autoFocus
            />
            <div style={{ display: 'flex', gap: 6, justifyContent: 'flex-end' }}>
              <button style={{ ...ls.iconBtn, fontSize: 11, padding: '4px 10px', color: 'rgba(255,255,255,0.4)' }} onClick={onEditCancel}>{t('disc.cancel')}</button>
              <button style={{ ...ls.scanBtn, fontSize: 11, padding: '4px 10px' }} onClick={onEditSubmit} disabled={!editingText.trim()}>
                <Send size={10} /> {t('disc.resend')}
                <span style={{ fontSize: 9, opacity: 0.5, marginLeft: 4 }}>Ctrl+Enter</span>
              </button>
            </div>
          </div>
        ) : (
          <MarkdownContent content={msg.content.replace(/KRONN:(BRIEFING_COMPLETE|VALIDATION_COMPLETE|BOOTSTRAP_COMPLETE)/gi, '').trim()} />
        )}
        {msg.role === 'Agent' && (
          <button
            style={{ background: 'none', border: 'none', borderRadius: 4, padding: '2px 6px', cursor: 'pointer', color: 'rgba(255,255,255,0.25)', fontSize: 10, display: 'inline-flex', alignItems: 'center', gap: 3, marginTop: 4 }}
            onClick={() => onTts(msg.id, msg.content, language)}
            title={isTtsActive ? (tts === 'loading' ? 'Chargement...' : tts === 'playing' ? 'Pause' : tts === 'paused' ? 'Reprendre' : 'Lire') : 'Lire à voix haute'}
          >
            {isTtsActive && tts === 'loading' ? <><Loader2 size={9} style={{ animation: 'spin 1s linear infinite' }} /> TTS</>
              : isTtsActive && tts === 'playing' ? <><Pause size={9} /> Pause</>
              : isTtsActive && tts === 'paused' ? <><Play size={9} /> Reprendre</>
              : <><Play size={9} /> TTS</>}
          </button>
        )}
        {RE_AUTH_ERROR.test(msg.content) && (
          <div style={{ display: 'flex', gap: 8, marginTop: 8, flexWrap: 'wrap' }}>
            <button style={{ ...ls.scanBtn, fontSize: 11, padding: '5px 12px' }} onClick={() => onNavigate('settings')}>
              <Key size={11} /> {t('disc.overrideKey')}
            </button>
            <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)', alignSelf: 'center' }}>{t('disc.orCheckAgent')}</span>
          </div>
        )}
        {RE_PARTIAL_RESPONSE.test(msg.content) && (
          <div style={{ display: 'flex', gap: 8, marginTop: 8, flexWrap: 'wrap' }}>
            <button style={{ ...ls.scanBtn, fontSize: 11, padding: '5px 12px', borderColor: 'rgba(245,158,11,0.3)', background: 'rgba(245,158,11,0.08)', color: '#f59e0b' }} onClick={() => onNavigate('settings', { scrollTo: 'settings-server' })}>
              <Settings size={11} /> {t('disc.editTimeout')}
            </button>
          </div>
        )}
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginTop: 4 }}>
          <div style={{ ...ds.msgTime, display: 'flex', alignItems: 'center', gap: 6 }}>
            {formattedTime}
            {msg.tokens_used > 0 && <span style={{ color: 'rgba(255,255,255,0.2)', fontSize: 9 }}>{msg.tokens_used.toLocaleString()} tok</span>}
            {msg.auth_mode && <span style={{ color: msg.auth_mode === 'override' ? 'rgba(200,255,0,0.3)' : 'rgba(255,255,255,0.15)', fontSize: 9 }}>{msg.auth_mode === 'override' ? 'API key' : 'auth locale'}</span>}
            {durationLabel && <span style={{ color: 'rgba(255,255,255,0.2)', fontSize: 9, display: 'inline-flex', alignItems: 'center', gap: 2 }}><Clock size={8} /> {durationLabel}</span>}
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            {msg.role === 'Agent' && copyBtn(9, true)}
            {msg.role === 'Agent' && hasFullAccess && (
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3, fontSize: 9, color: 'rgba(255,200,0,0.5)', padding: '1px 5px', borderRadius: 4, background: 'rgba(255,200,0,0.06)', border: '1px solid rgba(255,200,0,0.1)' }}>
                <AlertTriangle size={8} /> {t('config.fullAccessBadge')}
              </span>
            )}
            {msg.role === 'Agent' && msg.model_tier && (
              <span style={{
                display: 'inline-flex', alignItems: 'center', gap: 3, fontSize: 9, padding: '1px 5px', borderRadius: 4,
                color: msg.model_tier === 'economy' ? 'rgba(52,211,153,0.6)' : 'rgba(245,158,11,0.6)',
                background: msg.model_tier === 'economy' ? 'rgba(52,211,153,0.06)' : 'rgba(245,158,11,0.06)',
                border: `1px solid ${msg.model_tier === 'economy' ? 'rgba(52,211,153,0.15)' : 'rgba(245,158,11,0.15)'}`,
              }}>
                {msg.model_tier === 'economy' ? '⚡' : '🧠'} {t(`disc.tier.${msg.model_tier}`)}
              </span>
            )}
            {!sending && !isEditing && (isLastUser || isLastAgent) && (
              <div style={{ display: 'flex', gap: 4 }}>
                {isLastUser && (
                  <button style={{ ...ls.iconBtn, padding: '2px 6px', fontSize: 10, color: 'rgba(255,255,255,0.3)' }} onClick={() => onEditStart(msg.id, msg.content)} title={t('disc.editResend')} aria-label={t('disc.editResend')}>
                    <Pencil size={10} />
                  </button>
                )}
                {isLastAgent && (
                  <button style={{ ...ls.iconBtn, padding: '2px 6px', fontSize: 10, color: 'rgba(255,255,255,0.3)' }} onClick={onRetry} title={t('disc.retryResponse')} aria-label={t('disc.retryResponse')}>
                    <RotateCcw size={10} />
                  </button>
                )}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
});

// ─── MarkdownContent component ───────────────────────────────────────────────

const mdStyles: Record<string, React.CSSProperties> = {
  p: { margin: '4px 0' },
  h1: { fontSize: 18, fontWeight: 700, margin: '12px 0 6px', color: '#e8eaed' },
  h2: { fontSize: 16, fontWeight: 700, margin: '10px 0 4px', color: '#e8eaed' },
  h3: { fontSize: 14, fontWeight: 600, margin: '8px 0 4px', color: '#e8eaed' },
  ul: { margin: '4px 0', paddingLeft: 20 },
  ol: { margin: '4px 0', paddingLeft: 20 },
  li: { margin: '2px 0' },
  code: { background: 'rgba(255,255,255,0.08)', padding: '1px 5px', borderRadius: 4, fontSize: 12, fontFamily: 'monospace' },
  pre: { background: 'rgba(0,0,0,0.3)', padding: '10px 12px', borderRadius: 8, overflowX: 'auto', margin: '6px 0', border: '1px solid rgba(255,255,255,0.06)' },
  preCode: { background: 'none', padding: 0, fontSize: 12, fontFamily: 'monospace', color: '#c8ff00' },
  table: { borderCollapse: 'collapse' as const, width: '100%', margin: '8px 0', fontSize: 12 },
  th: { border: '1px solid rgba(255,255,255,0.12)', padding: '6px 10px', background: 'rgba(255,255,255,0.05)', fontWeight: 600, textAlign: 'left' as const },
  td: { border: '1px solid rgba(255,255,255,0.08)', padding: '5px 10px' },
  blockquote: { borderLeft: '3px solid rgba(200,255,0,0.3)', margin: '6px 0', paddingLeft: 12, color: 'rgba(255,255,255,0.6)' },
  hr: { border: 'none', borderTop: '1px solid rgba(255,255,255,0.1)', margin: '10px 0' },
  a: { color: '#c8ff00', textDecoration: 'underline' },
  strong: { fontWeight: 700, color: '#f0f0f0' },
};

/** Extract plain text from a DOM node tree (for copy-to-clipboard). */
function extractText(node: HTMLElement): string {
  if (node.tagName === 'TABLE') {
    const rows = Array.from(node.querySelectorAll('tr'));
    return rows.map(row => {
      const cells = Array.from(row.querySelectorAll('th, td'));
      return cells.map(c => c.textContent?.trim() ?? '').join('\t');
    }).join('\n');
  }
  return node.textContent ?? '';
}

/** Tiny copy button overlaid on a block (table or code). */
function CopyableBlock({ children, style, tag }: { children: any; style?: Record<string, any>; tag: 'table' | 'pre' }) {
  const ref = useRef<HTMLDivElement>(null);
  const [copied, setCopied] = useState(false);
  const handleCopy = () => {
    const el = ref.current?.querySelector(tag);
    if (el) {
      navigator.clipboard.writeText(extractText(el as HTMLElement));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    }
  };
  return (
    <div ref={ref} style={{ position: 'relative', ...style }}>
      {children}
      <button
        onClick={handleCopy}
        style={{
          position: 'absolute', top: 4, right: 4,
          background: 'rgba(0,0,0,0.5)', border: '1px solid rgba(255,255,255,0.1)',
          borderRadius: 4, padding: '2px 6px', cursor: 'pointer',
          color: copied ? '#34d399' : 'rgba(255,255,255,0.4)',
          fontSize: 9, display: 'flex', alignItems: 'center', gap: 3,
          transition: 'color 0.15s, opacity 0.15s', opacity: 0.6,
        }}
        onMouseEnter={e => (e.currentTarget.style.opacity = '1')}
        onMouseLeave={e => (e.currentTarget.style.opacity = '0.6')}
      >
        {copied ? <>{'\u2713'}</> : <>{'\u2398'}</>}
      </button>
    </div>
  );
}

const mdComponents = {
  p: ({ children }: any) => <p style={mdStyles.p}>{children}</p>,
  h1: ({ children }: any) => <h1 style={mdStyles.h1}>{children}</h1>,
  h2: ({ children }: any) => <h2 style={mdStyles.h2}>{children}</h2>,
  h3: ({ children }: any) => <h3 style={mdStyles.h3}>{children}</h3>,
  ul: ({ children }: any) => <ul style={mdStyles.ul}>{children}</ul>,
  ol: ({ children }: any) => <ol style={mdStyles.ol}>{children}</ol>,
  li: ({ children }: any) => <li style={mdStyles.li}>{children}</li>,
  code: ({ className, children }: any) => {
    const isBlock = className?.includes('language-');
    return isBlock
      ? <code style={mdStyles.preCode}>{children}</code>
      : <code style={mdStyles.code}>{children}</code>;
  },
  pre: ({ children }: any) => (
    <CopyableBlock tag="pre">
      <pre style={mdStyles.pre}>{children}</pre>
    </CopyableBlock>
  ),
  table: ({ children }: any) => (
    <CopyableBlock tag="table" style={{ overflowX: 'auto' }}>
      <table style={mdStyles.table}>{children}</table>
    </CopyableBlock>
  ),
  th: ({ children }: any) => <th style={mdStyles.th}>{children}</th>,
  td: ({ children }: any) => <td style={mdStyles.td}>{children}</td>,
  blockquote: ({ children }: any) => <blockquote style={mdStyles.blockquote}>{children}</blockquote>,
  hr: () => <hr style={mdStyles.hr} />,
  a: ({ href, children }: any) => <a href={href} style={mdStyles.a} target="_blank" rel="noopener noreferrer">{children}</a>,
  strong: ({ children }: any) => <strong style={mdStyles.strong}>{children}</strong>,
};

const remarkPluginsList = [remarkGfm];

const MarkdownContent = memo(({ content }: { content: string }) => (
  <div style={{ fontSize: 13, lineHeight: 1.55 }}>
    <ReactMarkdown remarkPlugins={remarkPluginsList} components={mdComponents}>
      {content}
    </ReactMarkdown>
  </div>
));

// ─── Local styles (from Dashboard s.*) ───────────────────────────────────────

const ls = {
  iconBtn: { background: 'none', border: '1px solid rgba(255,255,255,0.08)', borderRadius: 4, padding: '4px 8px', color: 'rgba(255,255,255,0.5)', cursor: 'pointer', display: 'flex', alignItems: 'center', fontSize: 11 } as const,
  scanBtn: { padding: '7px 14px', borderRadius: 6, border: '1px solid rgba(200,255,0,0.2)', background: 'rgba(200,255,0,0.05)', color: '#c8ff00', cursor: 'pointer', fontSize: 12, fontFamily: 'inherit', fontWeight: 500, display: 'flex', alignItems: 'center', gap: 6 } as const,
};

// ─── Discussion styles ───────────────────────────────────────────────────────

const ds = {
  sidebar: { width: 280, borderRight: '1px solid rgba(255,255,255,0.07)', background: '#0e1117', display: 'flex', flexDirection: 'column' as const, flexShrink: 0 },
  sidebarHeader: { display: 'flex', justifyContent: 'space-between', alignItems: 'center', padding: '14px 14px 10px', borderBottom: '1px solid rgba(255,255,255,0.06)' } as const,
  sidebarList: { flex: 1, overflowY: 'auto' as const, padding: '8px 0' },
  projectGroup: {
    fontSize: 10, fontWeight: 700, textTransform: 'uppercase' as const, letterSpacing: '0.06em',
    color: 'rgba(200,255,0,0.5)', padding: '14px 14px 6px',
    marginTop: 4, borderTop: '1px solid rgba(255,255,255,0.05)',
    display: 'flex', alignItems: 'center', gap: 6,
  },
  discItem: (active: boolean) => ({
    display: 'flex', alignItems: 'center', gap: 8, width: '100%', padding: '8px 14px 8px 22px', border: 'none',
    background: active ? 'rgba(200,255,0,0.06)' : 'transparent',
    borderLeft: active ? '2px solid #c8ff00' : '2px solid transparent',
    color: active ? '#e8eaed' : 'rgba(255,255,255,0.5)',
    cursor: 'pointer', textAlign: 'left' as const, fontFamily: 'inherit',
  }),
  chatArea: { flex: 1, display: 'flex', flexDirection: 'column' as const, minWidth: 0, background: '#0a0c10' },
  chatHeader: { display: 'flex', alignItems: 'center', gap: 12, padding: '14px 20px', borderBottom: '1px solid rgba(255,255,255,0.07)', background: '#12151c', flexShrink: 0 } as const,
  messages: { flex: 1, overflowY: 'auto' as const, padding: '20px 20px 10px' },
  // Pre-computed style variants (avoid creating new objects on every render)
  msgRowUser: { display: 'flex', justifyContent: 'flex-end', marginBottom: 12 } as const,
  msgRowAgent: { display: 'flex', justifyContent: 'flex-start', marginBottom: 12 } as const,
  msgRow: (isUser: boolean) => isUser ? ds.msgRowUser : ds.msgRowAgent,
  msgBubbleUser: { maxWidth: '70%', padding: '10px 14px', borderRadius: 12, background: 'rgba(200,255,0,0.08)', border: '1px solid rgba(200,255,0,0.15)', color: '#e8eaed', overflowWrap: 'break-word' as const, wordBreak: 'break-word' as const, minWidth: 0 } as const,
  msgBubbleAgent: { maxWidth: '70%', padding: '10px 14px', borderRadius: 12, background: 'rgba(255,255,255,0.04)', border: '1px solid rgba(255,255,255,0.07)', color: '#e8eaed', overflowWrap: 'break-word' as const, wordBreak: 'break-word' as const, minWidth: 0 } as const,
  msgBubble: (isUser: boolean) => isUser ? ds.msgBubbleUser : ds.msgBubbleAgent,
  msgAgent: { display: 'flex', alignItems: 'center', gap: 4, fontSize: 10, fontWeight: 600, color: 'rgba(139,92,246,0.7)', marginBottom: 4 } as const,
  msgTime: { fontSize: 10, color: 'rgba(255,255,255,0.55)', marginTop: 4, textAlign: 'right' as const },
  inputBar: { display: 'flex', gap: 8, padding: '12px 20px', borderTop: '1px solid rgba(255,255,255,0.07)', background: '#12151c', flexShrink: 0 } as const,
  chatInput: { width: '100%', padding: '10px 14px', background: 'rgba(255,255,255,0.04)', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit' } as const,
  sendBtn: { padding: '10px 16px', borderRadius: 8, border: '1px solid rgba(200,255,0,0.2)', background: 'rgba(200,255,0,0.1)', color: '#c8ff00', cursor: 'pointer', display: 'flex', alignItems: 'center', justifyContent: 'center' } as const,
  newDiscOverlay: { display: 'flex', alignItems: 'center', justifyContent: 'center', flex: 1 } as const,
  newDiscCard: { width: 420, maxHeight: 'calc(100vh - 120px)', overflowY: 'auto', padding: 24, borderRadius: 12, background: '#12151c', border: '1px solid rgba(255,255,255,0.1)' } as const,
  label: { display: 'block', fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', marginBottom: 4, marginTop: 8 } as const,
  selectStyled: {
    width: '100%', padding: '9px 12px', background: '#1a1d26', border: '1px solid rgba(255,255,255,0.12)',
    borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit',
    cursor: 'pointer', appearance: 'none' as const,
    backgroundImage: `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%23888' stroke-width='2'%3E%3Cpolyline points='6 9 12 15 18 9'%3E%3C/polyline%3E%3C/svg%3E")`,
    backgroundRepeat: 'no-repeat', backgroundPosition: 'right 10px center',
    paddingRight: 32,
  } as const,
  inputStyled: {
    width: '100%', padding: '9px 12px', background: '#1a1d26', border: '1px solid rgba(255,255,255,0.12)',
    borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit',
    boxSizing: 'border-box' as const,
  } as const,
  textareaStyled: {
    width: '100%', padding: '10px 12px', background: '#1a1d26', border: '1px solid rgba(255,255,255,0.12)',
    borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit',
    resize: 'vertical' as const, boxSizing: 'border-box' as const, lineHeight: 1.5,
  } as const,
};
