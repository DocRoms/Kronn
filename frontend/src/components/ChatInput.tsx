import { useState, useRef, useEffect, useCallback, useMemo } from 'react';
import '../pages/DiscussionsPage.css';
import type { Discussion, AgentDetection, AgentType, Skill, Directive, ContextFile, QuickPrompt } from '../types/generated';
import { isUsable } from '../lib/constants';
import { audioBufferToFloat32, transcribeAudio } from '../lib/stt-engine';
import { loadDraft, saveDraft, clearDraft } from '../lib/chat-drafts';
import { formatRelativeTime } from '../lib/relativeTime';
import { discussions as discussionsApi, autoTriggersApi } from '../lib/api';
import { detectTriggeredSkills } from '../lib/autoTriggers';
import {
  findEmojiQuery, searchEmojis, applyEmojiReplacement,
  type EmojiQuery, type EmojiSuggestion,
} from '../lib/emoji-autocomplete';
import type { ToastFn } from '../hooks/useToast';
import {
  Send, X, AlertTriangle, Users,
  StopCircle, RotateCcw, Loader2,
  Cpu, Mic, MicOff, Phone, PhoneOff,
  Volume2, VolumeX, Check, Zap, FileText, Paperclip, Image,
} from 'lucide-react';
import { useIsMobile } from '../hooks/useMediaQuery';

const ALL_AGENT_MENTIONS: { trigger: string; type: AgentType; label: string }[] = [
  { trigger: '@claude', type: 'ClaudeCode', label: 'Claude Code' },
  { trigger: '@codex', type: 'Codex', label: 'Codex' },
  { trigger: '@vibe', type: 'Vibe', label: 'Vibe' },
  { trigger: '@gemini', type: 'GeminiCli', label: 'Gemini CLI' },
  { trigger: '@kiro', type: 'Kiro', label: 'Kiro' },
  { trigger: '@copilot', type: 'CopilotCli', label: 'GitHub Copilot' },
];

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
  availableSkills: Skill[];
  availableDirectives: Directive[];
  onSend: (text: string, targetAgent?: AgentType) => void;
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
  toast: ToastFn;
  t: (key: string, ...args: any[]) => string;
}

export function ChatInput({
  discussion,
  agents,
  sending,
  disabled,
  ttsEnabled,
  ttsState,
  worktreeError,
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
  const chatInputValueRef = useRef('');
  const chatInputHasText = chatInput.trim().length > 0;
  const chatInputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const updateChatInput = useCallback((val: string) => {
    chatInputValueRef.current = val;
    setChatInput(val);
    if (chatInputRef.current) chatInputRef.current.value = val;
  }, []);

  // ─── Draft persistence (per-discussion) ─────────────────────────────────
  // Saved to localStorage so the textarea survives tab/page navigation.
  // The textarea is non-controlled (chatInputRef) for perf — this hook
  // rehydrates its `value` on discussion change, saves throttled on change,
  // and clears on successful send.
  const [restoredDraftAt, setRestoredDraftAt] = useState<string | null>(null);
  const draftSaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const currentDiscIdRef = useRef<string | null>(null);

  const scheduleDraftSave = useCallback((text: string) => {
    const discId = currentDiscIdRef.current;
    if (!discId) return;
    if (draftSaveTimerRef.current) clearTimeout(draftSaveTimerRef.current);
    // 250ms debounce — fast enough to survive a "type-and-tab-away" gesture
    // but sparse enough to never hammer localStorage on long messages.
    draftSaveTimerRef.current = setTimeout(() => {
      saveDraft(discId, text);
    }, 250);
  }, []);

  const flushDraftNow = useCallback((discId: string, text: string) => {
    if (draftSaveTimerRef.current) {
      clearTimeout(draftSaveTimerRef.current);
      draftSaveTimerRef.current = null;
    }
    saveDraft(discId, text);
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
      setRestoredDraftAt(null);
      return;
    }

    const saved = loadDraft(nextDiscId);
    if (saved) {
      updateChatInput(saved.text);
      setRestoredDraftAt(saved.savedAt);
    } else {
      updateChatInput('');
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
        saveDraft(discId, chatInputValueRef.current);
      }
    };
  }, []);

  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const [mentionIndex, setMentionIndex] = useState(0);

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

  const [sttState, setSttState] = useState<'idle' | 'recording' | 'transcribing'>('idle');
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

  const handleSendMessageRef = useRef<(() => void) | null>(null);

  // ─── Derived data ────────────────────────────────────────────────────────
  const installedAgentsList = useMemo(() => agents.filter(isUsable), [agents]);

  const AGENT_MENTIONS = useMemo(() => {
    const activeAgentTypes = new Set(installedAgentsList.map(a => a.agent_type));
    return ALL_AGENT_MENTIONS.filter(m => activeAgentTypes.has(m.type));
  }, [installedAgentsList]);

  const parseMention = (text: string): { targetAgent?: AgentType } => {
    for (const m of AGENT_MENTIONS) {
      if (text.toLowerCase().startsWith(m.trigger + ' ') || text.toLowerCase() === m.trigger) {
        return { targetAgent: m.type };
      }
    }
    return {};
  };

  // ─── Send handler ────────────────────────────────────────────────────────
  const handleSendMessage = useCallback(async () => {
    const inputVal = chatInputValueRef.current;
    if (!discussion || !inputVal.trim() || sending) return;
    const msg = inputVal.trim();
    const { targetAgent } = parseMention(msg);

    // ── Auto-trigger skills based on message keywords ──
    // Every skill can declare regex triggers in its frontmatter
    // (see `backend/src/skills/kronn-docs.md`). If the pending
    // message matches a trigger for a skill that's not yet active,
    // we add it to `discussion.skill_ids` BEFORE firing onSend so
    // the backend picks it up on the same turn. Non-blocking: if
    // the update fails we still send the message (better to lose
    // the auto-activation than the whole message).
    const locale = discussion.language ?? 'fr';
    const triggered = detectTriggeredSkills(
      msg,
      availableSkills,
      discussion.skill_ids ?? [],
      locale,
      disabledAutoSkills,
    );
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
      }
    }

    // Drop the persisted draft BEFORE we clear the textarea: if the onSend
    // callback throws synchronously we still don't want to leave a stale
    // draft around (the user sees the message in the chat anyway).
    if (draftSaveTimerRef.current) {
      clearTimeout(draftSaveTimerRef.current);
      draftSaveTimerRef.current = null;
    }
    clearDraft(discussion.id);
    setRestoredDraftAt(null);
    updateChatInput('');
    setMentionQuery(null);
    onSend(msg, targetAgent);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [discussion, sending, onSend, updateChatInput, AGENT_MENTIONS, availableSkills, toast, t, disabledAutoSkills]);

  handleSendMessageRef.current = handleSendMessage;

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
    if (sttState === 'transcribing') return;

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
          const arrayBuf = await blob.arrayBuffer();
          const audioCtx = new AudioContext({ sampleRate: 16000 });
          let decoded;
          try {
            decoded = await audioCtx.decodeAudioData(arrayBuf);
          } finally {
            await audioCtx.close();
          }
          const float32 = audioBufferToFloat32(decoded);

          const lang = discussion?.language || 'fr';
          const text = await transcribeAudio(getSttWorker(), float32, lang);

          if (text) {
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
  }, [sttState, discussion?.language, voiceMode, updateChatInput]);

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
          if (!onUploadFiles) return;
          const items = Array.from(e.clipboardData.items);
          const files = items
            .filter(item => item.kind === 'file')
            .map(item => item.getAsFile())
            .filter((f): f is File => f !== null);
          if (files.length > 0) {
            e.preventDefault();
            onUploadFiles(files);
          }
        }}
      >
        {/* @mention autocomplete dropdown */}
        {mentionQuery !== null && (() => {
          const filtered = AGENT_MENTIONS.filter(m => m.trigger.slice(1).startsWith(mentionQuery ?? ''));
          if (filtered.length === 0) return null;
          return (
            <div className="disc-mention-popover">
              {filtered.map((m, i) => (
                <button
                  key={m.trigger}
                  className="disc-mention-item"
                  data-highlighted={i === mentionIndex}
                  onMouseDown={e => {
                    e.preventDefault();
                    updateChatInput(m.trigger + ' ');
                    setMentionQuery(null);
                    chatInputRef.current?.focus();
                  }}
                  onMouseEnter={() => setMentionIndex(i)}
                >
                  <Cpu size={12} className="text-accent" />
                  <span className="font-semibold text-accent">{m.trigger}</span>
                  <span className="text-muted">{m.label}</span>
                </button>
              ))}
            </div>
          );
        })()}

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

        {/* Textarea */}
        <textarea
          ref={chatInputRef}
          className="disc-composer-textarea"
          rows={1}
          aria-label={t('disc.messagePlaceholder')}
          placeholder={discussion && (discussion.participants?.length ?? 0) > 1 && AGENT_MENTIONS.length > 0
            ? t('disc.mentionHint', AGENT_MENTIONS.map(m => m.trigger).join(', '))
            : t('disc.messagePlaceholder')}
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
            scheduleDraftSave(val);
            // Hide the "restored draft" hint as soon as the user edits.
            if (restoredDraftAt) setRestoredDraftAt(null);
            const atMatch = val.match(/^@(\w*)$/);
            if (atMatch) {
              setMentionQuery(atMatch[1].toLowerCase());
              setMentionIndex(0);
            } else {
              setMentionQuery(null);
            }
            // Emoji shortcode autocomplete — uses the caret position, not
            // just the full value, so `:ta` buried mid-sentence also opens.
            refreshEmojiQuery(val, ta.selectionStart ?? val.length);
          }}
          onKeyUp={e => {
            // Caret-only moves (arrow keys inside existing text) don't
            // fire onChange but still need to refresh the emoji popover.
            if (['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) {
              const ta = e.currentTarget;
              refreshEmojiQuery(ta.value, ta.selectionStart ?? ta.value.length);
            }
          }}
          onClick={e => {
            const ta = e.currentTarget;
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
          disabled={sending || disabled}
        />

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
              disabled={sending || sttState === 'transcribing'}
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
                    {t('debate.instructions')}
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
            {sending ? '' : 'Enter'}
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
              {!queuedQP && onQueueQP && chainableQPs.length > 0 && (
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
                            onQueueQP(qp);
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
