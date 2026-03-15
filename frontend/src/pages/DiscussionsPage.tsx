import { useState, useRef, useEffect, useCallback, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { discussions as discussionsApi, projects as projectsApi, skills as skillsApi, profiles as profilesApi, directives as directivesApi } from '../lib/api';
import type { Project, AgentDetection, Discussion, AgentType, AgentsConfig, Skill, AgentProfile, Directive } from '../types/generated';
import { useT } from '../lib/I18nContext';
import { AGENT_LABELS, agentColor, isAgentRestricted as isAgentRestrictedUtil, hasAgentFullAccess } from '../lib/constants';
import type { ToastFn } from '../hooks/useToast';
import {
  Folder, ChevronRight, Cpu,
  Plus, Trash2, Loader2,
  MessageSquare, Send, X, Key, AlertTriangle, Users,
  StopCircle, RotateCcw, Pencil, ShieldCheck, Check, Archive, Zap, UserCircle, FileText, Settings, Rocket,
} from 'lucide-react';

const isHiddenPath = (path: string) => path.split('/').some(s => s.startsWith('.'));

/** Agent is usable: locally installed OR available via npx/uvx runtime fallback */
const isUsable = (a: AgentDetection) => (a.installed || a.runtime_available) && a.enabled;

const isValidationDisc = (title: string) => title === 'Validation audit AI';
const isBootstrapDisc = (title: string) => title.startsWith('Bootstrap: ');

const ALL_AGENT_MENTIONS: { trigger: string; type: AgentType; label: string }[] = [
  { trigger: '@claude', type: 'ClaudeCode', label: 'Claude Code' },
  { trigger: '@codex', type: 'Codex', label: 'Codex' },
  { trigger: '@vibe', type: 'Vibe', label: 'Vibe' },
  { trigger: '@gemini', type: 'GeminiCli', label: 'Gemini CLI' },
  { trigger: '@kiro', type: 'Kiro', label: 'Kiro' },
];

const SWIPE_THRESHOLD = 80;

function SwipeableDiscItem({ disc, isActive, lastSeenCount, sendingMap, onSelect, onArchive, onDelete, t, archiveLabel }: {
  disc: Discussion;
  isActive: boolean;
  lastSeenCount: number;
  sendingMap: Record<string, boolean>;
  onSelect: () => void;
  onArchive: () => void;
  onDelete: () => void;
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
      onArchive();
    } else if (offsetX < -SWIPE_THRESHOLD) {
      onDelete();
    } else if (Math.abs(offsetX) < 5) {
      onSelect();
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
            {isBootstrapDisc(disc.title) && <Rocket size={10} style={{ color: '#c8ff00', flexShrink: 0 }} />}
            {disc.title}
            {showBadge && (
              <span style={{
                background: '#c8ff00', color: '#0a0c10', fontSize: 8, fontWeight: 800,
                borderRadius: 6, padding: '1px 5px', minWidth: 14, textAlign: 'center', flexShrink: 0,
              }}>{unseen}</span>
            )}
          </div>
          <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', marginTop: 2, display: 'flex', alignItems: 'center', gap: 4 }}>
            {sendingMap[disc.id] && <Loader2 size={8} style={{ animation: 'spin 1s linear infinite', color: '#c8ff00' }} />}
            {(disc.participants?.length ?? 0) > 1 && (
              <Users size={8} style={{ color: '#8b5cf6' }} />
            )}
            {disc.message_count ?? disc.messages.length} msg · {disc.agent}
          </div>
        </div>
      </div>
    </div>
  );
}

export interface DiscussionsPageProps {
  projects: Project[];
  agents: AgentDetection[];
  allDiscussions: Discussion[];
  configLanguage: string | null;
  agentAccess: AgentsConfig | null;
  refetchDiscussions: () => void;
  refetchProjects: () => void;
  onNavigate: (page: string) => void;
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
  streamingMap: Record<string, string>;
  setStreamingMap: React.Dispatch<React.SetStateAction<Record<string, string>>>;
  abortControllers: React.MutableRefObject<Record<string, AbortController>>;
  cleanupStream: (discId: string) => void;
  // Lifted unseen tracking (lives in Dashboard for cross-page visibility)
  markDiscussionSeen: (discId: string, msgCount: number) => void;
  onActiveDiscussionChange: (id: string | null) => void;
  lastSeenMsgCount: Record<string, number>;
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
  streamingMap,
  setStreamingMap,
  abortControllers,
  cleanupStream: cleanupStreamBase,
  markDiscussionSeen,
  onActiveDiscussionChange,
  lastSeenMsgCount,
  initialActiveDiscussionId,
}: DiscussionsPageProps) {
  const { t } = useT();

  // ─── Internal state ──────────────────────────────────────────────────────
  const [activeDiscussionId, setActiveDiscussionId] = useState<string | null>(initialActiveDiscussionId ?? null);
  const [showNewDiscussion, setShowNewDiscussion] = useState(false);
  const [newDiscTitle, setNewDiscTitle] = useState('');
  const [newDiscAgent, setNewDiscAgent] = useState<AgentType | ''>('');
  const [newDiscProjectId, setNewDiscProjectId] = useState<string>('');
  const [newDiscPrompt, setNewDiscPrompt] = useState('');
  const [newDiscPrefilled, setNewDiscPrefilled] = useState(false);
  const [showAdvancedOptions, setShowAdvancedOptions] = useState(false);
  const [chatInput, setChatInput] = useState('');
  const [editingMsgId, setEditingMsgId] = useState<string | null>(null);
  const [editingText, setEditingText] = useState('');
  const [collapsedDiscGroups, setCollapsedDiscGroups] = useState<Set<string>>(new Set());
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

  const chatInputRef = useRef<HTMLTextAreaElement>(null);
  const chatEndRef = useRef<HTMLDivElement>(null);

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

  // ─── Derived data ────────────────────────────────────────────────────────
  const activeDiscussion = (activeDiscussionId && loadedDiscussions[activeDiscussionId])
    ? loadedDiscussions[activeDiscussionId]
    : allDiscussions.find(d => d.id === activeDiscussionId) ?? null;

  const activeAgentDisabled = useMemo(() => {
    if (!activeDiscussion || agents.length === 0) return false;
    const agentDet = agents.find(a => a.agent_type === activeDiscussion.agent);
    return !agentDet || !isUsable(agentDet);
  }, [activeDiscussion, agents]);

  const installedAgentsList = agents.filter(isUsable);

  const sending = activeDiscussionId ? !!sendingMap[activeDiscussionId] : false;
  const streamingText = activeDiscussionId ? (streamingMap[activeDiscussionId] ?? '') : '';

  const AGENT_MENTIONS = useMemo(() => {
    const activeAgentTypes = new Set(agents.filter(isUsable).map(a => a.agent_type));
    return ALL_AGENT_MENTIONS.filter(m => activeAgentTypes.has(m.type));
  }, [agents]);

  const installedAgents = agents.filter(isUsable);

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
    directivesApi.list().then(setAvailableDirectives).catch(console.error);
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
    if (activeDiscussionId && activeDiscussion && !sendingMap[activeDiscussionId]) {
      markDiscussionSeen(activeDiscussionId, activeDiscussion.messages.length);
    }
  }, [activeDiscussionId, activeDiscussion?.messages.length, sendingMap, markDiscussionSeen]);

  // Auto-scroll on new messages / streaming
  useEffect(() => {
    chatEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [activeDiscussion?.messages.length, streamingText]);

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
    reloadDiscussion(discId);
  }, [cleanupStreamBase, refetchDiscussions, reloadDiscussion]);

  // Handle auto-run: open existing discussion and trigger agent (e.g. after full audit)
  // Uses a ref to track the pending run so that re-renders (from onAutoRunConsumed/refetch)
  // don't cancel the timeout via effect cleanup.
  const pendingAutoRun = useRef<string | null>(null);
  useEffect(() => {
    if (!autoRunDiscussionId || pendingAutoRun.current === autoRunDiscussionId) return;
    const discId = autoRunDiscussionId;
    pendingAutoRun.current = discId;
    onAutoRunConsumed?.();

    // Select the discussion and show loader immediately
    setActiveDiscussionId(discId);
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
        (text) => setStreamingMap(prev => ({ ...prev, [discId]: (prev[discId] ?? '') + text })),
        () => cleanupStream(discId),
        (error) => { console.error('Agent error:', error); toast(String(error), 'error'); cleanupStream(discId); },
        controller.signal,
      );
    }, 500);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoRunDiscussionId]);

  // Handle open-discussion: just select it without triggering agent (e.g. Resume Validation)
  useEffect(() => {
    if (!openDiscussionId) return;
    setActiveDiscussionId(openDiscussionId);
    onOpenDiscConsumed?.();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openDiscussionId]);

  const handleCreateDiscussion = async () => {
    if (!newDiscPrompt.trim() || !newDiscAgent) return;
    const prompt = newDiscPrompt.trim();
    const title = newDiscTitle.trim() || prompt.slice(0, 60);
    const disc = await discussionsApi.create({
      project_id: newDiscProjectId || null,
      title,
      agent: newDiscAgent as AgentType,
      language: configLanguage ?? 'fr',
      initial_prompt: prompt,
      skill_ids: newDiscSkillIds.length > 0 ? newDiscSkillIds : undefined,
      profile_ids: newDiscProfileIds.length > 0 ? newDiscProfileIds : undefined,
      ...(newDiscDirectiveIds.length > 0 ? { directive_ids: newDiscDirectiveIds } : {}),
    });
    setShowNewDiscussion(false);
    setNewDiscTitle('');
    setNewDiscPrompt('');
    setNewDiscPrefilled(false);
    setNewDiscSkillIds([]);
    setNewDiscDirectiveIds([]);
    setNewDiscProfileIds([]);
    setActiveDiscussionId(disc.id);
    refetchDiscussions();

    const discId = disc.id;
    const controller = new AbortController();
    abortControllers.current[discId] = controller;
    setSendingMap(prev => ({ ...prev, [discId]: true }));
    setStreamingMap(prev => ({ ...prev, [discId]: '' }));
    await discussionsApi.runAgent(
      discId,
      (text) => setStreamingMap(prev => ({ ...prev, [discId]: (prev[discId] ?? '') + text })),
      () => cleanupStream(discId),
      (error) => { console.error('Agent error:', error); toast(String(error), 'error'); cleanupStream(discId); },
      controller.signal,
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

  const handleSendMessage = async () => {
    if (!activeDiscussionId || !chatInput.trim() || sending) return;
    const discId = activeDiscussionId;
    const msg = chatInput.trim();
    const { targetAgent } = parseMention(msg);
    setChatInput('');
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
        },
      };
    });

    const controller = new AbortController();
    abortControllers.current[discId] = controller;
    setStreamingMap(prev => ({ ...prev, [discId]: '' }));

    // Optimistic update: show user message immediately before agent starts
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

    await discussionsApi.sendMessageStream(
      discId,
      { content: msg, target_agent: targetAgent },
      (text) => setStreamingMap(prev => ({ ...prev, [discId]: (prev[discId] ?? '') + text })),
      () => cleanupStream(discId),
      (error) => { console.error('Agent error:', error); toast(String(error), 'error'); cleanupStream(discId); },
      controller.signal,
      () => {
        refetchDiscussions();
        setSendingMap(prev => ({ ...prev, [discId]: true }));
        // Mark seen so user's own message doesn't trigger unseen badge
        markDiscussionSeen(discId, (activeDiscussion?.messages.length ?? 0) + 1);
      },
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
    await discussionsApi.runAgent(
      discId,
      (text) => setStreamingMap(prev => ({ ...prev, [discId]: (prev[discId] ?? '') + text })),
      () => cleanupStream(discId),
      (error) => { console.error('Agent error:', error); toast(String(error), 'error'); cleanupStream(discId); },
      controller.signal,
    );
  };

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
    await discussionsApi.runAgent(
      discId,
      (text) => setStreamingMap(prev => ({ ...prev, [discId]: (prev[discId] ?? '') + text })),
      () => cleanupStream(discId),
      (error) => { console.error('Agent error:', error); toast(String(error), 'error'); cleanupStream(discId); },
      controller.signal,
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
        setOrchState(prev => {
          const s = prev[discId];
          if (!s) return prev;
          const streams = [...s.agentStreams];
          const last = [...streams].reverse().find((st: typeof streams[0]) => st.agent === agent && !st.done);
          if (last) last.text = (last.text ?? '') + text;
          return { ...prev, [discId]: { ...s, agentStreams: streams } };
        });
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
      <div style={ds.sidebar}>
        <div style={ds.sidebarHeader}>
          <span style={{ fontWeight: 600, fontSize: 13 }}>Discussions</span>
          <button style={ls.scanBtn} onClick={() => { setShowNewDiscussion(true); setNewDiscPrefilled(false); }}>
            <Plus size={12} /> {t('disc.new')}
          </button>
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
                <div
                  style={{ ...ds.projectGroup, borderTop: 'none', cursor: 'pointer', userSelect: 'none' as const }}
                  onClick={() => setCollapsedDiscGroups(prev => { const n = new Set(prev); isCollapsed ? n.delete('__global__') : n.add('__global__'); return n; })}
                >
                  <ChevronRight size={10} style={{ transform: isCollapsed ? 'none' : 'rotate(90deg)', transition: 'transform 0.15s' }} />
                  <MessageSquare size={10} /> {t('disc.general')}
                  <span style={{ fontWeight: 400, opacity: 0.5, marginLeft: 'auto' }}>{globalDiscs.length}</span>
                </div>
                {!isCollapsed && globalDiscs.sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
                  <SwipeableDiscItem
                    key={disc.id}
                    disc={disc}
                    isActive={disc.id === activeDiscussionId}
                    lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                    sendingMap={sendingMap}
                    onSelect={() => { setActiveDiscussionId(disc.id); markDiscussionSeen(disc.id, disc.message_count ?? disc.messages.length); }}
                    onArchive={async () => {
                      await discussionsApi.update(disc.id, { archived: true });
                      if (activeDiscussionId === disc.id) setActiveDiscussionId(null);
                      refetchDiscussions();
                    }}
                    onDelete={async () => {
                      await discussionsApi.delete(disc.id);
                      if (activeDiscussionId === disc.id) setActiveDiscussionId(null);
                      refetchDiscussions();
                    }}
                    t={t}
                  />
                ))}
              </div>
            );
          })()}

          {/* Project discussions */}
          {projects.filter(p => !isHiddenPath(p.path)).map(proj => {
            const projDiscs = activeDiscByProject.get(proj.id) ?? [];
            if (projDiscs.length === 0) return null;
            const isCollapsed = collapsedDiscGroups.has(proj.id);
            return (
              <div key={proj.id}>
                <div
                  style={{ ...ds.projectGroup, cursor: 'pointer', userSelect: 'none' as const }}
                  onClick={() => setCollapsedDiscGroups(prev => { const n = new Set(prev); isCollapsed ? n.delete(proj.id) : n.add(proj.id); return n; })}
                >
                  <ChevronRight size={10} style={{ transform: isCollapsed ? 'none' : 'rotate(90deg)', transition: 'transform 0.15s' }} />
                  <Folder size={10} /> {proj.name}
                  <span style={{ fontWeight: 400, opacity: 0.5, marginLeft: 'auto' }}>{projDiscs.length}</span>
                </div>
                {!isCollapsed && projDiscs.sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
                  <SwipeableDiscItem
                    key={disc.id}
                    disc={disc}
                    isActive={disc.id === activeDiscussionId}
                    lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                    sendingMap={sendingMap}
                    onSelect={() => { setActiveDiscussionId(disc.id); markDiscussionSeen(disc.id, disc.message_count ?? disc.messages.length); }}
                    onArchive={async () => {
                      await discussionsApi.update(disc.id, { archived: true });
                      if (activeDiscussionId === disc.id) setActiveDiscussionId(null);
                      refetchDiscussions();
                    }}
                    onDelete={async () => {
                      await discussionsApi.delete(disc.id);
                      if (activeDiscussionId === disc.id) setActiveDiscussionId(null);
                      refetchDiscussions();
                    }}
                    t={t}
                  />
                ))}
              </div>
            );
          })}

          {allDiscussions.length === 0 && !showNewDiscussion && (
            <div style={{ padding: 24, textAlign: 'center', color: 'rgba(255,255,255,0.25)', fontSize: 12, whiteSpace: 'pre-line' }}>
              {t('disc.empty')}
            </div>
          )}

          {/* Archives section */}
          {archivedDiscussions.length > 0 && (
            <div>
              <div
                style={{ ...ds.projectGroup, cursor: 'pointer', userSelect: 'none' as const, color: 'rgba(255,255,255,0.25)' }}
                onClick={() => setShowArchives(!showArchives)}
              >
                <ChevronRight size={10} style={{ transform: showArchives ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s' }} />
                <Archive size={10} /> {t('disc.archived')}
                <span style={{ fontWeight: 400, opacity: 0.5, marginLeft: 'auto' }}>{archivedDiscussions.length}</span>
              </div>
              {showArchives && archivedDiscussions.sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
                <SwipeableDiscItem
                  key={disc.id}
                  disc={disc}
                  isActive={disc.id === activeDiscussionId}
                  lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                  sendingMap={sendingMap}
                  onSelect={() => { setActiveDiscussionId(disc.id); markDiscussionSeen(disc.id, disc.message_count ?? disc.messages.length); }}
                  onArchive={async () => {
                    // Swipe right on archived = unarchive
                    await discussionsApi.update(disc.id, { archived: false });
                    refetchDiscussions();
                  }}
                  onDelete={async () => {
                    await discussionsApi.delete(disc.id);
                    if (activeDiscussionId === disc.id) setActiveDiscussionId(null);
                    refetchDiscussions();
                  }}
                  archiveLabel={t('disc.unarchive')}
                  t={t}
                />
              ))}
            </div>
          )}
        </div>
      </div>

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
                <button style={ls.iconBtn} onClick={() => { setShowNewDiscussion(false); setNewDiscPrefilled(false); }}><X size={14} /></button>
              </div>

              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12, marginBottom: 12 }}>
                <div>
                  <label style={ds.label}>{t('disc.project')}</label>
                  <select style={{ ...ds.selectStyled, ...(newDiscPrefilled ? { opacity: 0.5, pointerEvents: 'none' as const } : {}) }} value={newDiscProjectId} onChange={e => {
                    const pid = e.target.value;
                    setNewDiscProjectId(pid);
                    const proj = projects.find(p => p.id === pid);
                    if (proj?.default_skill_ids?.length) setNewDiscSkillIds(proj.default_skill_ids);
                    if ((proj as any)?.default_profile_id) setNewDiscProfileIds([(proj as any).default_profile_id]);
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
                    {installedAgents.map(a => (
                      <option key={a.name} value={a.agent_type}>{a.name}</option>
                    ))}
                    {installedAgents.length === 0 && (
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
                    {(newDiscSkillIds.length > 0 || newDiscProfileIds.length > 0 || newDiscDirectiveIds.length > 0) && (
                      <span style={{ fontSize: 9, color: '#c8ff00', marginLeft: 2 }}>
                        ({newDiscSkillIds.length + newDiscProfileIds.length + newDiscDirectiveIds.length})
                      </span>
                    )}
                  </button>

                  {showAdvancedOptions && (
                    <div style={{ marginTop: 8, padding: '10px 12px', borderRadius: 8, background: 'rgba(255,255,255,0.02)', border: '1px solid rgba(255,255,255,0.06)' }}>
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
                                  title={skill.name}
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
                                  title={directive.name}
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
                onChange={e => !newDiscPrefilled && setNewDiscTitle(e.target.value)}
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
              <div style={{ flex: 1 }}>
                <div style={{ fontWeight: 600, fontSize: 14, display: 'flex', alignItems: 'center', gap: 6 }}>
                  {isValidationDisc(activeDiscussion.title) && <ShieldCheck size={14} style={{ color: '#c8ff00', flexShrink: 0 }} />}
                  {isBootstrapDisc(activeDiscussion.title) && <Rocket size={14} style={{ color: '#c8ff00', flexShrink: 0 }} />}
                  {editingTitleId === activeDiscussion.id && !isValidationDisc(activeDiscussion.title) && !isBootstrapDisc(activeDiscussion.title) ? (
                    <input
                      autoFocus
                      style={{
                        background: 'rgba(255,255,255,0.06)', border: '1px solid rgba(200,255,0,0.3)',
                        borderRadius: 4, padding: '2px 6px', color: '#e8eaed', fontSize: 14,
                        fontWeight: 600, fontFamily: 'inherit', outline: 'none', width: 260,
                      }}
                      value={editingTitleText}
                      onChange={e => setEditingTitleText(e.target.value)}
                      onKeyDown={async e => {
                        if (e.key === 'Enter' && editingTitleText.trim()) {
                          await discussionsApi.update(activeDiscussion.id, { title: editingTitleText.trim() });
                          setEditingTitleId(null);
                          refetchDiscussions();
                        }
                        if (e.key === 'Escape') setEditingTitleId(null);
                      }}
                      onBlur={async () => {
                        if (editingTitleText.trim() && editingTitleText.trim() !== activeDiscussion.title) {
                          await discussionsApi.update(activeDiscussion.id, { title: editingTitleText.trim() });
                          refetchDiscussions();
                        }
                        setEditingTitleId(null);
                      }}
                    />
                  ) : (
                    <span
                      style={{ cursor: (isValidationDisc(activeDiscussion.title) || isBootstrapDisc(activeDiscussion.title)) ? 'default' : 'pointer' }}
                      onDoubleClick={() => {
                        if (isValidationDisc(activeDiscussion.title) || isBootstrapDisc(activeDiscussion.title)) return;
                        setEditingTitleId(activeDiscussion.id);
                        setEditingTitleText(activeDiscussion.title);
                      }}
                      title={(isValidationDisc(activeDiscussion.title) || isBootstrapDisc(activeDiscussion.title)) ? undefined : t('disc.editTitle')}
                    >
                      {activeDiscussion.title}
                    </span>
                  )}
                  {!isValidationDisc(activeDiscussion.title) && !isBootstrapDisc(activeDiscussion.title) && (
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
                  >
                    <Pencil size={10} />
                  </button>
                  )}
                </div>
                <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.35)', marginTop: 2, display: 'flex', alignItems: 'center', gap: 4, flexWrap: 'wrap' }}>
                  <span>{activeDiscussion.project_id ? (projects.find(p => p.id === activeDiscussion.project_id)?.name ?? '?') : t('disc.general')} · {activeDiscussion.agent}</span>
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
              <button
                style={{ ...ls.iconBtn, color: '#ff4d6a' }}
                onClick={async () => {
                  if (!confirm(t('disc.confirmDelete'))) return;
                  await discussionsApi.delete(activeDiscussion.id);
                  setActiveDiscussionId(null);
                  refetchDiscussions();
                }}
              >
                <Trash2 size={12} />
              </button>
            </div>

            {/* Messages */}
            <div style={ds.messages}>
              {activeDiscussion.messages.map((msg, idx) => {
                const msgs = activeDiscussion.messages;
                const isLastUser = msg.role === 'User' && !msgs.slice(idx + 1).some(m => m.role === 'User');
                const isLastAgent = msg.role === 'Agent' && idx === msgs.length - 1;
                const isEditing = editingMsgId === msg.id;

                return (
                <div key={msg.id} style={ds.msgRow(msg.role === 'User')}>
                  <div style={{
                    ...ds.msgBubble(msg.role === 'User'),
                    ...(msg.role === 'System' ? { borderColor: 'rgba(255,77,106,0.3)', background: 'rgba(255,77,106,0.06)' } : {}),
                  }}>
                    {msg.role === 'Agent' && (
                      <div style={{ ...ds.msgAgent, color: agentColor(msg.agent_type ?? activeDiscussion.agent) }}>
                        <Cpu size={10} /> {msg.agent_type ?? activeDiscussion.agent}
                      </div>
                    )}
                    {msg.role === 'System' && (
                      <div style={{ ...ds.msgAgent, color: '#ff4d6a' }}>
                        <AlertTriangle size={10} /> {t('disc.system')}
                      </div>
                    )}
                    {isEditing ? (
                      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                        <textarea
                          value={editingText}
                          onChange={e => setEditingText(e.target.value)}
                          onKeyDown={e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); handleEditMessage(); } }}
                          style={{
                            width: '100%', minHeight: 60, padding: 8, borderRadius: 6,
                            background: 'rgba(255,255,255,0.06)', border: '1px solid rgba(200,255,0,0.3)',
                            color: '#e8eaed', fontFamily: 'inherit', fontSize: 12, resize: 'vertical',
                          }}
                          autoFocus
                        />
                        <div style={{ display: 'flex', gap: 6, justifyContent: 'flex-end' }}>
                          <button
                            style={{ ...ls.iconBtn, fontSize: 11, padding: '4px 10px', color: 'rgba(255,255,255,0.4)' }}
                            onClick={() => { setEditingMsgId(null); setEditingText(''); }}
                          >
                            {t('disc.cancel')}
                          </button>
                          <button
                            style={{ ...ls.scanBtn, fontSize: 11, padding: '4px 10px' }}
                            onClick={handleEditMessage}
                            disabled={!editingText.trim()}
                          >
                            <Send size={10} /> {t('disc.resend')}
                            <span style={{ fontSize: 9, opacity: 0.5, marginLeft: 4 }}>Ctrl+Enter</span>
                          </button>
                        </div>
                      </div>
                    ) : (
                      <MarkdownContent content={msg.content} />
                    )}
                    {/api.?key|invalid.*key|key.*not.*config|authenticat|unauthori|login|sign.?in/i.test(msg.content) && (
                      <div style={{ display: 'flex', gap: 8, marginTop: 8, flexWrap: 'wrap' }}>
                        <button
                          style={{ ...ls.scanBtn, fontSize: 11, padding: '5px 12px' }}
                          onClick={() => onNavigate('settings')}
                        >
                          <Key size={11} /> {t('disc.overrideKey')}
                        </button>
                        <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.25)', alignSelf: 'center' }}>
                          {t('disc.orCheckAgent')}
                        </span>
                      </div>
                    )}
                    <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginTop: 4 }}>
                      <div style={{ ...ds.msgTime, display: 'flex', alignItems: 'center', gap: 6 }}>
                        {new Date(msg.timestamp).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}
                        {msg.tokens_used > 0 && (
                          <span style={{ color: 'rgba(255,255,255,0.2)', fontSize: 9 }}>
                            {msg.tokens_used.toLocaleString()} tok
                          </span>
                        )}
                        {msg.auth_mode && (
                          <span style={{ color: msg.auth_mode === 'override' ? 'rgba(200,255,0,0.3)' : 'rgba(255,255,255,0.15)', fontSize: 9 }}>
                            {msg.auth_mode === 'override' ? 'API key' : 'auth locale'}
                          </span>
                        )}
                      </div>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                        {msg.role === 'Agent' && hasFullAccess(msg.agent_type ?? activeDiscussion.agent) && (
                          <span style={{
                            display: 'inline-flex', alignItems: 'center', gap: 3,
                            fontSize: 9, color: 'rgba(255,200,0,0.5)',
                            padding: '1px 5px', borderRadius: 4,
                            background: 'rgba(255,200,0,0.06)', border: '1px solid rgba(255,200,0,0.1)',
                          }}>
                            <AlertTriangle size={8} />
                            {t('config.fullAccessBadge')}
                          </span>
                        )}
                        {!sending && !isEditing && (isLastUser || isLastAgent) && (
                          <div style={{ display: 'flex', gap: 4 }}>
                            {isLastUser && (
                              <button
                                style={{ ...ls.iconBtn, padding: '2px 6px', fontSize: 10, color: 'rgba(255,255,255,0.3)' }}
                                onClick={() => { setEditingMsgId(msg.id); setEditingText(msg.content); }}
                                title={t('disc.editResend')}
                              >
                                <Pencil size={10} />
                              </button>
                            )}
                            {isLastAgent && (
                              <button
                                style={{ ...ls.iconBtn, padding: '2px 6px', fontSize: 10, color: 'rgba(255,255,255,0.3)' }}
                                onClick={handleRetry}
                                title={t('disc.retryResponse')}
                              >
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
              })}

              {/* Streaming: single agent mode */}
              {sending && !orchState[activeDiscussion.id]?.active && (
                <div style={ds.msgRow(false)}>
                  <div style={ds.msgBubble(false)}>
                    <div style={{ ...ds.msgAgent, color: agentColor(activeDiscussion.agent) }}>
                      <Cpu size={10} /> {activeDiscussion.agent}
                      <Loader2 size={10} style={{ animation: 'spin 1s linear infinite', marginLeft: 4 }} />
                    </div>
                    {streamingText ? (
                      <MarkdownContent content={streamingText} />
                    ) : (
                      <div style={{ fontSize: 12, color: 'rgba(255,255,255,0.3)', fontStyle: 'italic' }}>
                        {t('disc.running')}
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
                            <div style={{ fontSize: 12, color: 'rgba(255,255,255,0.3)', fontStyle: 'italic' }}>
                              {t('disc.thinking', as_.agent)}
                            </div>
                          ) : null}
                        </div>
                      </div>
                    ))}
                  </>
                );
              })()}

              {/* Validation complete banner */}
              {(() => {
                if (activeDiscussion.title !== 'Validation audit AI' || !activeDiscussion.project_id) return null;
                const proj = projects.find(p => p.id === activeDiscussion.project_id);
                if (!proj || proj.audit_status !== 'Audited') return null;
                const lastAgentMsg = [...activeDiscussion.messages].reverse().find(m => m.role === 'Agent');
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
                const lastAgentMsg = [...activeDiscussion.messages].reverse().find(m => m.role === 'Agent');
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

            {/* Input */}
            <div style={{
              ...ds.inputBar,
              ...(activeAgentDisabled ? { opacity: 0.4, pointerEvents: 'none' as const } : {}),
            }}>
              <div style={{ flex: 1, position: 'relative' }}>
                <textarea
                  ref={chatInputRef}
                  style={{ ...ds.chatInput, resize: 'none', minHeight: 42, maxHeight: 160, lineHeight: 1.4 }}
                  rows={1}
                  placeholder={activeDiscussion && (activeDiscussion.participants?.length ?? 0) > 1 && AGENT_MENTIONS.length > 0
                    ? t('disc.mentionHint', AGENT_MENTIONS.map(m => m.trigger).join(', '))
                    : t('disc.messagePlaceholder')}
                  value={chatInput}
                  onChange={e => {
                    const val = e.target.value;
                    setChatInput(val);
                    // Auto-resize textarea
                    e.target.style.height = 'auto';
                    e.target.style.height = Math.min(e.target.scrollHeight, 160) + 'px';
                    // Detect @mention at start
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
                        setChatInput(filtered[mentionIndex].trigger + ' ');
                        setMentionQuery(null);
                        return;
                      }
                      if (e.key === 'Escape') { setMentionQuery(null); return; }
                    }
                    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); handleSendMessage(); }
                  }}
                  disabled={sending || activeAgentDisabled}
                />
                {/* @mention autocomplete dropdown */}
                {mentionQuery !== null && (() => {
                  const filtered = AGENT_MENTIONS.filter(m => m.trigger.slice(1).startsWith(mentionQuery ?? ''));
                  if (filtered.length === 0) return null;
                  return (
                    <div style={{
                      position: 'absolute', bottom: '100%', left: 0, marginBottom: 4,
                      background: '#1a1d26', border: '1px solid rgba(200,255,0,0.2)',
                      borderRadius: 8, overflow: 'hidden', boxShadow: '0 4px 16px rgba(0,0,0,0.4)',
                      minWidth: 180,
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
                            setChatInput(m.trigger + ' ');
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
              </div>
              {/* Debate button */}
              <div style={{ position: 'relative' }}>
                <button
                  style={{
                    ...ds.sendBtn,
                    background: showDebatePopover ? 'rgba(139,92,246,0.2)' : 'rgba(139,92,246,0.08)',
                    border: '1px solid rgba(139,92,246,0.3)',
                    color: '#8b5cf6',
                  }}
                  onClick={() => {
                    if (!showDebatePopover) {
                      setDebateAgents(installedAgents.map(a => a.agent_type));
                    }
                    setShowDebatePopover(!showDebatePopover);
                  }}
                  disabled={sending}
                  title={t('debate.title')}
                >
                  <Users size={16} />
                </button>
                {showDebatePopover && (
                  <div style={{
                    position: 'absolute', bottom: '100%', right: 0, marginBottom: 8,
                    width: 260, padding: 14, borderRadius: 10,
                    background: '#1a1d26', border: '1px solid rgba(139,92,246,0.2)',
                    boxShadow: '0 8px 32px rgba(0,0,0,0.5)',
                  }}>
                    <div style={{ fontSize: 12, fontWeight: 700, color: '#8b5cf6', marginBottom: 10, display: 'flex', alignItems: 'center', gap: 6 }}>
                      <Users size={12} /> {t('debate.header')}
                    </div>
                    <p style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginBottom: 10, lineHeight: 1.4 }}>
                      {t('debate.instructions')}
                    </p>
                    {installedAgents.map(a => {
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
                                  title={skill.name}
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
                                title={directive.name}
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
              {sending ? (
                <button
                  style={{
                    ...ds.sendBtn,
                    background: 'rgba(255,77,106,0.15)',
                    border: '1px solid rgba(255,77,106,0.4)',
                    color: '#ff4d6a',
                  }}
                  onClick={handleStop}
                  title={t('disc.stopThinking')}
                >
                  <StopCircle size={16} />
                </button>
              ) : (
                <button
                  style={ds.sendBtn}
                  onClick={handleSendMessage}
                  disabled={!chatInput.trim()}
                >
                  <Send size={16} />
                </button>
              )}
            </div>
          </>
        ) : !showNewDiscussion && (
          <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', flex: 1, color: 'rgba(255,255,255,0.2)' }}>
            <MessageSquare size={48} style={{ marginBottom: 16, opacity: 0.3 }} />
            <p style={{ fontSize: 14 }}>{t('disc.selectOrCreate')}</p>
          </div>
        )}
      </div>
    </div>
  );
}

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

const MarkdownContent = ({ content }: { content: string }) => (
  <div style={{ fontSize: 13, lineHeight: 1.55 }}>
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      components={{
        p: ({ children }) => <p style={mdStyles.p}>{children}</p>,
        h1: ({ children }) => <h1 style={mdStyles.h1}>{children}</h1>,
        h2: ({ children }) => <h2 style={mdStyles.h2}>{children}</h2>,
        h3: ({ children }) => <h3 style={mdStyles.h3}>{children}</h3>,
        ul: ({ children }) => <ul style={mdStyles.ul}>{children}</ul>,
        ol: ({ children }) => <ol style={mdStyles.ol}>{children}</ol>,
        li: ({ children }) => <li style={mdStyles.li}>{children}</li>,
        code: ({ className, children }) => {
          const isBlock = className?.includes('language-');
          return isBlock
            ? <code style={mdStyles.preCode}>{children}</code>
            : <code style={mdStyles.code}>{children}</code>;
        },
        pre: ({ children }) => <pre style={mdStyles.pre}>{children}</pre>,
        table: ({ children }) => <table style={mdStyles.table}>{children}</table>,
        th: ({ children }) => <th style={mdStyles.th}>{children}</th>,
        td: ({ children }) => <td style={mdStyles.td}>{children}</td>,
        blockquote: ({ children }) => <blockquote style={mdStyles.blockquote}>{children}</blockquote>,
        hr: () => <hr style={mdStyles.hr} />,
        a: ({ href, children }) => <a href={href} style={mdStyles.a} target="_blank" rel="noopener noreferrer">{children}</a>,
        strong: ({ children }) => <strong style={mdStyles.strong}>{children}</strong>,
      }}
    >
      {content}
    </ReactMarkdown>
  </div>
);

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
  msgRow: (isUser: boolean) => ({ display: 'flex', justifyContent: isUser ? 'flex-end' : 'flex-start', marginBottom: 12 }),
  msgBubble: (isUser: boolean) => ({
    maxWidth: '70%', padding: '10px 14px', borderRadius: 12,
    background: isUser ? 'rgba(200,255,0,0.08)' : 'rgba(255,255,255,0.04)',
    border: `1px solid ${isUser ? 'rgba(200,255,0,0.15)' : 'rgba(255,255,255,0.07)'}`,
    color: '#e8eaed',
  }),
  msgAgent: { display: 'flex', alignItems: 'center', gap: 4, fontSize: 10, fontWeight: 600, color: 'rgba(139,92,246,0.7)', marginBottom: 4 } as const,
  msgTime: { fontSize: 10, color: 'rgba(255,255,255,0.2)', marginTop: 4, textAlign: 'right' as const },
  inputBar: { display: 'flex', gap: 8, padding: '12px 20px', borderTop: '1px solid rgba(255,255,255,0.07)', background: '#12151c', flexShrink: 0 } as const,
  chatInput: { width: '100%', padding: '10px 14px', background: 'rgba(255,255,255,0.04)', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit', outline: 'none' } as const,
  sendBtn: { padding: '10px 16px', borderRadius: 8, border: '1px solid rgba(200,255,0,0.2)', background: 'rgba(200,255,0,0.1)', color: '#c8ff00', cursor: 'pointer', display: 'flex', alignItems: 'center', justifyContent: 'center' } as const,
  newDiscOverlay: { display: 'flex', alignItems: 'center', justifyContent: 'center', flex: 1 } as const,
  newDiscCard: { width: 420, maxHeight: 'calc(100vh - 120px)', overflowY: 'auto', padding: 24, borderRadius: 12, background: '#12151c', border: '1px solid rgba(255,255,255,0.1)' } as const,
  label: { display: 'block', fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', marginBottom: 4, marginTop: 8 } as const,
  selectStyled: {
    width: '100%', padding: '9px 12px', background: '#1a1d26', border: '1px solid rgba(255,255,255,0.12)',
    borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit', outline: 'none',
    cursor: 'pointer', appearance: 'none' as const,
    backgroundImage: `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%23888' stroke-width='2'%3E%3Cpolyline points='6 9 12 15 18 9'%3E%3C/polyline%3E%3C/svg%3E")`,
    backgroundRepeat: 'no-repeat', backgroundPosition: 'right 10px center',
    paddingRight: 32,
  } as const,
  inputStyled: {
    width: '100%', padding: '9px 12px', background: '#1a1d26', border: '1px solid rgba(255,255,255,0.12)',
    borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit', outline: 'none',
    boxSizing: 'border-box' as const,
  } as const,
  textareaStyled: {
    width: '100%', padding: '10px 12px', background: '#1a1d26', border: '1px solid rgba(255,255,255,0.12)',
    borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit', outline: 'none',
    resize: 'vertical' as const, boxSizing: 'border-box' as const, lineHeight: 1.5,
  } as const,
};
