import { Fragment, useState, useRef, useEffect, useLayoutEffect, useCallback, useMemo } from 'react';
import '../pages/DiscussionsPage.css';
import type {
  Discussion,
  DiscussionMessage,
  AgentDetection,
  AgentType,
  MessageTarget,
  MessageChannel,
  ParticipantView,
  Skill,
  Directive,
  ContextFile,
  QuickPrompt,
  ModelTier,
  ModelTiersConfig,
} from '../types/generated';
import {
  AGENT_MENTIONS as ALL_AGENT_MENTIONS,
  MODEL_TIER_ICONS,
  agentTextColor,
  isUsable,
  modelForAgentTier,
} from '../lib/constants';
import { audioBufferToFloat32, transcribeAudio } from '../lib/stt-engine';
import {
  loadDraft,
  saveDraft,
  clearDraft,
  type DraftRoutingTiers,
} from '../lib/chat-drafts';
import {
  loadDiscussionRoutingPreferences,
  saveDiscussionRoutingPreferences,
  type DiscussionRoutingPreferences,
} from '../lib/discussionRoutingPreferences';
import { quoteMultilinePaste } from '../lib/quoteMultilinePaste';
import { formatRelativeTime } from '../lib/relativeTime';
import { discussions as discussionsApi, autoTriggersApi, config as configApi } from '../lib/api';
import { detectTriggeredSkills } from '../lib/autoTriggers';
import {
  MESSAGE_SEND_SETTLED_EVENT,
  type MessageSendSettledDetail,
} from '../lib/messageSendLifecycle';
import {
  findEmojiQuery, searchEmojis, applyEmojiReplacement,
  type EmojiQuery, type EmojiSuggestion,
} from '../lib/emoji-autocomplete';
import type { ToastFn } from '../hooks/useToast';
import {
  Send, X, AlertTriangle, Users,
  StopCircle, RotateCcw, Loader2,
  Cpu, Mic, MicOff, Phone, PhoneOff,
  Volume2, VolumeX, Check, Zap, FileText, Paperclip, Image, Reply,
  Eye, EyeOff, StickyNote, Terminal,
} from 'lucide-react';
import { useIsMobile } from '../hooks/useMediaQuery';
import { MarkdownEditor } from './MarkdownComposerTools';
import {
  composerMentions,
  targetsFromComposerText,
} from '../lib/messageTargets';
import { findAgentMentionQuery, type AgentMentionQuery } from '../lib/mention-autocomplete';

const MENTION_TIER_CHOICES: ModelTier[] = ['economy', 'default', 'reasoning'];

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

export interface ChatInputProps {
  discussion: Discussion | null;
  agents: AgentDetection[];
  sending: boolean;
  disabled: boolean;
  ttsEnabled: boolean;
  ttsState: 'idle' | 'loading' | 'playing' | 'paused';
  worktreeError: string | null;
  /** True while a send is refused because the previous run is still recovering. */
  partialPending?: boolean;
  /** KT-251 — id of the answer currently blocking this send, when known.
   *  Shown so the user can name it: "je ne vois pas encore d'id […] ça t'aurait
   *  aidé au debug". Undefined = unknown, never rendered as an empty id. */
  partialPendingMessageId?: string;
  partialForcing?: boolean;
  onPartialPendingForce?: () => void;
  onPartialPendingDismiss?: () => void;
  availableSkills: Skill[];
  availableDirectives: Directive[];
  onSend: (
    text: string,
    targets?: MessageTarget[],
    targetAll?: boolean,
    replyToMessageId?: string,
    channel?: MessageChannel,
  ) => void;
  onStop: () => void;
  onOrchestrate: (agents: AgentType[], rounds: number, skillIds: string[], directiveIds: string[]) => void;
  onTtsToggle: () => void;
  onWorktreeErrorDismiss: () => void;
  onWorktreeRetry: () => void;
  isAgentRestricted: (type: AgentType) => boolean;
  contextFiles?: ContextFile[];
  onUploadFiles?: (files: File[]) => void;
  onDeleteContextFile?: (fileId: string) => void;
  uploadingFiles?: boolean;
  /** QPs without variables — shown in the "chain" picker while sending. */
  chainableQPs?: QuickPrompt[];
  /** Currently queued QP (will auto-fire when the agent finishes). */
  queuedQP?: QuickPrompt | null;
  onQueueQP?: (qp: QuickPrompt) => void;
  onCancelQueuedQP?: () => void;
  replyTarget?: DiscussionMessage | null;
  hasDiscussionNotes?: boolean;
  showDiscussionNotes?: boolean;
  onToggleDiscussionNotes?: () => void;
  onCancelReply?: () => void;
  modelTiers?: ModelTiersConfig | null;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ChatInput({
  discussion,
  agents,
  sending,
  disabled,
  ttsEnabled,
  ttsState,
  worktreeError,
  partialPending = false,
  partialPendingMessageId,
  partialForcing = false,
  onPartialPendingForce,
  onPartialPendingDismiss,
  availableSkills,
  availableDirectives,
  onSend,
  onStop,
  onOrchestrate,
  onTtsToggle,
  onWorktreeErrorDismiss,
  onWorktreeRetry,
  isAgentRestricted,
  contextFiles = [],
  onUploadFiles,
  onDeleteContextFile,
  uploadingFiles = false,
  chainableQPs = [],
  queuedQP = null,
  onQueueQP,
  onCancelQueuedQP,
  replyTarget = null,
  hasDiscussionNotes = false,
  showDiscussionNotes = true,
  onToggleDiscussionNotes,
  onCancelReply,
  modelTiers,
  toast,
  t,
}: ChatInputProps) {
  const isMobile = useIsMobile();

  // ── Auto-trigger opt-out list ────────────────────────────────────────
  // Pulled once on mount + on external toggle events. The `Set` is
  // consumed by `detectTriggeredSkills()` to skip skills the operator
  // has opted out of in Settings > Skills > ⚡ toggle.
  const [disabledAutoSkills, setDisabledAutoSkills] = useState<Set<string>>(new Set());
  useEffect(() => {
    const refetch = () => {
      autoTriggersApi.listDisabled()
        .then(ids => setDisabledAutoSkills(new Set(ids)))
        .catch(e => console.warn('fetch disabled auto-skills failed:', e));
    };
    refetch();
    window.addEventListener('kronn:auto-trigger-changed', refetch);
    return () => window.removeEventListener('kronn:auto-trigger-changed', refetch);
  }, []);

  // ─── Internal state ──────────────────────────────────────────────────────
  const [chatInput, setChatInput] = useState('');
  const [discussionNotesEnabled, setDiscussionNotesEnabled] = useState(false);
  const [sendAsNote, setSendAsNote] = useState(false);
  const [cliParticipants, setCliParticipants] = useState<ParticipantView[]>([]);
  const [nativeAgentDisabled, setNativeAgentDisabled] = useState<boolean | null>(null);
  const chatInputValueRef = useRef('');
  const chatInputHasText = chatInput.trim().length > 0;
  const chatInputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    configApi.getServerConfig()
      .then(serverConfig => setDiscussionNotesEnabled(serverConfig.discussion_notes_enabled ?? true))
      .catch(error => console.warn('fetch discussion note setting failed:', error));
  }, []);

  useEffect(() => {
    setSendAsNote(false);
  }, [discussion?.id]);
  const replyAuthor = useMemo(() => {
    if (!replyTarget) return '';
    if (replyTarget.agent_type) {
      return ALL_AGENT_MENTIONS.find(mention => mention.type === replyTarget.agent_type)?.trigger
        ?? replyTarget.agent_type;
    }
    return replyTarget.author_pseudo || t('disc.humanAuthor');
  }, [replyTarget, t]);
  const replyExcerpt = useMemo(
    () => replyTarget?.content.replace(/\s+/g, ' ').trim().slice(0, 180) ?? '',
    [replyTarget],
  );

  useEffect(() => {
    if (replyTarget) chatInputRef.current?.focus();
  }, [replyTarget]);
  const discussionLanguage = discussion?.language;

  useEffect(() => {
    let current = true;
    const load = () => {
      if (!discussion?.id) {
        setCliParticipants([]);
        setNativeAgentDisabled(null);
        return;
      }
      void discussionsApi.participants(discussion.id)
        .then(participants => {
          if (current) setCliParticipants(participants as ParticipantView[]);
        })
        .catch(error => console.warn('[ChatInput] participants fetch failed:', error));
      void discussionsApi.nativeAgentMode(discussion.id)
        .then(mode => {
          if (current) setNativeAgentDisabled(mode.disabled);
        })
        .catch(error => console.warn('[ChatInput] native agent mode fetch failed:', error));
    };
    load();
    const interval = setInterval(load, 5_000);
    return () => {
      current = false;
      clearInterval(interval);
    };
  }, [discussion?.id]);

  const updateChatInput = useCallback((val: string) => {
    chatInputValueRef.current = val;
    setChatInput(val);
    if (chatInputRef.current) {
      chatInputRef.current.value = val;
      // Re-snap height to the content. Pre-fix the textarea kept the
      // multi-line height it had grown to during typing — after a send
      // (`updateChatInput('')`) the empty composer stayed 4-5 lines tall
      // until the user clicked it again. The same recompute is used by
      // the onChange handler, so behaviour is consistent for typed,
      // pasted and programmatic value changes.
      chatInputRef.current.style.height = 'auto';
      chatInputRef.current.style.height = Math.min(chatInputRef.current.scrollHeight, 160) + 'px';
    }
  }, []);

  // ─── Draft persistence (per-discussion) ─────────────────────────────────
  // Saved to localStorage so the textarea survives tab/page navigation.
  // The textarea is non-controlled (chatInputRef) for perf — this hook
  // rehydrates its `value` on discussion change, saves throttled on change,
  // and clears on successful send.
  const [restoredDraftAt, setRestoredDraftAt] = useState<string | null>(null);
  const draftSaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const currentDiscIdRef = useRef<string | null>(null);
  const [mentionTierOverrides, setMentionTierOverrides] = useState<DraftRoutingTiers>({});
  const mentionTierOverridesRef = useRef<DraftRoutingTiers>({});
  const [preferredTiers, setPreferredTiers] = useState<DiscussionRoutingPreferences>({});
  const preferredTiersRef = useRef<DiscussionRoutingPreferences>({});
  const submittedRoutingTiersRef = useRef<Record<string, DraftRoutingTiers>>({});

  const updateMentionTierOverrides = useCallback((next: DraftRoutingTiers) => {
    mentionTierOverridesRef.current = next;
    setMentionTierOverrides(next);
  }, []);

  const updatePreferredTiers = useCallback((next: DiscussionRoutingPreferences) => {
    preferredTiersRef.current = next;
    setPreferredTiers(next);
    const discId = currentDiscIdRef.current;
    if (discId) saveDiscussionRoutingPreferences(discId, next);
  }, []);

  const scheduleDraftSave = useCallback((text: string) => {
    const discId = currentDiscIdRef.current;
    if (!discId) return;
    if (draftSaveTimerRef.current) clearTimeout(draftSaveTimerRef.current);
    // 250ms debounce — fast enough to survive a "type-and-tab-away" gesture
    // but sparse enough to never hammer localStorage on long messages.
    draftSaveTimerRef.current = setTimeout(() => {
      saveDraft(discId, text, mentionTierOverridesRef.current);
    }, 250);
  }, []);

  // KT-453/457 — "Talk about it in the discussion" from the Git diff viewer
  // prefills the draft with a file/range/SHA quote. Never overwrite an
  // in-progress draft: append below it, same as a normal paste would. Must
  // go through the same persistence contract as typed input — a
  // programmatic prefill the user never touches again would otherwise
  // vanish on reload instead of surviving like every other draft.
  useEffect(() => {
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<{ discussionId: string; text: string }>).detail;
      if (!detail?.text || detail.discussionId !== discussion?.id) return;
      const existing = chatInputValueRef.current;
      const next = existing.trim().length > 0 ? `${existing}\n\n${detail.text}` : detail.text;
      updateChatInput(next);
      scheduleDraftSave(next);
      chatInputRef.current?.focus();
      const end = next.length;
      chatInputRef.current?.setSelectionRange(end, end);
    };
    window.addEventListener('kronn:composer-prefill', handler);
    return () => window.removeEventListener('kronn:composer-prefill', handler);
  }, [discussion?.id, updateChatInput, scheduleDraftSave]);

  const flushDraftNow = useCallback((discId: string, text: string) => {
    if (draftSaveTimerRef.current) {
      clearTimeout(draftSaveTimerRef.current);
      draftSaveTimerRef.current = null;
    }
    saveDraft(discId, text, mentionTierOverridesRef.current);
  }, []);

  // On discussion switch: flush the previous discussion's draft (without
  // waiting for the debounce), then rehydrate the textarea for the new one.
  useEffect(() => {
    const prevDiscId = currentDiscIdRef.current;
    const nextDiscId = discussion?.id ?? null;

    // Flush any pending save for the previous discussion so switching away
    // quickly doesn't lose the last keystroke.
    if (prevDiscId && prevDiscId !== nextDiscId) {
      flushDraftNow(prevDiscId, chatInputValueRef.current);
    }

    currentDiscIdRef.current = nextDiscId;

    if (!nextDiscId) {
      // No discussion selected → clear textarea state.
      updateChatInput('');
      updateMentionTierOverrides({});
      preferredTiersRef.current = {};
      setPreferredTiers({});
      setRestoredDraftAt(null);
      return;
    }

    const rememberedTiers = loadDiscussionRoutingPreferences(nextDiscId);
    preferredTiersRef.current = rememberedTiers;
    setPreferredTiers(rememberedTiers);

    const saved = loadDraft(nextDiscId);
    if (saved) {
      updateChatInput(saved.text);
      updateMentionTierOverrides(saved.routingTiers);
      setRestoredDraftAt(saved.savedAt);
    } else {
      updateChatInput('');
      updateMentionTierOverrides({});
      setRestoredDraftAt(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [discussion?.id]);

  // Flush the pending debounce on unmount so navigation away (e.g. tab
  // change) doesn't drop the last 250 ms of typing.
  useEffect(() => {
    return () => {
      const discId = currentDiscIdRef.current;
      if (discId && draftSaveTimerRef.current) {
        clearTimeout(draftSaveTimerRef.current);
        draftSaveTimerRef.current = null;
        saveDraft(discId, chatInputValueRef.current, mentionTierOverridesRef.current);
      }
    };
  }, []);

  // A send is only durable once the backend emits its `accepted` receipt.
  // Keep the submitted text in localStorage while the optimistic request is
  // in flight, then either clear that snapshot on acceptance or restore it on
  // a pre-receipt failure (502, backend restart, network loss). If the user
  // already started typing the next message, preserve BOTH texts instead of
  // overwriting the newer draft.
  useEffect(() => {
    const onSendSettled = (rawEvent: Event) => {
      const { detail } = rawEvent as CustomEvent<MessageSendSettledDetail>;
      if (!detail) return;
      if (detail.discussionId !== currentDiscIdRef.current) {
        // The user switched rooms while the request was in flight. The
        // submitted snapshot belongs to the previous room: remove it only
        // after durable acceptance; on refusal leave it stored so returning
        // to that room restores the unsent message.
        if (detail.settlement === 'accepted') clearDraft(detail.discussionId);
        return;
      }

      const current = chatInputValueRef.current;
      const submittedTiers = submittedRoutingTiersRef.current[detail.discussionId] ?? {};
      if (detail.settlement === 'accepted') {
        delete submittedRoutingTiersRef.current[detail.discussionId];
        if (current.trim()) {
          flushDraftNow(detail.discussionId, current);
        } else {
          clearDraft(detail.discussionId);
          if (currentDiscIdRef.current === detail.discussionId) {
            updateMentionTierOverrides({});
          }
        }
        return;
      }

      const restored = !current.trim()
        ? detail.message
        : current === detail.message
          ? current
          : `${detail.message}\n\n${current}`;
      updateChatInput(restored);
      const restoredTiers = { ...submittedTiers, ...mentionTierOverridesRef.current };
      updateMentionTierOverrides(restoredTiers);
      flushDraftNow(detail.discussionId, restored);
      setRestoredDraftAt(new Date().toISOString());
      requestAnimationFrame(() => {
        chatInputRef.current?.focus();
        if (chatInputRef.current) {
          const end = chatInputRef.current.value.length;
          chatInputRef.current.setSelectionRange(end, end);
        }
      });
    };

    window.addEventListener(MESSAGE_SEND_SETTLED_EVENT, onSendSettled);
    return () => window.removeEventListener(MESSAGE_SEND_SETTLED_EVENT, onSendSettled);
  }, [flushDraftNow, updateChatInput, updateMentionTierOverrides]);

  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const [mentionIndex, setMentionIndex] = useState(0);
  // Keyboard tier pick inside the mention palette. `null` means UNTOUCHED, and
  // that distinction is load-bearing: Tab/Enter must keep sending no tier at all
  // (no override) unless the user actually pressed Left/Right, otherwise every
  // mention would silently start carrying an explicit tier override.
  const [mentionTierIndex, setMentionTierIndex] = useState<number | null>(null);
  // Set when keydown CONSUMED a Left/Right for tier picking. `onKeyUp` refreshes
  // the mention query on Left/Right (they normally move the caret out of the
  // mention) and that refresh resets the highlighted row to 0 — which threw the
  // selection back to the top of the list on every tier keypress. keyup cannot
  // see keydown's preventDefault, so the fact has to be carried explicitly.
  const tierKeyConsumedRef = useRef(false);
  const mentionRangeRef = useRef<Pick<AgentMentionQuery, 'start' | 'end'> | null>(null);

  const refreshMentionQuery = useCallback((text: string, cursorPos: number) => {
    const found = findAgentMentionQuery(text, cursorPos);
    mentionRangeRef.current = found
      ? { start: found.start, end: found.end }
      : null;
    setMentionQuery(found?.query ?? null);
    if (found) setMentionIndex(0);
  }, []);

  const applyMentionSuggestion = useCallback((
    trigger: string,
    agentType?: AgentType,
    tier?: ModelTier,
  ) => {
    const range = mentionRangeRef.current;
    if (!range) return;
    const current = chatInputValueRef.current;
    const trailing = current.slice(range.end);
    const spacer = trailing.length === 0 || !/^\s/.test(trailing) ? ' ' : '';
    const next = `${current.slice(0, range.start)}${trigger}${spacer}${trailing}`;
    const cursor = range.start + trigger.length + spacer.length;
    updateChatInput(next);
    if (agentType) {
      const nextTiers = { ...mentionTierOverridesRef.current };
      const effectiveTier = tier ?? preferredTiersRef.current[agentType];
      if (effectiveTier) nextTiers[agentType] = effectiveTier;
      else delete nextTiers[agentType];
      updateMentionTierOverrides(nextTiers);
      if (tier) {
        updatePreferredTiers({
          ...preferredTiersRef.current,
          [agentType]: tier,
        });
      }
    }
    scheduleDraftSave(next);
    setMentionQuery(null);
    mentionRangeRef.current = null;
    requestAnimationFrame(() => {
      chatInputRef.current?.focus();
      chatInputRef.current?.setSelectionRange(cursor, cursor);
    });
  }, [scheduleDraftSave, updateChatInput, updateMentionTierOverrides, updatePreferredTiers]);

  // ─── Emoji shortcode autocomplete (:tada: → 🎉) ──────────────────────────
  // Clones the @mention plumbing below but matches `:word` anywhere in the
  // textarea, not just at the start. The match is computed on every edit
  // from (text, cursorPos); the resulting `EmojiQuery` is stored here with
  // its fresh suggestion list so render + keyboard handlers read from the
  // same snapshot (otherwise Tab/Enter could fire against a stale list).
  const [emojiMatch, setEmojiMatch] = useState<EmojiQuery | null>(null);
  const [emojiSuggestions, setEmojiSuggestions] = useState<EmojiSuggestion[]>([]);
  const [emojiIndex, setEmojiIndex] = useState(0);

  /** Recompute emoji suggestions from the current textarea state. Called
   *  from the textarea onChange and onKeyUp so caret-only moves (arrow
   *  keys inside the text) still refresh the popover correctly. */
  const refreshEmojiQuery = useCallback((text: string, cursorPos: number) => {
    const found = findEmojiQuery(text, cursorPos);
    if (!found) {
      setEmojiMatch(null);
      setEmojiSuggestions([]);
      return;
    }
    const suggestions = searchEmojis(found.query);
    if (suggestions.length === 0) {
      setEmojiMatch(null);
      setEmojiSuggestions([]);
      return;
    }
    setEmojiMatch(found);
    setEmojiSuggestions(suggestions);
    setEmojiIndex(0);
  }, []);

  /** Insert the picked shortcode, update the DOM textarea, restore caret. */
  const applyEmojiSuggestion = useCallback((suggestion: EmojiSuggestion) => {
    const ta = chatInputRef.current;
    const match = emojiMatch;
    if (!ta || !match) return;
    // Insert the Unicode glyph directly (Discord/Slack UX) — cleaner than
    // showing `:tada:` in the textarea and letting the user guess whether
    // it will render. `remark-emoji` still handles the reverse direction
    // for agent output that uses the `:shortcode:` form.
    const { text: next, cursor } = applyEmojiReplacement(
      chatInputValueRef.current,
      match,
      suggestion.emoji,
    );
    updateChatInput(next);
    // Restore caret right after the inserted ":shortcode: " (on the next
    // frame so React has flushed the DOM value).
    requestAnimationFrame(() => {
      if (chatInputRef.current) {
        chatInputRef.current.selectionStart = cursor;
        chatInputRef.current.selectionEnd = cursor;
      }
    });
    setEmojiMatch(null);
    setEmojiSuggestions([]);
    scheduleDraftSave(next);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [emojiMatch, updateChatInput]);

  const [dragOver, setDragOver] = useState(false);

  // 0.8.6 fix — 'loading' surfaces the worker's first-time model
  // download (~40MB from HF). Pre-fix the user saw a blank "transcribing"
  // banner during the 30s-2min download, with no indication of progress
  // vs a silent failure.
  const [sttState, setSttState] = useState<'idle' | 'recording' | 'loading' | 'transcribing'>('idle');
  const [voiceMode, setVoiceMode] = useState(false);
  const [voiceCountdown, setVoiceCountdown] = useState<number | null>(null);
  const voiceCountdownRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const voiceAutoSendRef = useRef(false);
  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const audioChunksRef = useRef<Blob[]>([]);
  const sttCancelledRef = useRef(false);

  const [showDebatePopover, setShowDebatePopover] = useState(false);
  const [showQPPicker, setShowQPPicker] = useState(false);
  const [debateAgents, setDebateAgents] = useState<AgentType[]>([]);
  const [debateRounds, setDebateRounds] = useState(2);
  const [debateSkillIds, setDebateSkillIds] = useState<string[]>(['token-saver', 'devils-advocate']);
  const [debateDirectiveIds, setDebateDirectiveIds] = useState<string[]>([]);

  // Esc-to-close on the debate popover. Pre-fix the popover had no
  // dismiss handler — once open the user was stuck until they clicked
  // the Users icon again (Tya's audit, 2026-05-09). Mirrors the same
  // pattern used by ChatHeader's badge popover.
  useEffect(() => {
    if (!showDebatePopover) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setShowDebatePopover(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [showDebatePopover]);

  const handleSendMessageRef = useRef<(() => void) | null>(null);

  // ─── Derived data ────────────────────────────────────────────────────────
  const installedAgentsList = useMemo(() => agents.filter(isUsable), [agents]);

  const AGENT_MENTIONS = useMemo(() => {
    if (!discussion) return [];
    return composerMentions(
      discussion.agent,
      installedAgentsList.map(agent => agent.agent_type),
      cliParticipants,
      {
        discussionAgent: t('disc.targetDiscussionAgent'),
        punctualAgent: t('disc.targetPunctualAgent'),
        cli: t('disc.targetCli'),
        all: t('disc.targetAll'),
      },
    );
  }, [discussion, installedAgentsList, cliParticipants, t]);
  const MENTION_OPTIONS = useMemo(() => {
    if (!discussion) return [];
    const active: typeof AGENT_MENTIONS = [];
    const available: typeof AGENT_MENTIONS = [];
    for (const mention of AGENT_MENTIONS) {
      if (
        mention.target?.kind === 'discussion_agent'
        && nativeAgentDisabled !== false
      ) {
        continue;
      }
      const isActive = mention.targetAll
        || mention.target?.kind === 'discussion_agent'
        || mention.target?.kind === 'cli'
        || (
          mention.target?.kind === 'agent'
          && discussion.participants.includes(mention.target.agent_type)
        );
      (isActive ? active : available).push(mention);
    }
    const activeRank = (mention: (typeof AGENT_MENTIONS)[number]) => {
      if (mention.target?.kind === 'discussion_agent') return 0;
      if (mention.target?.kind === 'cli') return 1;
      if (mention.target?.kind === 'agent') return 2;
      return 3;
    };
    active.sort((left, right) => activeRank(left) - activeRank(right));
    return [
      ...active.map(mention => ({ mention, group: 'active' as const })),
      ...available.map(mention => ({ mention, group: 'available' as const })),
    ];
  }, [discussion, AGENT_MENTIONS, nativeAgentDisabled]);
  const DISABLED_MENTION_OPTIONS = useMemo(
    () => nativeAgentDisabled
      ? AGENT_MENTIONS.filter(mention => mention.target?.kind === 'discussion_agent')
      : [],
    [AGENT_MENTIONS, nativeAgentDisabled],
  );
  const filteredMentionOptions = useMemo(
    () => mentionQuery === null
      ? []
      : MENTION_OPTIONS.filter(
        ({ mention }) => mention.trigger.slice(1).startsWith(mentionQuery),
      ),
    [mentionQuery, MENTION_OPTIONS],
  );
  const filteredDisabledMentionOptions = useMemo(
    () => mentionQuery === null
      ? []
      : DISABLED_MENTION_OPTIONS.filter(
        mention => mention.trigger.slice(1).startsWith(mentionQuery),
      ),
    [mentionQuery, DISABLED_MENTION_OPTIONS],
  );
  const mentionRoutingMode = useCallback((mention: (typeof AGENT_MENTIONS)[number]) => {
    const target = mention.target;
    if (!target || !discussion) return null;
    if (target.kind === 'cli') {
      return {
        icon: null,
        title: t('disc.routingCliModelManaged'),
      };
    }
    const selectedTier = mentionTierOverrides[target.agent_type]
      ?? preferredTiers[target.agent_type];
    const isPrincipal = target.kind === 'discussion_agent';
    const tier = selectedTier
      ?? (isPrincipal ? discussion.tier : target.tier ?? 'default');
    const model = isPrincipal && !selectedTier && discussion.model?.trim()
      ? discussion.model.trim()
      : modelForAgentTier(
        target.agent_type,
        tier,
        modelTiers,
        t('disc.defaultAgentModel'),
      );
    return {
      icon: MODEL_TIER_ICONS[tier],
      title: t(
        isPrincipal && !selectedTier ? 'disc.routingNativeTier' : 'disc.routingTargetTier',
        t(`disc.tier.${tier}`),
        model,
      ),
      tier,
    };
  }, [discussion, mentionTierOverrides, modelTiers, preferredTiers, t]);
  const pruneMentionTierOverrides = useCallback((text: string) => {
    const activeAgents = new Set(
      targetsFromComposerText(text, AGENT_MENTIONS).targets
        .filter(target => target.kind !== 'cli')
        .map(target => target.agent_type),
    );
    const next = Object.fromEntries(
      Object.entries(mentionTierOverridesRef.current)
        .filter(([agent]) => activeAgents.has(agent as AgentType)),
    ) as DraftRoutingTiers;
    if (Object.keys(next).length !== Object.keys(mentionTierOverridesRef.current).length) {
      updateMentionTierOverrides(next);
    }
  }, [AGENT_MENTIONS, updateMentionTierOverrides]);
  const mentionTierChoiceTitle = useCallback((agent: AgentType, tier: ModelTier) => (
    t(
      'disc.routingInvokeTier',
      t(`disc.tier.${tier}`),
      modelForAgentTier(agent, tier, modelTiers, t('disc.defaultAgentModel')),
    )
  ), [modelTiers, t]);
  const routingHelp = useMemo(() => {
    if (!discussion) {
      return {
        discussionAgent: null,
        activePunctualAgents: [],
        availablePunctualAgents: [],
        cliSessions: [],
        allParticipants: '',
      };
    }

    const configuredMention = ALL_AGENT_MENTIONS.find(
      mention => mention.type === discussion.agent,
    );
    const configuredTrigger = configuredMention?.trigger ?? '@agent';
    const configuredUsable = nativeAgentDisabled === false
      && installedAgentsList.some(agent => agent.agent_type === discussion.agent);
    const activePunctualAgents = AGENT_MENTIONS.filter(mention => {
      const target = mention.target;
      return target?.kind === 'agent'
        && discussion.participants.includes(target.agent_type);
    });
    const availablePunctualAgents = AGENT_MENTIONS.filter(mention => {
      const target = mention.target;
      return target?.kind === 'agent'
        && !discussion.participants.includes(target.agent_type);
    });
    const cliSessions = AGENT_MENTIONS.filter(
      mention => mention.target?.kind === 'cli',
    );
    const seenPunctual = new Set<AgentType>();
    const allParticipants = [
      ...(configuredUsable
        ? [`${configuredTrigger} · ${t('disc.targetDiscussionAgent')}`]
        : []),
      ...discussion.participants.flatMap(agentType => {
        if (agentType === discussion.agent || seenPunctual.has(agentType)) return [];
        seenPunctual.add(agentType);
        const mention = ALL_AGENT_MENTIONS.find(candidate => candidate.type === agentType);
        return mention
          ? [`${mention.trigger} · ${t('disc.targetPunctualAgent')}`]
          : [];
      }),
      ...cliSessions.map(mention => `${mention.trigger} · ${mention.label}`),
    ];

    return {
      discussionAgent: {
        trigger: configuredTrigger,
        usable: configuredUsable,
      },
      activePunctualAgents,
      availablePunctualAgents,
      cliSessions,
      allParticipants: allParticipants.join(', '),
    };
  }, [discussion, installedAgentsList, AGENT_MENTIONS, nativeAgentDisabled, t]);

  // ─── Send handler ────────────────────────────────────────────────────────
  // Closure-stale guard : the `sending` prop is updated by the parent on
  // the `false→true` edge, but two synchronous clicks fire BEFORE React
  // re-renders, so the second click still sees `sending=false`. The ref
  // below (set+cleared in the same tick) catches that race the way
  // `feedback_race_guards.md` documents — `disabled={sending}` is not
  // enough by itself, and double-POST on send is the highest-blast bug
  // in the chat path.
  const sendInFlightRef = useRef(false);
  const handleSendMessage = useCallback(async () => {
    const inputVal = chatInputValueRef.current;
    // NOTE: `sending` is intentionally NOT a guard here. Submitting mid-stream
    // is allowed — the parent (handleSendMessage) routes it to the message
    // QUEUE instead of dropping it (CLI-style). `sendInFlightRef` still blocks
    // a same-tick double-fire of the SAME keystroke.
    if (!discussion || !inputVal.trim() || sendInFlightRef.current) return;
    sendInFlightRef.current = true;
    const msg = inputVal.trim();
    const channel: MessageChannel = sendAsNote ? 'note' : 'main';
    const parsedTargets = channel === 'main'
      ? targetsFromComposerText(msg, AGENT_MENTIONS)
      : { targets: [], targetAll: false };
    // Empty targets intentionally let the backend route to the configured
    // discussion agent. Historical punctual participants must never become an
    // implicit fan-out: only an explicit mention or @all can address them.
    const targets = parsedTargets.targets.map(target => {
      if (target.kind === 'cli') return target;
      const tier = mentionTierOverridesRef.current[target.agent_type]
        ?? preferredTiersRef.current[target.agent_type];
      return tier ? { ...target, tier } : target;
    });
    const targetAll = parsedTargets.targetAll;

    // ── Auto-trigger skills based on message keywords ──
    // Every skill can declare regex triggers in its frontmatter
    // (see `backend/src/skills/kronn-docs.md`). If the pending
    // message matches a trigger for a skill that's not yet active,
    // we add it to `discussion.skill_ids` BEFORE firing onSend so
    // the backend picks it up on the same turn. Non-blocking: if
    // the update fails we still send the message (better to lose
    // the auto-activation than the whole message).
    const locale = discussion.language ?? 'fr';
    const triggered = channel === 'main'
      ? detectTriggeredSkills(
          msg,
          availableSkills,
          discussion.skill_ids ?? [],
          locale,
          disabledAutoSkills,
        )
      : [];
    if (triggered.length > 0) {
      const nextSkillIds = [
        ...(discussion.skill_ids ?? []),
        ...triggered.map(s => s.id),
      ];
      try {
        await discussionsApi.update(discussion.id, { skill_ids: nextSkillIds });
        for (const s of triggered) {
          toast(t('skills.autoActivated', s.name), 'info');
        }
        // Let the rest of the UI (sidebar, header chips) refetch the
        // discussion so the new skill_ids show up immediately.
        window.dispatchEvent(new CustomEvent('kronn:discussion-updated'));
      } catch (e) {
        console.warn('auto-activate skills failed:', e);
        // Non-blocking by design (the message still sends), but the user must
        // know the agent runs WITHOUT the skill(s) they believe are active.
        toast(t('skills.autoActivateFailed', triggered.map(s => s.name).join(', ')), 'warning');
      }
    }

    // Visually clear the composer immediately, but retain the submitted text
    // as a durable draft until DiscussionsPage receives the backend's
    // `accepted` receipt. A failure before that receipt restores this exact
    // snapshot instead of losing the user's message.
    submittedRoutingTiersRef.current = {
      ...submittedRoutingTiersRef.current,
      [discussion.id]: { ...mentionTierOverridesRef.current },
    };
    flushDraftNow(discussion.id, msg);
    setRestoredDraftAt(null);
    updateChatInput('');
    updateMentionTierOverrides({});
    setMentionQuery(null);
    try {
      if (channel === 'note') {
        onSend(msg, undefined, false, replyTarget?.id, channel);
      } else {
        onSend(
          msg,
          targets.length > 0 ? targets : undefined,
          targetAll,
          replyTarget?.id,
        );
      }
      setSendAsNote(false);
    } finally {
      // Release the synchronous re-entry guard ONE microtask later — by
      // then either the parent has flipped `sending=true` (the prop-based
      // guard takes over for the in-flight duration) OR onSend threw
      // synchronously without flipping it (the user must be able to
      // retry, so the ref must not stay stuck).
      queueMicrotask(() => { sendInFlightRef.current = false; });
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [discussion, sending, onSend, updateChatInput, AGENT_MENTIONS, availableSkills, toast, t, disabledAutoSkills, replyTarget, sendAsNote]);

  useLayoutEffect(() => {
    handleSendMessageRef.current = handleSendMessage;
  }, [handleSendMessage]);

  // ─── Keyboard shortcuts during recording ─────────────────────────────────
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
  }, [sttState, voiceMode]);

  // ─── Mic toggle ──────────────────────────────────────────────────────────
  const handleMicToggle = useCallback(async () => {
    if (sttState === 'recording') {
      mediaRecorderRef.current?.stop();
      return;
    }
    if (sttState === 'transcribing' || sttState === 'loading') return;

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
        stream.getTracks().forEach(t => t.stop());

        if (sttCancelledRef.current) {
          sttCancelledRef.current = false;
          audioChunksRef.current = [];
          setSttState('idle');
          return;
        }

        setSttState('transcribing');

        try {
          const blob = new Blob(audioChunksRef.current, { type: 'audio/webm' });
          // 0.8.6 fix — guard against empty audio (recorder didn't fire
          // ondataavailable, very fast click-record-stop, etc.). Without
          // this, decodeAudioData throws a cryptic error and the toast
          // misleads the user about what happened.
          if (blob.size === 0) {
            toast(t('disc.sttEmptyAudio'), 'error');
            setSttState('idle');
            return;
          }
          const arrayBuf = await blob.arrayBuffer();
          const audioCtx = new AudioContext({ sampleRate: 16000 });
          let decoded;
          try {
            decoded = await audioCtx.decodeAudioData(arrayBuf);
          } finally {
            await audioCtx.close();
          }
          const float32 = audioBufferToFloat32(decoded);

          const lang = discussionLanguage || 'fr';
          // 0.8.6 fix — wire the worker's `status` messages so the UI
          // can show "Téléchargement du modèle…" on the first call
          // (Whisper-tiny is ~40MB from HF, takes 30s-2min). Pre-fix
          // the user saw a silent transcribing banner for up to 2 min
          // before either getting text or hitting the 120s timeout.
          const text = await transcribeAudio(getSttWorker(), float32, lang, {
            onStatus: (status) => {
              if (status === 'loading') setSttState('loading');
              else if (status === 'transcribing') setSttState('transcribing');
            },
          });

          const cleaned = text.trim();
          if (cleaned) {
            if (voiceMode) {
              voiceAutoSendRef.current = true;
            }
            // 0.8.6 fix — trim the existing-input trailing space before
            // concatenation so we don't end up with double-spaces. Also
            // ensure a single separator between old and new text.
            const prev = chatInputValueRef.current.trimEnd();
            updateChatInput(prev ? `${prev} ${cleaned}` : cleaned);
            setTimeout(() => {
              if (chatInputRef.current) {
                chatInputRef.current.focus();
                chatInputRef.current.style.height = 'auto';
                chatInputRef.current.style.height = Math.min(chatInputRef.current.scrollHeight, 160) + 'px';
              }
            }, 0);
          } else {
            // 0.8.6 fix — silent empty-text was a confusing UX (user
            // recorded, transcribed, and saw nothing happen). Surface
            // that no speech was detected.
            toast(t('disc.sttNoSpeech'), 'error');
          }
        } catch (err) {
          // 0.8.6 fix — pre-fix this was console.error only ; the user
          // got no UI signal that anything went wrong. Now we toast the
          // error so they can either retry or report it.
          console.error('STT transcription failed:', err);
          const msg = err instanceof Error ? err.message : String(err);
          toast(t('disc.sttFailed', msg), 'error');
        }
        setSttState('idle');
      };

      recorder.start();
      setSttState('recording');
    } catch (err) {
      console.error('Microphone access denied:', err);
      setSttState('idle');
    }
  }, [sttState, discussionLanguage, voiceMode, updateChatInput, toast, t]);

  // ─── Voice mode effects ──────────────────────────────────────────────────

  // Voice mode: auto-send after STT transcription fills chatInput
  useEffect(() => {
    if (voiceAutoSendRef.current && chatInput.trim() && sttState === 'idle' && !sending) {
      voiceAutoSendRef.current = false;
      setTimeout(() => handleSendMessageRef.current?.(), 0);
    }
  }, [chatInput, sttState, sending]);

  // Voice mode: after TTS finishes reading agent response → start countdown → auto-record
  const prevTtsStateRef = useRef(ttsState);
  useEffect(() => {
    const wasPlaying = prevTtsStateRef.current === 'playing' || prevTtsStateRef.current === 'loading';
    prevTtsStateRef.current = ttsState;

    if (!wasPlaying || ttsState !== 'idle') return;
    if (!voiceMode || sending || sttState !== 'idle') return;
    if (voiceCountdown !== null) return;

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
    // Cleanup on unmount or when deps change before the countdown
    // expires — without this, leaving the discussion mid-countdown left
    // a dangling 1 Hz interval setting state on an unmounted component
    // (silent in React 18 but still leaks the timer + closure).
    return () => {
      clearInterval(interval);
      if (voiceCountdownRef.current === interval) {
        voiceCountdownRef.current = null;
      }
    };
  }, [voiceMode, ttsState, sending, sttState, voiceCountdown]);

  // When countdown reaches null (finished) → start recording
  const prevCountdownRef = useRef<number | null>(null);
  useEffect(() => {
    if (prevCountdownRef.current !== null && prevCountdownRef.current > 0 && voiceCountdown === null && voiceMode) {
      handleMicToggle();
    }
    prevCountdownRef.current = voiceCountdown;
  }, [voiceCountdown, voiceMode, handleMicToggle]);

  // Cancel countdown when voice mode is turned off
  useEffect(() => {
    if (!voiceMode) {
      if (voiceCountdownRef.current) { clearInterval(voiceCountdownRef.current); voiceCountdownRef.current = null; }
      setVoiceCountdown(null);
    }
  }, [voiceMode]);

  // Reset voice state when discussion changes
  useEffect(() => {
    if (voiceCountdownRef.current) { clearInterval(voiceCountdownRef.current); voiceCountdownRef.current = null; }
    setVoiceCountdown(null);
    setVoiceMode(false);
  }, [discussion?.id]);

  // ─── Orchestrate handler ─────────────────────────────────────────────────
  const handleOrchestrate = () => {
    if (!discussion || debateAgents.length < 2) return;
    setShowDebatePopover(false);
    onOrchestrate(debateAgents, debateRounds, debateSkillIds, debateDirectiveIds);
  };

  // Chain-QP picker — shared by both composer states. While the agent
  // streams, `onPick` queues the QP (auto-fires after the turn). When idle,
  // `onPick` launches it now by sending its prompt. Same markup either way.
  const qpChainPicker = (onPick: (qp: QuickPrompt) => void) => (
    <div className="relative">
      <button
        type="button"
        className="disc-chain-qp-btn"
        onClick={() => setShowQPPicker(prev => !prev)}
        title={t('disc.chainQP')}
        aria-label={t('disc.chainQP')}
      >
        <Zap size={13} />
      </button>
      {showQPPicker && (
        <div className="disc-qp-picker" role="menu">
          <div className="disc-qp-picker-header">{t('disc.chainQP')}</div>
          {chainableQPs.map(qp => (
            <button
              key={qp.id}
              type="button"
              role="menuitem"
              className="disc-qp-picker-item"
              onMouseDown={e => {
                e.preventDefault();
                onPick(qp);
                setShowQPPicker(false);
              }}
            >
              <span className="disc-qp-picker-icon">{qp.icon}</span>
              <span className="disc-qp-picker-meta">
                <span className="disc-qp-picker-name">{qp.name}</span>
                {qp.description && (
                  <span className="disc-qp-picker-desc">{qp.description}</span>
                )}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );

  const composerHelpContent = (
    <div className="disc-routing-help-content">
      <div className="disc-composer-help-section-heading">@ {t('disc.routingHelpTitle')}</div>
      <p>{t(routingHelp.discussionAgent?.usable
        ? 'disc.routingHelpDefault'
        : 'disc.routingHelpDefaultDisabled')}</p>
      <div className="disc-routing-help-group">
        <strong>{t('disc.routingActiveAgents')}</strong>
        <ul>
          {routingHelp.discussionAgent?.usable && (
            <li>
              <code>
                {routingHelp.discussionAgent.trigger}
                <span> · {t('disc.targetDiscussionAgent')}</span>
              </code>
              {' — '}{t('disc.routingHelpDiscussionAgent')}
            </li>
          )}
          {routingHelp.cliSessions.map(mention => (
            <li key={`cli:${mention.target?.cli_session_id}`}>
              <code>
                {mention.trigger}
                <span> · {mention.label}</span>
              </code>
              {' — '}{t('disc.routingHelpCli')}
            </li>
          ))}
          {routingHelp.activePunctualAgents.map(mention => (
            <li key={`agent:${mention.target?.agent_type}`}>
              <code>
                {mention.displayTrigger}
                <span> · {t('disc.targetPunctualAgent')}</span>
              </code>
              {' — '}{t('disc.routingHelpPunctual')}
            </li>
          ))}
        </ul>
      </div>
      {routingHelp.availablePunctualAgents.length > 0 && (
        <div className="disc-routing-help-group">
          <strong>{t('disc.routingAvailableAgents')}</strong>
          <ul>
            {routingHelp.availablePunctualAgents.map(mention => (
              <li key={`available:${mention.target?.agent_type}`}>
                <code>
                  {mention.displayTrigger}
                  <span> · {t('disc.targetPunctualAgent')}</span>
                </code>
                {' — '}{t('disc.routingHelpPunctual')}
              </li>
            ))}
          </ul>
        </div>
      )}
      {routingHelp.discussionAgent && !routingHelp.discussionAgent.usable && (
        <div className="disc-routing-help-group">
          <strong>{t('disc.routingDisabledAgent')}</strong>
          <ul>
            <li>
              <code>
                {routingHelp.discussionAgent.trigger}
                <span> · {t('disc.targetDiscussionAgent')}</span>
              </code>
              {' — '}{t('disc.routingHelpDiscussionAgentDisabled')}
            </li>
          </ul>
        </div>
      )}
      <ul>
        <li>
          <code>@all</code>
          {' — '}{t('disc.routingHelpAll', routingHelp.allParticipants)}
        </li>
      </ul>
      <p className="text-muted">{t('disc.routingHelpTokenSaver')}</p>
    </div>
  );

  // ─── Render ──────────────────────────────────────────────────────────────
  return (
    <div className="disc-composer-wrap" data-disabled={disabled}>
      {/* Voice mode countdown banner */}
      {voiceCountdown !== null && (
        <div className="disc-voice-countdown">
          <span className="disc-voice-countdown-number">{voiceCountdown}</span>
          <span className="disc-voice-countdown-text">{t('disc.resumeListening')}</span>
          <button
            className="disc-voice-cancel-btn"
            onClick={() => {
              if (voiceCountdownRef.current) { clearInterval(voiceCountdownRef.current); voiceCountdownRef.current = null; }
              setVoiceCountdown(null);
              setVoiceMode(false);
            }}
          >
            {t('disc.cancelVoice')}
          </button>
        </div>
      )}
      {/* Recording indicator banner */}
      {sttState === 'recording' && (
        <div className="disc-recording-banner">
          <span className="disc-recording-dot" />
          <span className="disc-recording-text">{t('disc.recording')}</span>
          <button
            className="disc-recording-cancel-btn"
            onClick={() => {
              sttCancelledRef.current = true;
              mediaRecorderRef.current?.stop();
              if (voiceMode) { setVoiceMode(false); }
            }}
          >
            <X size={10} /> {t('disc.cancelVoice')}
          </button>
          <button className="disc-recording-stop-btn" onClick={handleMicToggle}>
            <StopCircle size={10} /> {voiceMode ? t('disc.sendVoice') : t('disc.stopRecording')}
          </button>
        </div>
      )}
      {sttState === 'loading' && (
        <div className="disc-transcribing-banner">
          <Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} className="text-accent" />
          <span className="disc-transcribing-text">{t('disc.sttModelLoading')}</span>
        </div>
      )}
      {sttState === 'transcribing' && (
        <div className="disc-transcribing-banner">
          <Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} className="text-accent" />
          <span className="disc-transcribing-text">{t('disc.transcribing')}</span>
        </div>
      )}

      {/* Composer container — drag & drop + clipboard paste */}
      <div
        className={`disc-composer ${dragOver ? 'disc-composer-dragover' : ''}`}
        data-recording={sttState === 'recording'}
        onDragOver={e => { if (onUploadFiles) { e.preventDefault(); setDragOver(true); } }}
        onDragEnter={e => { if (onUploadFiles) { e.preventDefault(); setDragOver(true); } }}
        onDragLeave={() => setDragOver(false)}
        onDrop={e => {
          e.preventDefault();
          setDragOver(false);
          if (onUploadFiles && e.dataTransfer.files.length > 0) {
            onUploadFiles(Array.from(e.dataTransfer.files));
          }
        }}
        onPaste={e => {
          // 1) File paste (images / attachments) — existing behaviour.
          if (onUploadFiles) {
            const items = Array.from(e.clipboardData.items);
            const files = items
              .filter(item => item.kind === 'file')
              .map(item => item.getAsFile())
              .filter((f): f is File => f !== null);
            if (files.length > 0) {
              e.preventDefault();
              onUploadFiles(files);
              return;
            }
          }
          // 2) Blockquote-aware multiline paste: pasting several lines while the
          //    caret is on a `> ` line keeps the whole paste quoted. No-op
          //    otherwise (single line, or not in a blockquote) → native paste.
          const ta = chatInputRef.current;
          if (!ta) return;
          const start = ta.selectionStart ?? 0;
          const lineStart = ta.value.lastIndexOf('\n', start - 1) + 1;
          const currentLine = ta.value.slice(lineStart, start);
          const quoted = quoteMultilinePaste(currentLine, e.clipboardData.getData('text'));
          if (quoted === null) return;
          e.preventDefault();
          // Insert at the selection (preserves the textarea's own undo stack),
          // then re-run the existing onChange pipeline (draft save, autosize,
          // emoji popover) by dispatching the input event React listens to.
          ta.setRangeText(quoted, start, ta.selectionEnd ?? start, 'end');
          ta.dispatchEvent(new Event('input', { bubbles: true }));
        }}
      >
        {/* @mention autocomplete dropdown */}
        {mentionQuery !== null
          && (filteredMentionOptions.length > 0 || filteredDisabledMentionOptions.length > 0)
          && (
            <div className="disc-mention-popover">
              {filteredMentionOptions.map(({ mention: m, group }, i) => (
                <Fragment key={m.trigger}>
                  {(i === 0 || filteredMentionOptions[i - 1].group !== group) && (
                    <div className="disc-mention-group">
                      {t(group === 'active'
                        ? 'disc.routingActiveAgents'
                        : 'disc.routingAvailableAgents')}
                    </div>
                  )}
                  <div
                    className="disc-mention-item"
                    data-highlighted={i === mentionIndex}
                    onMouseEnter={() => setMentionIndex(i)}
                  >
                    <button
                      type="button"
                      className="disc-mention-main"
                      onMouseDown={e => {
                        e.preventDefault();
                        applyMentionSuggestion(m.trigger, m.type);
                      }}
                    >
                      {m.type
                        ? <Cpu size={12} style={{ color: agentTextColor(m.type) }} />
                        : <Users size={12} className="text-accent" />}
                      <span
                        className="font-semibold"
                        style={m.type ? { color: agentTextColor(m.type) } : undefined}
                      >
                        {m.displayTrigger}
                      </span>
                      <span className="text-muted">{m.label}</span>
                      {m.target?.kind === 'cli' && (
                        <span
                          className="disc-mention-routing-mode"
                          title={t('disc.routingCliModelManaged')}
                          aria-label={t('disc.routingCliModelManaged')}
                        >
                          <Terminal size={12} aria-hidden="true" />
                        </span>
                      )}
                    </button>
                    {m.type && m.target && m.target.kind !== 'cli' && (
                        <span
                          className="disc-mention-tier-choices"
                          aria-label={mentionRoutingMode(m)?.title}
                        >
                          {MENTION_TIER_CHOICES.map(tier => (
                            <button
                              key={tier}
                              type="button"
                              className="disc-mention-tier-choice"
                              data-tier={tier}
                              data-current={mentionRoutingMode(m)?.tier === tier}
                              data-keyboard-selected={
                                i === mentionIndex
                                && mentionTierIndex !== null
                                && MENTION_TIER_CHOICES[mentionTierIndex] === tier
                              }
                              aria-label={`${m.displayTrigger} · ${mentionTierChoiceTitle(m.type as AgentType, tier)}`}
                              title={mentionTierChoiceTitle(m.type as AgentType, tier)}
                              onMouseDown={event => {
                                event.preventDefault();
                                event.stopPropagation();
                                applyMentionSuggestion(m.trigger, m.type, tier);
                              }}
                            >
                              <span aria-hidden="true">{MODEL_TIER_ICONS[tier]}</span>
                            </button>
                          ))}
                        </span>
                    )}
                  </div>
                </Fragment>
              ))}
              {filteredDisabledMentionOptions.length > 0 && (
                <>
                  <div className="disc-mention-group">
                    {t('disc.routingDisabledAgent')}
                  </div>
                  {filteredDisabledMentionOptions.map(mention => (
                    <div
                      key={`disabled:${mention.trigger}`}
                      className="disc-mention-item disc-mention-item-disabled"
                      aria-disabled="true"
                    >
                      <Cpu
                        size={12}
                        style={{ color: mention.type ? agentTextColor(mention.type) : undefined }}
                      />
                      <span className="font-semibold">
                        {mention.displayTrigger}
                      </span>
                      <span className="text-muted">{t('disc.nativeAgentDisabled')}</span>
                      {mentionRoutingMode(mention) && (
                          <span
                            className="disc-mention-routing-mode"
                            title={mentionRoutingMode(mention)?.title}
                            aria-label={mentionRoutingMode(mention)?.title}
                          >
                            <span aria-hidden="true">{mentionRoutingMode(mention)?.icon}</span>
                          </span>
                      )}
                    </div>
                  ))}
                </>
              )}
            </div>
          )}

        {/* Emoji shortcode autocomplete (:tada: → 🎉). Reuses the same CSS
            class as @mentions so both popovers look consistent; the extra
            `disc-emoji-item` class lets us style the emoji glyph without
            disturbing the mention item layout. */}
        {emojiMatch && emojiSuggestions.length > 0 && (
          <div className="disc-mention-popover disc-emoji-popover">
            {emojiSuggestions.map((s, i) => (
              <button
                key={s.shortcode}
                type="button"
                className="disc-mention-item disc-emoji-item"
                data-highlighted={i === emojiIndex}
                onMouseDown={e => {
                  e.preventDefault();
                  applyEmojiSuggestion(s);
                  chatInputRef.current?.focus();
                }}
                onMouseEnter={() => setEmojiIndex(i)}
              >
                <span className="disc-emoji-glyph" aria-hidden="true">{s.emoji}</span>
                <span className="font-semibold text-accent">:{s.shortcode}:</span>
              </button>
            ))}
          </div>
        )}

        {replyTarget && (
          <div className="disc-reply-composer-preview" role="status">
            <Reply size={13} aria-hidden="true" />
            <div className="disc-reply-composer-copy">
              <span>
                {t('disc.replyingTo', replyAuthor)}
                <code>#{replyTarget.id.slice(0, 8)}</code>
              </span>
              <small>{replyExcerpt}</small>
            </div>
            {onCancelReply && (
              <button
                type="button"
                onClick={onCancelReply}
                aria-label={t('disc.cancelReply')}
                title={t('disc.cancelReply')}
              >
                <X size={12} />
              </button>
            )}
          </div>
        )}

        {/* Worktree error banner */}
        {worktreeError && (
          <div className="disc-worktree-error">
            <AlertTriangle size={14} className="text-error flex-shrink-0" />
            <span className="flex-1">{worktreeError}</span>
            <button
              className="disc-worktree-retry-btn"
              onClick={onWorktreeRetry}
            >
              <RotateCcw size={10} /> Retry
            </button>
            <button className="disc-worktree-dismiss-btn" onClick={onWorktreeErrorDismiss}>
              <X size={12} />
            </button>
          </div>
        )}

        {/* Previous run still recovering. Non-blocking on purpose: recovery can
            last minutes, and the text is already back in the composer. */}
        {partialPending && (
          <div className="disc-partial-pending" role="status" data-testid="disc-partial-pending">
            <AlertTriangle size={14} className="text-warning flex-shrink-0" />
            <span className="flex-1">
              {partialForcing ? t('disc.partialForcing') : t('disc.partialPendingNotice')}
              {/* KT-251 — name the answer that is blocking. Rendered only when
                  the backend knows it: an empty or fabricated id would be worse
                  than none, since the user would go looking for it. */}
              {partialPendingMessageId && (
                <code className="disc-partial-pending-id">
                  {' '}#{partialPendingMessageId.slice(0, 8)}
                </code>
              )}
            </span>
            <button
              className="disc-worktree-retry-btn"
              onClick={onPartialPendingForce}
              disabled={partialForcing}
              data-testid="disc-partial-pending-force"
            >
              <RotateCcw size={10} /> {t('disc.partialPendingForce')}
            </button>
            <button
              className="disc-worktree-dismiss-btn"
              onClick={onPartialPendingDismiss}
              disabled={partialForcing}
              data-testid="disc-partial-pending-dismiss"
            >
              <X size={12} /> {t('disc.partialPendingCancel')}
            </button>
          </div>
        )}

        {/* Context files badges */}
        {contextFiles.length > 0 && (
          <div className="disc-context-files">
            {contextFiles.map(f => (
              <span key={f.id} className={`disc-context-file-badge ${f.disk_path ? 'disc-context-file-image' : ''}`} title={`${f.filename} (${(f.original_size / 1024).toFixed(0)} KB)`}>
                {f.disk_path ? <Image size={10} className="text-accent" /> : <FileText size={10} />}
                <span className="disc-context-file-name">{f.filename}</span>
                {onDeleteContextFile && (
                  <button className="disc-context-file-remove" onClick={() => onDeleteContextFile(f.id)} aria-label="Remove file">
                    <X size={9} />
                  </button>
                )}
              </span>
            ))}
          </div>
        )}

        {/* Restored draft indicator — shown when a draft was loaded on
            discussion switch/remount. Auto-hides as soon as the user edits. */}
        {restoredDraftAt && (
          <div className="disc-draft-restored" role="status" aria-live="polite">
            <FileText size={11} className="text-muted flex-shrink-0" />
            <span className="disc-draft-restored-text">
              {t('disc.draftRestored', formatRelativeTime(restoredDraftAt, discussion?.language ?? 'fr'))}
            </span>
            <button
              type="button"
              className="disc-draft-restored-dismiss"
              onClick={() => {
                if (discussion?.id) {
                  if (draftSaveTimerRef.current) {
                    clearTimeout(draftSaveTimerRef.current);
                    draftSaveTimerRef.current = null;
                  }
                  clearDraft(discussion.id);
                }
                updateChatInput('');
                setRestoredDraftAt(null);
              }}
              aria-label={t('disc.draftDismiss')}
              title={t('disc.draftDismiss')}
            >
              <X size={10} />
            </button>
          </div>
        )}

        {Object.entries(mentionTierOverrides).length > 0 && (
          <div
            className="disc-composer-routing-chips"
            aria-label={t('disc.routingOverrides')}
          >
            {Object.entries(mentionTierOverrides).map(([agent, tier]) => {
              const agentType = agent as AgentType;
              const trigger = ALL_AGENT_MENTIONS.find(
                mention => mention.type === agentType,
              )?.trigger ?? agent;
              const title = mentionTierChoiceTitle(agentType, tier);
              return (
                <span
                  key={agent}
                  className="disc-composer-routing-chip"
                  title={title}
                >
                  <span style={{ color: agentTextColor(agentType) }}>{trigger}</span>
                  <span>{MODEL_TIER_ICONS[tier]} {t(`disc.tier.${tier}`)}</span>
                  <button
                    type="button"
                    aria-label={t('disc.routingResetTier', trigger)}
                    title={t('disc.routingResetTier', trigger)}
                    onClick={() => {
                      const nextOverrides = { ...mentionTierOverridesRef.current };
                      delete nextOverrides[agentType];
                      updateMentionTierOverrides(nextOverrides);
                      const nextPreferences = { ...preferredTiersRef.current };
                      delete nextPreferences[agentType];
                      updatePreferredTiers(nextPreferences);
                      scheduleDraftSave(chatInputValueRef.current);
                    }}
                  >
                    <X size={10} aria-hidden="true" />
                  </button>
                </span>
              );
            })}
          </div>
        )}

        {/* Shared Edit / Preview surface. The textarea stays mounted while
            preview is active so drafts, caret state and queued sends survive
            tab switches unchanged. */}
        <MarkdownEditor
          key={discussion?.id ?? 'none'}
          sourceId={`disc-message-composer-${discussion?.id ?? 'none'}`}
          embedded
          helpTitle={t('disc.composerHelpTitle')}
          helpContent={composerHelpContent}
        >
        <textarea
          id={`disc-message-composer-${discussion?.id ?? 'none'}`}
          ref={chatInputRef}
          className="disc-composer-textarea"
          rows={1}
          aria-label={t('disc.messagePlaceholder')}
          placeholder={t('disc.messagePlaceholder')}
          defaultValue=""
          onChange={e => {
            const val = e.target.value;
            chatInputValueRef.current = val;
            const hadText = chatInputHasText;
            const hasText = val.trim().length > 0;
            if (hadText !== hasText) setChatInput(val);
            const ta = e.target;
            requestAnimationFrame(() => { ta.style.height = 'auto'; ta.style.height = Math.min(ta.scrollHeight, 160) + 'px'; });
            // Persist draft so tab/page navigation doesn't wipe the in-flight
            // textarea content. Debounced inside scheduleDraftSave.
            pruneMentionTierOverrides(val);
            scheduleDraftSave(val);
            // Hide the "restored draft" hint as soon as the user edits.
            if (restoredDraftAt) setRestoredDraftAt(null);
            refreshMentionQuery(val, ta.selectionStart ?? val.length);
            // Emoji shortcode autocomplete — uses the caret position, not
            // just the full value, so `:ta` buried mid-sentence also opens.
            refreshEmojiQuery(val, ta.selectionStart ?? val.length);
          }}
          onKeyUp={e => {
            // Caret-only moves (arrow keys inside existing text) don't
            // fire onChange but still need to refresh the emoji popover
            // so that moving the caret into / out of a `:word` segment
            // shows or hides the popover.
            //
            // BUT: when the popover is already open, Up/Down navigate
            // the popover (handled in onKeyDown above) and the keydown
            // calls `preventDefault` so the textarea caret doesn't
            // move. If we still call `refreshEmojiQuery` here it
            // reaches `setEmojiIndex(0)` and wipes whatever index the
            // keydown just set, locking the user at index 0 — they can
            // briefly flash to 1 between keydown/keyup but never reach
            // 2+. Reported as "seuls les 2 premiers emojis sont
            // sélectionnables". Skip refresh on Up/Down while the
            // popover is open; Left/Right/Home/End still refresh
            // because they DO move the caret and may exit the query.
            if (
              (emojiMatch || mentionQuery !== null)
              && (e.key === 'ArrowUp' || e.key === 'ArrowDown')
            ) return;
            // A Left/Right that picked a tier moved no caret and must not refresh:
            // the refresh would reset the highlighted row to 0 and undo the
            // Up/Down the user had just done. Only that exact case is skipped —
            // a Left/Right on a row WITHOUT tiers still moves the caret and still
            // needs the refresh, which is why this is a consumed-flag and not a
            // blanket exclusion of the two keys.
            if (tierKeyConsumedRef.current) {
              tierKeyConsumedRef.current = false;
              return;
            }
            if (['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) {
              const ta = e.currentTarget;
              refreshMentionQuery(ta.value, ta.selectionStart ?? ta.value.length);
              refreshEmojiQuery(ta.value, ta.selectionStart ?? ta.value.length);
            }
          }}
          onClick={e => {
            const ta = e.currentTarget;
            refreshMentionQuery(ta.value, ta.selectionStart ?? ta.value.length);
            refreshEmojiQuery(ta.value, ta.selectionStart ?? ta.value.length);
          }}
          onKeyDown={e => {
            // Emoji popover takes priority over the mention popover and
            // over the default Enter-to-send behavior. Keeps keyboard UX
            // predictable: Tab/Enter confirm the highlighted suggestion,
            // Escape dismisses, arrows move the selection.
            if (emojiMatch && emojiSuggestions.length > 0) {
              if (e.key === 'ArrowDown') { e.preventDefault(); setEmojiIndex(i => Math.min(i + 1, emojiSuggestions.length - 1)); return; }
              if (e.key === 'ArrowUp')   { e.preventDefault(); setEmojiIndex(i => Math.max(i - 1, 0)); return; }
              if (e.key === 'Tab' || e.key === 'Enter') {
                e.preventDefault();
                applyEmojiSuggestion(emojiSuggestions[emojiIndex]);
                return;
              }
              if (e.key === 'Escape') {
                e.preventDefault();
                setEmojiMatch(null);
                setEmojiSuggestions([]);
                return;
              }
            }
            if (mentionQuery !== null) {
              // The SAME list the palette renders, so the highlighted row and the
              // row Tab/Enter applies can never drift apart.
              const filtered = filteredMentionOptions;
              const highlighted = filtered[mentionIndex]?.mention;
              // Only a non-CLI agent mention exposes tiers (a joined CLI manages
              // its own model), which is exactly the render-side condition.
              const tierable = Boolean(
                highlighted?.type
                && highlighted.target
                && highlighted.target.kind !== 'cli',
              );
              if (e.key === 'ArrowDown') { e.preventDefault(); setMentionIndex(i => Math.min(i + 1, filtered.length - 1)); setMentionTierIndex(null); return; }
              if (e.key === 'ArrowUp') { e.preventDefault(); setMentionIndex(i => Math.max(i - 1, 0)); setMentionTierIndex(null); return; }
              if (tierable && highlighted && (e.key === 'ArrowRight' || e.key === 'ArrowLeft')) {
                // preventDefault keeps the caret still, so the query survives —
                // the keyup handler refreshes on Left/Right precisely because they
                // normally move the caret out of the mention. The flag tells that
                // handler this keypress was ours, so it leaves the highlighted row
                // alone instead of resetting it to the top.
                e.preventDefault();
                tierKeyConsumedRef.current = true;
                const step = e.key === 'ArrowRight' ? 1 : -1;
                setMentionTierIndex(current => {
                  // Start from what is already in effect for this row rather than
                  // from index 0, so one keypress moves one step from what the user
                  // can see highlighted.
                  const from = current
                    ?? Math.max(
                      0,
                      MENTION_TIER_CHOICES.indexOf(
                        (mentionRoutingMode(highlighted)?.tier as ModelTier | undefined) ?? 'default',
                      ),
                    );
                  return Math.min(Math.max(from + step, 0), MENTION_TIER_CHOICES.length - 1);
                });
                return;
              }
              if ((e.key === 'Tab' || e.key === 'Enter') && filtered.length > 0) {
                e.preventDefault();
                const selectedMention = filtered[mentionIndex].mention;
                applyMentionSuggestion(
                  selectedMention.trigger,
                  selectedMention.type,
                  mentionTierIndex === null ? undefined : MENTION_TIER_CHOICES[mentionTierIndex],
                );
                return;
              }
              if (e.key === 'Escape') { setMentionQuery(null); return; }
            }
            // `nativeEvent.isComposing` is true while an IME is
            // composing a candidate (CJK, accented dead keys on some
            // layouts). Pressing Enter to confirm the composition
            // should not send the message.
            if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              handleSendMessage();
            }
          }}
          // Editable WHILE the agent streams — typing + Enter queues a
          // follow-up (the parent routes it to useMessageQueue). Only a
          // hard-disabled composer (no usable agent) blocks input.
          disabled={disabled && !sendAsNote}
        />
        </MarkdownEditor>

        {/* Bottom toolbar inside composer */}
        <div className="disc-composer-toolbar" data-mobile={isMobile}>
          {/* Left: secondary actions */}
          <div className="disc-composer-left">
            {/* Mic / STT */}
            <button
              className="disc-tool-btn"
              data-active={sttState === 'recording'}
              data-color="red"
              onClick={handleMicToggle}
              disabled={sending || sttState === 'transcribing' || sttState === 'loading'}
              title={sttState === 'recording' ? t('disc.micStop') : t('disc.micDictate')}
              aria-label={sttState === 'recording' ? t('disc.micStop') : t('disc.micDictate')}
            >
              {sttState === 'recording' ? <MicOff size={15} /> : <Mic size={15} />}
            </button>

            {/* Voice conversation mode */}
            <button
              className="disc-tool-btn"
              data-active={voiceMode}
              data-color="accent"
              onClick={() => {
                const next = !voiceMode;
                setVoiceMode(next);
                if (next) {
                  // Voice mode implicitly enables TTS — only toggle if currently disabled
                  if (!ttsEnabled) onTtsToggle();
                } else {
                  if (voiceCountdownRef.current) { clearInterval(voiceCountdownRef.current); voiceCountdownRef.current = null; }
                  setVoiceCountdown(null);
                }
              }}
              title={voiceMode ? t('disc.voiceModeOff') : t('disc.voiceModeOn')}
              aria-label={voiceMode ? t('disc.voiceModeOff') : t('disc.voiceModeOn')}
            >
              {voiceMode ? <Phone size={15} /> : <PhoneOff size={15} />}
            </button>

            {/* TTS toggle */}
            <button
              className="disc-tool-btn"
              data-active={ttsEnabled}
              data-color="accent"
              onClick={onTtsToggle}
              title={ttsEnabled ? t('disc.ttsDisable') : t('disc.ttsEnable')}
              aria-label={ttsEnabled ? t('disc.ttsDisable') : t('disc.ttsEnable')}
            >
              {ttsEnabled ? <Volume2 size={15} /> : <VolumeX size={15} />}
            </button>

            {discussionNotesEnabled && (
              <span className="disc-note-tools" role="group" aria-label={t('disc.note.label')}>
                <button
                  type="button"
                  className="disc-tool-btn"
                  data-active={sendAsNote}
                  data-color="warning"
                  onClick={() => setSendAsNote(current => !current)}
                  title={sendAsNote ? t('disc.note.sendAsMessage') : t('disc.note.sendAsNote')}
                  aria-label={sendAsNote ? t('disc.note.sendAsMessage') : t('disc.note.sendAsNote')}
                  aria-pressed={sendAsNote}
                >
                  <StickyNote size={15} />
                </button>
                {hasDiscussionNotes && onToggleDiscussionNotes && (
                  <button
                    type="button"
                    className="disc-tool-btn disc-note-visibility-btn"
                    data-color="warning"
                    data-active={showDiscussionNotes}
                    onClick={onToggleDiscussionNotes}
                    title={showDiscussionNotes ? t('disc.note.hide') : t('disc.note.show')}
                    aria-label={showDiscussionNotes ? t('disc.note.hide') : t('disc.note.show')}
                    aria-pressed={showDiscussionNotes}
                  >
                    {showDiscussionNotes ? <Eye size={15} /> : <EyeOff size={15} />}
                  </button>
                )}
              </span>
            )}

            {/* Debate / multi-agent */}
            <div className="relative">
              <button
                className="disc-tool-btn"
                data-active={showDebatePopover}
                data-color="purple"
                onClick={() => {
                  if (!showDebatePopover) {
                    setDebateAgents(installedAgentsList.map(a => a.agent_type));
                  }
                  setShowDebatePopover(!showDebatePopover);
                }}
                disabled={sending}
                title={t('debate.title')}
                aria-label={t('debate.title')}
              >
                <Users size={15} />
              </button>
              {showDebatePopover && (
                <div className="disc-debate-popover">
                  <div className="disc-debate-title">
                    <Users size={12} /> {t('debate.header')}
                  </div>
                  <p className="disc-debate-desc">
                    {t('debate.instructions', debateRounds, debateRounds > 1 ? 's' : '')}
                  </p>
                  {installedAgentsList.map(a => {
                    const isPrincipal = a.agent_type === discussion?.agent;
                    const checked = debateAgents.includes(a.agent_type);
                    return (
                      <label key={a.name} className="disc-debate-agent-label"
                        style={{
                          cursor: isPrincipal ? 'default' : 'pointer',
                          color: checked ? 'var(--kr-text-primary)' : 'var(--kr-text-faint)',
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
                          style={{ accentColor: 'var(--kr-purple)' }}
                        />
                        <Cpu size={11} style={{ color: isPrincipal ? 'var(--kr-accent-ink)' : 'var(--kr-purple)' }} />
                        {a.name}
                        {isPrincipal && (
                          <span className="disc-debate-agent-main">{t('debate.main')}</span>
                        )}
                      </label>
                    );
                  })}
                  <div className="disc-debate-rounds-row">
                    <span className="disc-debate-rounds-label">{t('debate.rounds')}</span>
                    {[1, 2, 3].map(n => (
                      <button
                        key={n}
                        className="disc-debate-round-btn"
                        data-active={debateRounds === n}
                        onClick={() => setDebateRounds(n)}
                      >
                        {n}
                      </button>
                    ))}
                  </div>
                  {/* Recommended skills for debate */}
                  {(() => {
                    const DEBATE_SKILL_IDS = ['token-saver', 'devils-advocate'];
                    const discSkillIds = discussion?.skill_ids ?? [];
                    const relevantIds = [...new Set([...DEBATE_SKILL_IDS, ...discSkillIds])];
                    const relevantSkills = relevantIds
                      .map(id => availableSkills.find(s => s.id === id))
                      .filter((s): s is Skill => !!s);
                    if (relevantSkills.length === 0) return null;
                    return (
                      <div className="disc-debate-section">
                        <div className="disc-debate-section-label">
                          <Zap size={10} /> Skills
                        </div>
                        <div className="flex-wrap gap-2">
                          {relevantSkills.map(skill => {
                            const active = debateSkillIds.includes(skill.id);
                            return (
                              <button
                                key={skill.id}
                                title={skill.description || skill.name}
                                className="disc-debate-chip"
                                data-active={active}
                                data-color="accent"
                                onClick={() => setDebateSkillIds(prev =>
                                  prev.includes(skill.id)
                                    ? prev.filter(id => id !== skill.id)
                                    : [...prev, skill.id]
                                )}
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
                    <div className="disc-debate-section">
                      <div className="disc-debate-section-label">
                        <FileText size={10} /> {t('directives.title')}
                      </div>
                      <div className="flex-wrap gap-2">
                        {availableDirectives.map(directive => {
                          const active = debateDirectiveIds.includes(directive.id);
                          return (
                            <button
                              key={directive.id}
                              title={directive.description || directive.name}
                              className="disc-debate-chip"
                              data-active={active}
                              data-color="warning"
                              onClick={() => setDebateDirectiveIds(prev =>
                                prev.includes(directive.id)
                                  ? prev.filter(id => id !== directive.id)
                                  : [...prev, directive.id]
                              )}
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
                    <div className="disc-restricted-warn" style={{ marginTop: 8, marginBottom: 0 }}>
                      <AlertTriangle size={10} className="text-warning flex-shrink-0" />
                      <span className="disc-restricted-warn-text">
                        {t('config.restrictedDebate')}
                      </span>
                    </div>
                  )}
                  <button
                    className="disc-debate-launch-btn"
                    data-ready={debateAgents.length >= 2}
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
          <div className="flex-1" />

          {/* Right: shortcut hint + primary action */}
          <span className="disc-composer-hint">
            {sending ? (chatInputHasText ? t('disc.queueHint') : '') : 'Enter'}
          </span>

          {sending ? (
            <div className="flex-row gap-2" style={{ alignItems: 'center' }}>
              {/* Queued QP badge — click to cancel */}
              {queuedQP && onCancelQueuedQP && (
                <button
                  type="button"
                  className="disc-queued-qp-badge"
                  onClick={onCancelQueuedQP}
                  title={t('disc.cancelQueuedQP')}
                >
                  <Zap size={10} />
                  <span>{queuedQP.icon} {queuedQP.name}</span>
                  <X size={9} />
                </button>
              )}
              {/* Queue a QP picker — only QPs without variables */}
              {!queuedQP && onQueueQP && chainableQPs.length > 0 && qpChainPicker(onQueueQP)}
              {/* Queue-send: while the agent streams, a message with text can
                  be added to the queue by mouse (Enter does it from the
                  textarea). Only shown when there's something to queue. */}
              {chatInputHasText && (
                <button
                  className="disc-send-btn"
                  data-active={true}
                  data-variant="queue"
                  onClick={handleSendMessage}
                  title={t('disc.queueSend')}
                  aria-label={t('disc.queueSend')}
                >
                  <Send size={16} />
                </button>
              )}
              <button
                className="disc-stop-btn"
                onClick={onStop}
                title={t('disc.stopThinking')}
                aria-label={t('disc.stopThinking')}
              >
                <StopCircle size={16} />
              </button>
            </div>
          ) : (
            <>
              {onUploadFiles && (
                <>
                  <input
                    type="file"
                    multiple
                    style={{ display: 'none' }}
                    ref={fileInputRef}
                    onChange={e => {
                      const files = Array.from(e.target.files ?? []);
                      if (files.length > 0) onUploadFiles(files);
                      e.target.value = '';
                    }}
                  />
                  <button
                    className="disc-attach-btn"
                    onClick={() => fileInputRef.current?.click()}
                    disabled={uploadingFiles}
                    aria-label={t('disc.attachFile')}
                    title={t('disc.attachFile')}
                  >
                    {uploadingFiles ? <Loader2 size={14} className="set-spin" /> : <Paperclip size={14} />}
                    {contextFiles.length > 0 && <span className="disc-attach-count">{contextFiles.length}</span>}
                  </button>
                </>
              )}
              {/* Chain picker while idle: launches the QP now (sends its
                  prompt into this discussion). Variable-free QPs only. */}
              {discussion && chainableQPs.length > 0 && qpChainPicker(qp => onSend(qp.prompt_template))}
              <button
                className="disc-send-btn"
                data-active={chatInputHasText}
                onClick={handleSendMessage}
                disabled={!chatInputHasText}
                aria-label="Send message"
              >
                <Send size={16} />
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
