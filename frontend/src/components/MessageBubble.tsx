import {
  createContext,
  useContext,
  useState,
  useRef,
  useMemo,
  useEffect,
  useLayoutEffect,
  memo,
  Fragment,
  type ReactNode,
} from 'react';
import ReactMarkdown from 'react-markdown';
import { useTheme } from '../lib/ThemeContext';
import { useT } from '../lib/I18nContext';
import { useLocalIdentity } from '../lib/localIdentity';
import { MatrixText } from './MatrixText';
import { DocPreview } from './DocPreview';
import { DocDataExport } from './DocDataExport';
import { PlanningActionCard } from './PlanningActionCard';
import { DiscussionQuestionCard } from './DiscussionQuestionCard';
import { SourceCitationChip } from './SourceCitationChip';
import { hasSourceCitation, splitSourceCitations } from '../lib/sourceCitations';
import { DiscussionActionCard } from './DiscussionActionCard';
import { parsePlanningProposal } from '../lib/planningProposal';
import { MermaidDiagram } from './MermaidDiagram';
import remarkGfm from 'remark-gfm';
import remarkEmoji from 'remark-emoji';
import '../pages/DiscussionsPage.css';
import type { DiscussionMessage, AgentType, QuickPrompt, ContextFile, SourceCheck, MessageTarget, DiscussionAction } from '../types/generated';
import { MessageAttachments } from './MessageAttachments';
import { AGENT_LABELS, AGENT_MENTIONS, MODEL_TIER_ICONS, USER_MENTION_TRIGGER, agentColor, agentTextColor } from '../lib/constants';
import { gravatarUrl } from '../lib/gravatar';
import {
  splitInjectedContext,
  splitMessageSeed,
  stripAgentHandoff,
  isDeletedMessage,
} from '../lib/messageContent';
import { parseModelErrorEvent } from '../lib/modelErrorEvent';
import { executionVariables } from '../lib/api';
import {
  Cpu, AlertTriangle, Zap, Loader2, Pause, Play,
  Key, Settings, Send, Pencil, RotateCcw, Check, Copy, Clock, ShieldCheck,
  ChevronRight, ListTodo, User, Users, Trash2, Workflow,
  Reply, Eye, EyeOff,
} from 'lucide-react';

// Hoisted regexes (avoid creating new RegExp objects per message per render)
const RE_AUTH_ERROR = /api.?key|invalid.*key|key.*not.*config|authenticat|unauthori|login|sign.?in/i;
const RE_PARTIAL_RESPONSE = /Réponse partielle.*interrompu|Timeout d'inactivité/i;
const EDIT_TEXTAREA_MAX_HEIGHT = 160;

/** Hide the transport envelope while keeping the durable server-side card.
 * This removes presentation syntax only; validation and execution always use
 * the action registry created when the Agent message was inserted. */
function stripKronnActionFences(content: string): string {
  return content.replace(/```kronn-action\s*\n[\s\S]*?```/gi, '').trim();
}

interface ExecutionContextCard {
  run_kind: string;
  run_id: string;
  snapshot_id: string;
  resolved_at: string;
  expires_at?: string | null;
  purged: boolean;
  variables: Array<{ name: string; effective_source_ref: string; overridden: boolean }>;
}

function parseExecutionContext(content: string): ExecutionContextCard | null {
  if (!content.startsWith('execution_context:')) return null;
  try {
    return JSON.parse(content.slice('execution_context:'.length)) as ExecutionContextCard;
  } catch {
    return null;
  }
}

function resizeEditTextarea(textarea: HTMLTextAreaElement) {
  textarea.style.height = 'auto';
  textarea.style.height = `${Math.min(textarea.scrollHeight, EDIT_TEXTAREA_MAX_HEIGHT)}px`;
}

interface MentionMdNode {
  type: string;
  value?: string;
  url?: string;
  children?: MentionMdNode[];
}

const AGENT_MENTION_BY_TRIGGER = new Map(
  AGENT_MENTIONS.map(mention => [mention.trigger.toLowerCase(), mention]),
);
const AGENT_MENTION_TRIGGERS = [
  ...AGENT_MENTIONS.flatMap(({ trigger }) => {
    const escaped = trigger.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    return [escaped, `${escaped}-cli(?:-\\d+)?`];
  }),
  '@all',
  USER_MENTION_TRIGGER.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'),
].join('|');
const USER_MENTION_URL = '#kronn-user';
const MentionDiscussionAgentContext = createContext<AgentType | null>(null);

function splitAgentMentionText(value: string): MentionMdNode[] {
  const pattern = new RegExp(
    `(^|[\\s([{])(${AGENT_MENTION_TRIGGERS})(?=$|[\\s\\])},.!?;:])`,
    'gi',
  );
  const nodes: MentionMdNode[] = [];
  let last = 0;
  for (const match of value.matchAll(pattern)) {
    const leading = match[1] ?? '';
    const trigger = match[2];
    const lower = trigger.toLowerCase();
    const cliMatch = /^(@[\w-]+?)-cli(?:-(\d+))?$/.exec(lower);
    const mention = AGENT_MENTION_BY_TRIGGER.get(cliMatch?.[1] ?? lower);
    const isUser = lower === USER_MENTION_TRIGGER;
    const isAll = lower === '@all';
    if (!mention && !isUser && !isAll) continue;
    const mentionStart = (match.index ?? 0) + leading.length;
    if (mentionStart > last) {
      nodes.push({ type: 'text', value: value.slice(last, mentionStart) });
    }
    nodes.push({
      type: 'link',
      url: isUser
        ? USER_MENTION_URL
        : isAll
          ? '#kronn-all'
          : `#kronn-agent-${mention?.type ?? 'Unknown'}${cliMatch ? '-cli' : ''}`,
      children: [{
        type: 'text',
        value: isAll
          ? '@all'
          : cliMatch
          ? `${mention?.trigger ?? trigger} · CLI${cliMatch[2] ? ` ${cliMatch[2]}` : ''}`
          : trigger,
      }],
    });
    last = mentionStart + trigger.length;
  }
  if (last < value.length) nodes.push({ type: 'text', value: value.slice(last) });
  return nodes.length > 0 ? nodes : [{ type: 'text', value }];
}

function remarkAgentMentions() {
  return (tree: MentionMdNode) => {
    const walk = (node: MentionMdNode) => {
      if (!node.children) return;
      const next: MentionMdNode[] = [];
      for (const child of node.children) {
        if (child.type === 'text' && child.value) {
          next.push(...splitAgentMentionText(child.value));
        } else {
          if (
            child.type !== 'link'
            && child.type !== 'linkReference'
            && child.type !== 'inlineCode'
            && child.type !== 'code'
          ) {
            walk(child);
          }
          next.push(child);
        }
      }
      node.children = next;
    };
    walk(tree);
  };
}

// 0.8.5 (#qp-improver UX follow-up) — Kronn-emitted "seed" envelope.
// When the QP AI Improver / Workflow Architect / Bootstrap Architect
// spawn a discussion, the User message they post carries:
//   - a short visible status line ("✨ Audit en cours…")
//   - THEN the full technical seed wrapped in this marker pair.
// The agent runtime reads the entire message verbatim, so the seed
// reaches the agent. The UI parses the marker out and renders the
// seed inside a collapsed `<details>`-style toggle so the user isn't
// forced to scroll past hundreds of lines of QP JSON + catalog.
/** Same short-id label used by discussion and workflow header pills. */
function messageShortLabel(id: string): string {
  return `#${id.slice(0, 8)}`;
}

// 0.8.5 — collapsed disclosure for Kronn-internal seed payloads.
// Mirrors `<details>` semantics with kronn-styled controls. Hidden
// by default; opens on click. Pre-renders the seed inside a `<pre>`
// with whitespace-preserved formatting so JSON stays readable.
const KronnSeedToggle = memo(({ seed }: { seed: string }) => {
  const [open, setOpen] = useState(false);
  return (
    <div className="kronn-seed-toggle" data-testid="kronn-seed-toggle" style={{ marginTop: 8, fontSize: 12, color: 'var(--kr-text-dim)' }}>
      <button
        type="button"
        className="disc-icon-btn"
        style={{ padding: '3px 8px', fontSize: 11, color: 'var(--kr-text-dim)' }}
        onClick={() => setOpen(o => !o)}
        aria-expanded={open}
      >
        {open ? '▾' : '▸'} Contexte technique envoyé à l'agent
      </button>
      {open && (
        <pre
          data-testid="kronn-seed-body"
          style={{
            marginTop: 6,
            padding: 8,
            borderRadius: 4,
            background: 'rgba(255, 255, 255, 0.04)',
            fontSize: 11,
            whiteSpace: 'pre-wrap',
            wordBreak: 'break-word',
            maxHeight: 320,
            overflow: 'auto',
          }}
        >
          {seed}
        </pre>
      )}
    </div>
  );
});

// ─── MessageBubble component (memo'd to avoid re-rendering all messages) ─────

export interface MessageBubbleProps {
  msg: DiscussionMessage;
  /** Durable addressees recorded when this user message was accepted. */
  targets?: MessageTarget[];
  /** Who an ordinary turn reaches, resolved server-side. A discussion keeps
   *  its `agent` when the native responder is off, so it cannot be derived. */
  defaultTargets?: MessageTarget[];
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
  /** Provider alias for a Custom discussion agent (for example
   * `@openrouter`). The message row otherwise only knows AgentType::Custom. */
  defaultAgentAlias?: string;
  /** Dynamic aliases keyed by MessageTarget.connection_id. */
  targetConnectionAliases?: Record<string, string>;
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
  onRetryAgentDispatch?: (dispatchId: string, agentType: AgentType) => void;
  onExpandSummary: (msgId: string) => void;
  onNavigate: (page: string, opts?: { scrollTo?: string }) => void;
  /** Discussion id, threaded through to MarkdownContent so the
   *  `kronn-doc-preview` fence handler knows which generated-files
   *  directory to target when the user clicks "Export PDF". */
  discussionId?: string;
  /** 0.8.3 — project id of the active discussion (when bound). When a
   * message contains `KRONN:VALIDATION_COMPLETE` AND we have a
   * projectId, render a "View Tech Debts" CTA under the bubble that
   * jumps the user to the project card with the docs/tech-debt
   * section pre-expanded. Saves N clicks at end-of-validation. */
  projectId?: string | null;
  /** Variable-free QPs a `KRONN:CHAIN_QP:<id>` agent signal can reference.
   * When an Agent message carries that signal AND the id matches one of
   * these, a "launch this QP" CTA renders under the bubble — the agent
   * proposes the hand-off, the human stays the gate. */
  chainableQPs?: QuickPrompt[];
  /** Fires the referenced QP in this discussion (sends its prompt). */
  onLaunchQp?: (qp: QuickPrompt) => void;
  /** Typed, durable action proposals attached to this exact Agent message. */
  actions?: DiscussionAction[];
  onActionChanged?: (action: DiscussionAction) => void;
  onOpenActionDiscussion?: (discussionId: string) => void;
  /** 0.8.8 — files the user attached to THIS message (pinned at send).
   * Rendered as a strip under the content: image thumbnails (fetched as
   * auth'd blobs) and filename chips for non-images. Empty for most msgs. */
  attachments?: ContextFile[];
  /// Every media file of the discussion, so the carousel opened from this
  /// message browses the whole thread rather than this bubble alone.
  discussionMedia?: ContextFile[];
  /** F15+ — a federated attachment is announced but its binary hasn't been
   *  fetched/linked yet → show a "downloading…" placeholder until it lands. */
  pendingAttachment?: boolean;
  /** Current-discussion search state. The row attributes provide the
   * visual fallback when the CSS Custom Highlight API is unavailable. */
  isSearchMatch?: boolean;
  isSearchCurrent?: boolean;
  replyTarget?: DiscussionMessage | null;
  /** Messages that durably answer this one. Rendered as compact backlinks in
   * the footer so the original keeps visible follow-up context. */
  replies?: DiscussionMessage[];
  onReply?: (message: DiscussionMessage) => void;
  onReplyNavigate?: (messageId: string) => void;
  onDelete?: (message: DiscussionMessage) => void;
  isDeleting?: boolean;
  t: (key: string, ...args: (string | number)[]) => string;
}

export const MessageBubble = memo(function MessageBubble(props: MessageBubbleProps) {
  const { msg, isLastUser, isLastAgent, isEditing, isCopied, isTtsActive, ttsState: tts, isExpandedSummary,
    prevUserTs, defaultAgent, defaultAgentAlias, targetConnectionAliases = {}, summaryCache, language, sending, editingText, hasFullAccess,
    defaultTargets = [],
    onCopy, onTts, onEditStart, onEditCancel, onEditSubmit, onEditTextChange, onRetry, onRetryAgentDispatch, onExpandSummary, onNavigate, discussionId, projectId, chainableQPs, onLaunchQp, actions = [], onActionChanged, onOpenActionDiscussion, attachments, discussionMedia, pendingAttachment, isSearchMatch, isSearchCurrent, replyTarget, replies = [], onReply, onReplyNavigate, onDelete, isDeleting = false, targets = [], t } = props;
  const editTextareaRef = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => {
    if (isEditing && editTextareaRef.current) {
      resizeEditTextarea(editTextareaRef.current);
    }
  }, [editingText, isEditing]);
  const isUser = msg.role === 'User';
  const isOrchestrator = isUser && msg.author_pseudo === 'Orchestrateur';
  // Named targets win; otherwise the destination the server resolved for an
  // ordinary turn. Both are MessageTarget, so one renderer covers the two and
  // a CLI session reads as a CLI session in either case.
  const effectiveTargets = targets.length > 0 ? targets : defaultTargets;
  const isDeleted = isDeletedMessage(msg.content);
  const modelError = useMemo(() => {
    if (msg.role !== 'System') return null;
    const structured = parseModelErrorEvent(msg.content);
    if (structured) return structured;
    // Upgrade already-persisted pre-0.9.4 LiteLLM failures in place. Those
    // messages contain the raw HTTP body and provenance fields but predate the
    // structured marker, so users should not need to reproduce the failure to
    // gain the compact diagnostic + settings CTA.
    if (msg.agent_type !== 'LiteLlm' || !msg.model) return null;
    const status = /LiteLLM error (\d{3})\b/i.exec(msg.content)?.[1];
    if (!status || !['400', '404', '422'].includes(status)) return null;
    const tier = ['economy', 'default', 'reasoning'].includes(msg.model_tier ?? '')
      ? msg.model_tier as 'economy' | 'default' | 'reasoning'
      : 'default';
    return {
      kind: 'model_error' as const,
      status: Number(status),
      summary: t('disc.modelErrorSummary', 'LiteLLM', status, msg.model),
      detail: msg.content,
      tier,
      retry_dispatch_id: null,
      retried: false,
    };
  }, [msg.role, msg.content, msg.agent_type, msg.model, msg.model_tier, t]);
  const rawVisibleContent = isUser
    ? stripAgentHandoff(msg.content)
    : modelError?.summary ?? msg.content;
  const visibleContent = msg.role === 'Agent'
    ? stripKronnActionFences(rawVisibleContent)
    : rawVisibleContent;
  const retryDispatchId = modelError?.retry_dispatch_id ?? null;
  const errorAgentType = msg.agent_type ?? defaultAgent;
  const agentType = msg.agent_type ?? defaultAgent;
  const isTourDemo = msg.role === 'Agent'
    && msg.source_msg_id === 'kronn-guided-tour-demo-preview';
  const agentTrigger = isTourDemo
    ? t('disc.tourDemoAuthor')
    : agentType === defaultAgent && defaultAgentAlias
      ? defaultAgentAlias
    : AGENT_MENTIONS.find(mention => mention.type === agentType)?.trigger
      ?? AGENT_LABELS[agentType]
      ?? agentType;
  const agentIdentityLabel = isTourDemo
    ? t('disc.tourDemoKind')
    : msg.source_msg_id
      ? t('disc.targetCli')
      : agentType === defaultAgent
        ? t('disc.targetDiscussionAgent')
        : t('disc.targetPunctualAgent');
  const replyAuthor = useMemo(() => {
    if (!replyTarget) return '';
    if (replyTarget.agent_type) {
      return AGENT_MENTIONS.find(mention => mention.type === replyTarget.agent_type)?.trigger
        ?? replyTarget.agent_type;
    }
    return replyTarget.author_pseudo || t('disc.humanAuthor');
  }, [replyTarget, t]);
  const { mentionColors } = useLocalIdentity();
  const [isMessageIdCopied, setIsMessageIdCopied] = useState(false);
  const messageIdResetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // KT-251 — a salvaged FRAGMENT: the start of an answer whose agent was killed
  // mid-sentence, then retried. Folded by default because two half-answers beside
  // a real one read as several agents replying — that is exactly what a user
  // reported ("j'ai trois réponses 😱").
  //
  // Folded, never hidden: a fragment is real history and may hold reasoning the
  // retry never repeated, so it stays one click away.
  const isFragment = msg.recovered_partial === true;
  const [fragmentOpen, setFragmentOpen] = useState(false);
  const [orchestratorDetailsOpen, setOrchestratorDetailsOpen] = useState(false);

  const orchestratorPreview = useMemo(() => {
    if (!isOrchestrator) return '';
    const firstLine = visibleContent
      .split('\n')
      .map(line => line.trim())
      .find(Boolean)
      ?.replace(/^#{1,6}\s+/, '')
      .replace(/^\*\*(.*?)\*\*.*$/, '$1')
      .replace(/[`*_~]/g, '')
      .trim() ?? '';
    const compact = firstLine || t('disc.orchestratorUpdate');
    return compact.length > 140 ? `${compact.slice(0, 137).trimEnd()}…` : compact;
  }, [isOrchestrator, t, visibleContent]);

  useEffect(() => () => {
    if (messageIdResetTimer.current) clearTimeout(messageIdResetTimer.current);
  }, []);

  const copyMessageId = async () => {
    try {
      await navigator.clipboard.writeText(msg.id);
      setIsMessageIdCopied(true);
      if (messageIdResetTimer.current) clearTimeout(messageIdResetTimer.current);
      messageIdResetTimer.current = setTimeout(() => setIsMessageIdCopied(false), 1500);
    } catch {
      setIsMessageIdCopied(false);
    }
  };

  // KRONN:CHAIN_QP:<id> — agent-proposed QP hand-off. Agent messages only:
  // the batch seed (User role) quotes the signal inside its instructions
  // ("termine par KRONN:CHAIN_QP:…"), which must not raise the CTA.
  const chainQp = useMemo(() => {
    if (msg.role !== 'Agent' || !chainableQPs?.length) return null;
    const m = msg.content.match(/KRONN:CHAIN_QP:([0-9a-fA-F-]{8,})/);
    return m ? chainableQPs.find(q => q.id === m[1]) ?? null : null;
  }, [msg.role, msg.content, chainableQPs]);

  // Matrix theme — when the discussion first loads, decode the user's
  // LAST message as plain scrambled text before swapping to the normal
  // Markdown rendering. Gives the "you are in the system" feel without
  // disturbing agent answers or older history. Only runs once per
  // MessageBubble mount, under the matrix theme, for the last user msg.
  const { theme } = useTheme();
  const [decodeDone, setDecodeDone] = useState(false);
  const isMatrixLastUser = theme === 'matrix' && isUser && isLastUser;
  useEffect(() => {
    if (!isMatrixLastUser) { setDecodeDone(true); return; }
    setDecodeDone(false);
    // Slightly longer than the title decode (28 frames) because
    // message content is longer — gives every char time to settle.
    const timer = setTimeout(() => setDecodeDone(true), 700);
    return () => clearTimeout(timer);
  }, [isMatrixLastUser, msg.id]);

  const copyBtn = (size: number, showLabel: boolean) => (
    <button
      className="disc-copy-btn"
      data-copied={isCopied}
      onClick={() => onCopy(msg.id, visibleContent)}
      title={t('disc.copyMessage')}
    >
      {isCopied ? <><Check size={size} /> {t('disc.copied')}</> : <><Copy size={size} /> {showLabel && t('disc.copy')}</>}
    </button>
  );

  // 0.8.7 anti-hallucination — pill state + derived severity.
  const [showLint, setShowLint] = useState(false);
  const lint = msg.lint_report ?? null;
  // Three-state pill, in priority order :
  //   - 'fabricated' (red)   — at least one [src:] did not verify
  //   - 'unsourced'  (amber) — niveau-0 heuristic flagged a claim
  //   - 'verified'   (green) — every cited source verified mechanically
  //                            (or is honestly tagged unchecked: url/user/
  //                            inferred/etc.) AND no unsourced claim. Only
  //                            shown when at least ONE source has status
  //                            'verified' so a reply with only `unchecked`
  //                            citations doesn't earn a green chip for free.
  //   - null                  — nothing to surface (no sources, no flags).
  const verifiedCount = lint?.sources?.filter(s => s.status === 'verified').length ?? 0;
  const unverifiedCount = lint?.unverified_count ?? 0;
  // Sources that CAN'T be machine-checked (URL / user-confirmed / inferred /
  // commit / hypothesis) — distinct from the inline anchors that DID fail
  // (those carry "couldn't verify" in their detail + drive `unverified`).
  const unverifiableCount = lint?.sources?.filter(
    s => s.status === 'unchecked' && !s.detail.includes("couldn't verify"),
  ).length ?? 0;
  // Priority (worst signal wins the headline colour):
  //   fabricated (red, formal [src:] failed) > unsourced (amber, claim w/ no
  //   anchor) > unverified (soft amber, inline anchor didn't resolve) >
  //   verified (green) > unverifiable (NEUTRAL grey — cited but uncheckable;
  //   Option B: surfaced, never hidden — "warn about everything").
  const lintSeverity: 'fabricated' | 'unsourced' | 'unverified' | 'verified' | 'unchecked' | null = lint
    ? (lint.fabricated_count > 0
        ? 'fabricated'
        : lint.unsourced_count > 0
          ? 'unsourced'
          : unverifiedCount > 0
            ? 'unverified'
            : verifiedCount > 0
              ? 'verified'
              : unverifiableCount > 0
                ? 'unchecked'
                : null)
    : null;

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

  // kronn-internal tool-call trace — System messages emitted by the
  // backend (post-stream MCP capture or slash-marker resolver) so the
  // user sees "the agent looked at message #4" inline in the
  // transcript. Distinct visual treatment from regular System messages
  // (which carry summary-cache notices or errors) so the user
  // recognises tool activity at a glance.
  const isKronnTool = msg.role === 'System' && msg.content.startsWith('[kronn-internal:');
  const isKronnPlanning = msg.role === 'System' && msg.content.startsWith('[kronn-planning:');
  const executionContext = msg.role === 'System' ? parseExecutionContext(msg.content) : null;
  const [revealedExecutionVariables, setRevealedExecutionVariables] = useState<Record<string, string>>({});
  const [revealingExecutionVariable, setRevealingExecutionVariable] = useState<string | null>(null);
  const [executionVariableError, setExecutionVariableError] = useState<string | null>(null);
  const revealTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  useEffect(() => () => {
    Object.values(revealTimers.current).forEach(clearTimeout);
  }, []);

  const remaskExecutionVariable = (name: string) => {
    if (revealTimers.current[name]) clearTimeout(revealTimers.current[name]);
    delete revealTimers.current[name];
    setRevealedExecutionVariables(current => {
      const next = { ...current };
      delete next[name];
      return next;
    });
  };

  const revealExecutionVariable = async (name: string) => {
    if (!executionContext || revealingExecutionVariable) return;
    setRevealingExecutionVariable(name);
    setExecutionVariableError(null);
    try {
      const value = await executionVariables.reveal(executionContext.run_kind, executionContext.run_id, name);
      setRevealedExecutionVariables(current => ({ ...current, [name]: value }));
      revealTimers.current[name] = setTimeout(() => remaskExecutionVariable(name), 30_000);
    } catch (error) {
      setExecutionVariableError(error instanceof Error ? error.message : String(error));
    } finally {
      setRevealingExecutionVariable(null);
    }
  };
  const kronnToolMatch = isKronnTool
    ? /^\[kronn-internal: ([a-z_]+)(?:\(([^)]*)\))?(?: → (.*))?\]$/s.exec(msg.content.trim())
    : null;

  if (isDeleted) {
    const deletedAuthor = isOrchestrator
      ? t('disc.orchestrator')
      : msg.role === 'Agent'
        ? agentTrigger
        : msg.author_pseudo || t('disc.humanAuthor');
    return (
      <div
        className="disc-msg-row"
        data-role="deleted"
        data-former-role={isUser ? 'user' : msg.role === 'System' ? 'system' : 'agent'}
        data-message-id={msg.id}
      >
        <div className="disc-msg-bubble disc-msg-deleted" data-role="deleted">
          <Trash2 size={12} aria-hidden="true" />
          <span>{t('disc.deletedMessage')}</span>
          <span className="disc-msg-deleted-meta">· {deletedAuthor} · {formattedTime}</span>
          <button
            type="button"
            className="disc-id-pill disc-message-id-pill"
            data-copied={isMessageIdCopied}
            onClick={copyMessageId}
            title={t('disc.idPillTooltip', msg.id)}
            aria-label={t('disc.idPillTooltip', msg.id)}
          >
            {isMessageIdCopied ? <Check size={8} /> : null}
            {messageShortLabel(msg.id)}
          </button>
        </div>
      </div>
    );
  }

  const visualRole = isOrchestrator
    ? 'orchestrator'
    : isUser
      ? 'user'
      : msg.role === 'System'
        ? 'system'
        : 'agent';

  return (
    // KT-58 — `data-target-agent` carries the message's durable dispatch
    // target, so CSS can single out the mention actually awaiting a reply
    // instead of giving every mention the same weight.
    <div
      className="disc-msg-row"
      data-role={visualRole}
      data-target-agent={msg.target_agent ?? undefined}
      data-message-id={msg.id}
      data-search-match={isSearchMatch || undefined}
      data-search-current={isSearchCurrent || undefined}
    >
      <div
        className={`disc-msg-bubble${isOrchestrator ? ' disc-msg-bubble-full' : ''}`}
        data-role={visualRole}
        data-editing={isEditing || undefined}
        data-variant={isOrchestrator ? 'orchestration' : msg.role === 'System' ? (
          isKronnTool ? 'kronn-tool'
          : isKronnPlanning ? 'kronn-planning'
          : msg.content.startsWith('summary cached') ? 'summary'
                  : executionContext ? 'execution-context'
                  : 'error'
        ) : undefined}
        style={msg.role === 'Agent'
          ? { borderLeftColor: agentColor(agentType, mentionColors) }
          : undefined}
      >
        <div className="disc-msg-header-row">
          <div className="disc-msg-header-main">
            {isUser && (
              // Always render a clear HUMAN attribution on user messages — even with
              // no pseudo (federated from a peer whose pseudo is unset → "anonyme").
              // The "· humain" marker is what tells a reader this is a PERSON typing
              // in Kronn, not an agent reply (F11: cross-instance, both used to read
              // as a bare "Anonymous" with no human/agent distinction).
              <div className="disc-msg-author">
                {isOrchestrator ? (
                  <span className="disc-msg-author-initials" data-kind="orchestrator">
                    <Workflow size={10} aria-hidden="true" />
                  </span>
                ) : msg.author_avatar_email ? (
                  <img src={gravatarUrl(msg.author_avatar_email, 20)} alt="" className="disc-msg-author-avatar" />
                ) : (
                  <span className="disc-msg-author-initials">
                    {(msg.author_pseudo || 'anonyme').slice(0, 2).toUpperCase()}
                  </span>
                )}
                <span className="disc-msg-author-name">
                  {isOrchestrator ? t('disc.orchestrator') : msg.author_pseudo || 'anonyme'}
                </span>
                {isOrchestrator ? (
                  <span className="disc-msg-author-kind" title={t('disc.orchestratorKind')}>
                    · {t('disc.orchestratorKind')}
                  </span>
                ) : (
                  <span
                    className="disc-msg-author-kind"
                    style={{ fontSize: 'var(--kr-fs-xs)', fontWeight: 400, opacity: 0.55 }}
                    title="Message humain — saisi dans Kronn"
                  >
                    · humain
                  </span>
                )}
                {/* A message naming nobody still goes somewhere, and the header
                 *  used to say nothing about it. Where it goes cannot be read
                 *  off the discussion's `agent`: that field survives switching
                 *  the native responder off, so showing it announced a
                 *  destination receiving nothing. The server resolves the real
                 *  one with the router's own rule and sends it as
                 *  `default_targets` — empty meaning nobody is listening, which
                 *  is worth saying out loud. */}
                <span
                  className="disc-msg-routing-receipt"
                  role="group"
                  aria-label={targets.length > 0 ? t('disc.routingRequested') : t('disc.routingImplicit')}
                  title={targets.length > 0 ? t('disc.routingRequested') : t('disc.routingImplicit')}
                  data-testid="message-routing-receipt"
                  data-implicit={targets.length === 0}
                >
                  <span className="disc-msg-routing-label" aria-hidden="true">→</span>
                  <span className="disc-msg-routing-targets">
                    {targets.length === 0 && effectiveTargets.length === 0 && (
                      <span className="disc-msg-routing-target" data-kind="nobody">
                        {t('disc.routingNobody')}
                      </span>
                    )}
                      {effectiveTargets.map((target, index) => {
                        const dynamicAlias = target.connection_id
                          ? targetConnectionAliases[target.connection_id]
                          : null;
                        const trigger = dynamicAlias
                          ? `@${dynamicAlias}`
                          : AGENT_MENTIONS.find(mention => mention.type === target.agent_type)?.trigger
                          ?? `@${AGENT_LABELS[target.agent_type] ?? target.agent_type}`;
                        const alias = target.kind === 'cli' ? `${trigger}-cli` : trigger;
                        const tierLabel = target.tier ? t(`disc.tier.${target.tier}`) : null;
                        return (
                          <span
                            key={`${target.kind}-${target.agent_type}-${target.cli_session_id ?? index}`}
                            className="disc-msg-routing-target"
                            data-kind={target.kind}
                            title={target.kind === 'cli'
                              ? `${alias} · ${t('disc.targetCli')}`
                              : tierLabel
                                ? `${alias} · ${tierLabel}`
                                : alias}
                          >
                            <span>{alias}{' '}</span>
                            {target.kind === 'cli' ? (
                              <span className="disc-msg-routing-tier">· {t('disc.targetCli')}</span>
                            ) : target.tier && tierLabel ? (
                              <span className="disc-msg-routing-tier">
                                · <span aria-hidden="true">{MODEL_TIER_ICONS[target.tier]}</span> {tierLabel}
                              </span>
                            ) : null}
                          </span>
                        );
                      })}
                  </span>
                </span>
              </div>
            )}
            {msg.role === 'Agent' && (
              <div className="disc-msg-agent-label" style={{ color: agentTextColor(agentType, mentionColors), justifyContent: 'space-between' }}>
                <span className="flex-row gap-2">
                  {/* "<agent>[ · <model>][ · <owner>]" — answers WHO + WHAT at a
                   *  glance. model_tier is present on local agent messages; a
                   *  federated agent reply also carries its owner's pseudo
                   *  (handle_incoming_chat_message) so "ClaudeCode · reasoning · Romu"
                   *  reads as "Romu's ClaudeCode on the other instance". */}
                  <Cpu size={10} /> {agentTrigger}
                  <span className="disc-msg-agent-kind"> · {agentIdentityLabel}</span>
                  {/* KT-247 — stable per-provider CLI ordinal, so two joined
                   *  Claude Code (or two Codex) are distinguishable in the
                   *  timeline. Matches the `@claude-cli-2` room alias. */}
                  {msg.author_cli_ordinal != null && (
                    <span
                      className="disc-msg-agent-cli"
                      style={{ opacity: 0.75, fontWeight: 600 }}
                      title={`Session ${agentTrigger}-cli${msg.author_cli_ordinal > 1 ? `-${msg.author_cli_ordinal}` : ''}`}
                    >
                      {' '}· CLI{msg.author_cli_ordinal > 1 ? ` ${msg.author_cli_ordinal}` : ''}
                    </span>
                  )}
                  {/* Prefer the CONCRETE model ("qwen3:32b", "sonnet") — a disc can
                   *  switch models mid-thread, so this is per-message. Fall back to
                   *  the tier when the model wasn't recorded (legacy rows / a
                   *  provider-default run with no explicit flag). */}
                  {(msg.model || msg.model_tier) && (
                    <span
                      style={{ opacity: 0.6, fontWeight: 400 }}
                      title={msg.model ? `Modèle : ${msg.model}` : `Palier : ${msg.model_tier}`}
                    >
                      · {msg.model ?? msg.model_tier}
                    </span>
                  )}
                  {msg.author_pseudo && (
                    <span
                      className="disc-msg-agent-peer"
                      style={{ opacity: 0.6, fontWeight: 400 }}
                      title={`Réponse fédérée du pair ${msg.author_pseudo}`}
                    >
                      · {msg.author_pseudo}
                    </span>
                  )}
                </span>
                {copyBtn(9, false)}
              </div>
            )}
            {msg.role === 'System' && isKronnTool && (
              <div className="disc-msg-kronn-tool" data-testid="kronn-tool-badge">
                {/* Wrench icon mirrors the `🔧 N` pill in ChatHeader so the
                 *  user makes the visual connection: pill counts these
                 *  badges. The tool name + args render compactly; if the
                 *  resolver returned a payload (slash-marker path) we
                 *  render it as a collapsed snippet on hover. */}
                <span className="disc-msg-kronn-tool-icon" aria-hidden="true">🔧</span>
                <span className="disc-msg-kronn-tool-label">
                  {kronnToolMatch ? (
                    <>
                      <code className="disc-msg-kronn-tool-name">{kronnToolMatch[1]}</code>
                      {kronnToolMatch[2] && (
                        <span className="disc-msg-kronn-tool-args">({kronnToolMatch[2]})</span>
                      )}
                    </>
                  ) : (
                    <span>{msg.content}</span>
                  )}
                </span>
                {kronnToolMatch && kronnToolMatch[3] && (
                  <details className="disc-msg-kronn-tool-result">
                    <summary>{t('disc.kronnToolResult')}</summary>
                    <pre>{kronnToolMatch[3]}</pre>
                  </details>
                )}
              </div>
            )}
            {msg.role === 'System' && !isKronnTool && (
              <div
                className="disc-msg-agent-label"
                style={{
                  color: isKronnPlanning
                    ? 'var(--kr-accent-ink)'
                    : msg.content.startsWith('summary cached')
                      ? 'var(--kr-success)'
                      : 'var(--kr-error)',
                }}
              >
                {executionContext
                  ? <ShieldCheck size={10} />
                  : isKronnPlanning
                  ? <ListTodo size={10} />
                  : msg.content.startsWith('summary cached')
                    ? <Zap size={10} />
                    : <AlertTriangle size={10} />}
                {' '}
                {executionContext
                  ? 'Execution context'
                  : isKronnPlanning
                  ? t('planning.receipt')
                  : msg.content.startsWith('summary cached')
                    ? t('disc.summaryCached')
                    : t('disc.system')}
                {(msg.agent_type || msg.model) && (
                  <span
                    className="disc-msg-attempted-model"
                    data-testid="disc-msg-attempted-model"
                    title={msg.model ? t('disc.attemptedModel', msg.model) : undefined}
                  >
                    · {msg.agent_type ?? defaultAgent}
                    {msg.model ? ` · ${msg.model}` : ''}
                  </span>
                )}
                {msg.content.startsWith('summary cached') && summaryCache && (
                  <button
                    className="disc-summary-toggle"
                    aria-label={t('disc.viewSummary')}
                    onClick={() => onExpandSummary(msg.id)}
                  >
                    {isExpandedSummary ? t('disc.hideSummary') : t('disc.viewSummary')}
                  </button>
                )}
              </div>
            )}
          </div>
          <div className="disc-msg-header-actions">
            {isOrchestrator && (
              <button
                type="button"
                className="disc-msg-orchestrator-toggle"
                aria-expanded={orchestratorDetailsOpen}
                onClick={() => setOrchestratorDetailsOpen(open => !open)}
              >
                {orchestratorDetailsOpen
                  ? t('disc.orchestratorHideDetails')
                  : t('disc.orchestratorShowDetails')}
              </button>
            )}
            <button
              type="button"
              className="disc-id-pill disc-message-id-pill"
              data-tour-id="message-id-pill"
              data-copied={isMessageIdCopied}
              onClick={copyMessageId}
              title={t('disc.idPillTooltip', msg.id)}
              aria-label={t('disc.idPillTooltip', msg.id)}
            >
              {isMessageIdCopied ? <Check size={8} /> : null}
              {messageShortLabel(msg.id)}
            </button>
          </div>
        </div>
        {msg.role !== 'System' && msg.reply_to_message_id && (
          <button
            type="button"
            className="disc-reply-header"
            data-missing={!replyTarget || undefined}
            onClick={() => {
              if (replyTarget && msg.reply_to_message_id) {
                onReplyNavigate?.(msg.reply_to_message_id);
              }
            }}
            disabled={!replyTarget || !onReplyNavigate}
            title={replyTarget ? t('disc.openReplyTarget') : t('disc.replyTargetMissing')}
          >
            <Reply size={11} aria-hidden="true" />
            <span>{replyTarget ? t('disc.inReplyTo') : t('disc.replyTargetMissing')}</span>
            <code>#{msg.reply_to_message_id.slice(0, 8)}</code>
            {replyTarget && <span>{t('disc.byAuthor', replyAuthor)}</span>}
          </button>
        )}
        {msg.role === 'System' && msg.content.startsWith('summary cached') && isExpandedSummary && summaryCache && (
          <div className="disc-summary-expanded">
            {summaryCache}
          </div>
        )}
        <div data-message-search-content>
        {isEditing ? (
          <div className="disc-edit-layout">
            <textarea
              ref={editTextareaRef}
              value={editingText}
              onChange={e => {
                onEditTextChange(e.target.value);
                const textarea = e.currentTarget;
                requestAnimationFrame(() => resizeEditTextarea(textarea));
              }}
              onKeyDown={e => {
                // IME guard: pressing Enter during an active composition
                // confirms the candidate, never re-submits the edit.
                if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  onEditSubmit();
                }
              }}
              className="disc-edit-textarea"
              aria-label={t('disc.editResend')}
              rows={1}
              autoFocus
            />
            <div className="disc-edit-actions">
              <button type="button" className="disc-icon-btn" style={{ fontSize: 11, padding: '4px 10px', color: 'var(--kr-text-faint)' }} onClick={onEditCancel}>{t('disc.cancel')}</button>
              <button type="button" className="disc-scan-btn" style={{ fontSize: 11, padding: '4px 10px' }} onClick={onEditSubmit} disabled={!editingText.trim()}>
                <Send size={10} /> {t('disc.resend')}
                <span className="text-2xs opacity-50" style={{ marginLeft: 4 }}>Ctrl+Enter</span>
              </button>
            </div>
          </div>
        ) : isKronnTool ? (
          // The kronn-tool badge above is the entire content. Skip
          // the markdown render — we don't want a second copy of
          // `[kronn-internal: ...]` rendered as raw text below.
          null
        ) : executionContext ? (
          <section className="disc-execution-context" aria-label={t('executionVariables.contextAria')}>
            <div><strong>{executionContext.run_kind}</strong> · {new Date(executionContext.resolved_at).toLocaleString()}</div>
            <div>{executionContext.purged ? t('executionVariables.purged') : t('executionVariables.encryptedMasked')}</div>
            <ul>
              {executionContext.variables.map(variable => (
                <li key={variable.name}>
                  <code>{variable.name}</code> · {variable.effective_source_ref}
                  {variable.overridden ? ` · ${t('executionVariables.manualOverride')}` : ''}
                  {' · '}
                  <code>{revealedExecutionVariables[variable.name] ?? '••••••'}</code>
                  {!executionContext.purged && (
                    <button
                      type="button"
                      className="disc-icon-btn"
                      aria-label={revealedExecutionVariables[variable.name]
                        ? t('executionVariables.remask', variable.name)
                        : t('executionVariables.revealTemporarily', variable.name)}
                      disabled={revealingExecutionVariable === variable.name}
                      onClick={() => revealedExecutionVariables[variable.name]
                        ? remaskExecutionVariable(variable.name)
                        : void revealExecutionVariable(variable.name)}
                    >
                      {revealingExecutionVariable === variable.name
                        ? <Loader2 size={12} className="spin" />
                        : revealedExecutionVariables[variable.name]
                          ? <EyeOff size={12} />
                          : <Eye size={12} />}
                    </button>
                  )}
                </li>
              ))}
            </ul>
            {executionVariableError && <div role="alert">{executionVariableError}</div>}
          </section>
        ) : modelError ? (
          <div className="disc-model-error-content" data-testid="disc-model-error-content">
            <p>{modelError.summary}</p>
            <details>
              <summary>{t('disc.modelErrorDetails')}</summary>
              <pre>{modelError.detail}</pre>
            </details>
          </div>
        ) : isMatrixLastUser && !decodeDone ? (
          // Matrix: render as plain scrambled text for ~700ms before
          // flipping to Markdown. Markdown parse is skipped during
          // decode — fewer re-renders through ReactMarkdown.
          <div className="matrix-text" data-decoding="true" style={{ whiteSpace: 'pre-wrap' }}>
            <MatrixText text={visibleContent} />
          </div>
        ) : (
          (() => {
            // 0.8.5 — split off the optional Kronn-internal seed payload
            // BEFORE we strip the KRONN:* signals. The seed itself may
            // contain a signal name inside instructions ("emit KRONN:QP_IMPROVED"),
            // and stripping those would corrupt the agent prompt. Splitting first
            // keeps the agent-bound full content intact in the DB while the UI
            // only renders the visible prefix.
            const { visible, seed } = splitMessageSeed(visibleContent);
            const cleaned = visible.replace(/KRONN:(BRIEFING_COMPLETE|VALIDATION_COMPLETE|BOOTSTRAP_COMPLETE|WORKFLOW_READY|REPO_READY|ARCHITECTURE_READY|PLAN_READY|STRUCTURE_READY|ISSUES_READY|ISSUES_CREATED|QP_IMPROVED|BUNDLE_READY|CHAIN_QP:[0-9a-fA-F-]+)/gi, '').trim();
            if (isOrchestrator) {
              return (
                <div className="disc-msg-orchestrator-content">
                  {!orchestratorDetailsOpen && (
                    <p className="disc-msg-orchestrator-preview">{orchestratorPreview}</p>
                  )}
                  {orchestratorDetailsOpen && (
                    <MentionDiscussionAgentContext.Provider value={defaultAgent}>
                      <MentionAwareMessageBody
                        content={cleaned}
                        discussionId={discussionId}
                      />
                    </MentionDiscussionAgentContext.Provider>
                  )}
                </div>
              );
            }
            // KT-251 — a salvaged fragment is folded behind a toggle. Two
            // half-answers next to a real one read as several agents replying,
            // which is what a user reported. Folded and NOT hidden: the fragment
            // is real history and may hold reasoning the retry never repeated.
            if (isFragment && !fragmentOpen) {
              return (
                <button
                  type="button"
                  className="disc-icon-btn disc-msg-fragment-toggle"
                  onClick={() => setFragmentOpen(true)}
                  aria-expanded={false}
                >
                  ▸ {t('disc.interruptedFragment')}
                </button>
              );
            }
            return (
              <>
                {isFragment && (
                  <button
                    type="button"
                    className="disc-icon-btn disc-msg-fragment-toggle"
                    onClick={() => setFragmentOpen(false)}
                    aria-expanded
                  >
                    ▾ {t('disc.interruptedFragment')}
                  </button>
                )}
                {isUser || msg.role === 'Agent'
                  ? (
                      <MentionDiscussionAgentContext.Provider value={defaultAgent}>
                        <MentionAwareMessageBody
                          content={cleaned}
                          discussionId={discussionId}
                          sourceMessageId={msg.role === 'Agent' ? msg.id : undefined}
                          sources={lint?.sources}
                        />
                      </MentionDiscussionAgentContext.Provider>
                    )
                  : (
                    <MessageBody
                      content={cleaned}
                      discussionId={discussionId}
                      sources={lint?.sources}
                    />
                  )}
                {seed && <KronnSeedToggle seed={seed} />}
              </>
            );
          })()
        )}
        </div>
        {attachments && attachments.length > 0 && discussionId && (
          <MessageAttachments
            files={attachments}
            discussionId={discussionId}
            t={t}
            carouselScope={discussionMedia}
          />
        )}
        {/* F15+ — federated file announced but not yet fetched/linked. Shows
         *  while the peer downloads the binary, replaced by the real attachment
         *  once the `file_attached{pending:false}` event lands + reload. */}
        {pendingAttachment && (!attachments || attachments.length === 0) && (
          <div className="disc-msg-attachment-pending" style={{ opacity: 0.7, fontSize: 'var(--kr-fs-xs)', marginTop: 4 }}>
            📎 {t('disc.attachmentDownloading')}
          </div>
        )}
        {msg.role === 'Agent' && onActionChanged && onOpenActionDiscussion && actions.map(action => (
          <DiscussionActionCard
            key={action.id}
            action={action}
            onChanged={onActionChanged}
            onOpenDiscussion={onOpenActionDiscussion}
          />
        ))}
        {msg.role === 'Agent' && (
          <button
            className="disc-tts-btn"
            onClick={() => onTts(msg.id, visibleContent, language)}
            title={isTtsActive ? (tts === 'loading' ? 'Chargement...' : tts === 'playing' ? 'Pause' : tts === 'paused' ? 'Reprendre' : 'Lire') : 'Lire \u00e0 voix haute'}
          >
            {isTtsActive && tts === 'loading' ? <><Loader2 size={9} style={{ animation: 'spin 1s linear infinite' }} /> TTS</>
              : isTtsActive && tts === 'playing' ? <><Pause size={9} /> Pause</>
              : isTtsActive && tts === 'paused' ? <><Play size={9} /> Reprendre</>
              : <><Play size={9} /> TTS</>}
          </button>
        )}
        {!modelError && RE_AUTH_ERROR.test(msg.content) && (
          <div className="disc-auth-error-cta">
            <button className="disc-scan-btn" style={{ fontSize: 11, padding: '5px 12px' }} onClick={() => onNavigate('settings')}>
              <Key size={11} /> {t('disc.overrideKey')}
            </button>
            <span className="disc-auth-error-hint">{t('disc.orCheckAgent')}</span>
          </div>
        )}
        {RE_PARTIAL_RESPONSE.test(msg.content) && (
          <div className="disc-auth-error-cta">
            <button className="disc-scan-btn" style={{ fontSize: 11, padding: '5px 12px', borderColor: 'rgba(var(--kr-warning-amber-rgb), 0.3)', background: 'rgba(var(--kr-warning-amber-rgb), 0.08)', color: 'var(--kr-warning-amber)' }} onClick={() => onNavigate('settings', { scrollTo: 'settings-server' })}>
              <Settings size={11} /> {t('disc.editTimeout')}
            </button>
          </div>
        )}
        {modelError && msg.agent_type && (
          <div className="disc-auth-error-cta">
            {retryDispatchId && !modelError.retried && onRetryAgentDispatch && (
              <button
                className="disc-scan-btn"
                data-testid="retry-agent-dispatch"
                style={{ fontSize: 11, padding: '5px 12px' }}
                onClick={() => onRetryAgentDispatch(retryDispatchId, errorAgentType)}
              >
                <RotateCcw size={11} />
                {t('disc.retryAgent', AGENT_LABELS[msg.agent_type] ?? msg.agent_type)}
              </button>
            )}
            {retryDispatchId && modelError.retried && (
              <span className="disc-auth-error-hint">{t('disc.agentRetryQueued')}</span>
            )}
            <button
              className="disc-scan-btn"
              style={{ fontSize: 11, padding: '5px 12px' }}
              onClick={() => {
                try {
                  sessionStorage.setItem('kronn:model-config-target', JSON.stringify({
                    agentType: msg.agent_type,
                    tier: modelError.tier,
                  }));
                } catch { /* private mode / quota: section navigation still works */ }
                onNavigate('settings', { scrollTo: 'settings-agent-config' });
              }}
            >
              <Settings size={11} />
              {t('disc.changeTierModel', t(`disc.tier.${modelError.tier}`))}
            </button>
            {modelError.status !== null && (
              <span className="disc-auth-error-hint">
                {t('disc.modelErrorHint', modelError.status)}
              </span>
            )}
          </div>
        )}
        {/* 0.8.3 — End-of-validation CTA. When the agent emits
            KRONN:VALIDATION_COMPLETE in a project-bound discussion,
            surface a one-click jump to the TD index. Pattern: same
            as the auth-error / timeout CTAs above (button below the
            bubble, before the footer). The hash + onNavigate combo
            re-uses Dashboard's existing `#project-<id>` deep-link so
            we don't need new plumbing in the Dashboard state machine. */}
        {projectId && /KRONN:VALIDATION_COMPLETE/i.test(msg.content) && (
          <div className="disc-auth-error-cta">
            <button
              className="disc-scan-btn"
              style={{ fontSize: 11, padding: '5px 12px' }}
              onClick={() => {
                // 0.8.3 (#314) — deep-link directly to the tech-debt folder.
                // Pre-fix the CTA only navigated to the project (via hash
                // #project-<id>) and the user landed on the AI Context
                // tab but had to manually expand + click into docs/tech-debt/.
                // The sessionStorage flag is read by ProjectCard on mount
                // and triggers `setExpandedTab('docAi') + setDocDeepLink`
                // automatically, so a single click takes the user from
                // "validation finished" to "looking at the TDs".
                try {
                  sessionStorage.setItem(`kronn:postValidation:${projectId}`, 'docs/tech-debt');
                } catch { /* private-mode / quota — fall through */ }
                window.location.hash = `#project-${projectId}`;
                onNavigate('projects');
              }}
            >
              <ShieldCheck size={11} /> {t('audit.viewTechDebtsAfterValidation')}
            </button>
          </div>
        )}
        {/* KRONN:CHAIN_QP:<id> — agent proposes a QP hand-off (e.g. triage →
            apply-framing). One click launches the QP in THIS discussion; the
            human click IS the gate, so no auto-fire. Disabled mid-stream so
            the launch can't race the in-flight turn. */}
        {chainQp && onLaunchQp && (
          <div className="disc-auth-error-cta">
            <button
              className="disc-scan-btn"
              style={{ fontSize: 11, padding: '5px 12px' }}
              disabled={sending}
              onClick={() => onLaunchQp(chainQp)}
            >
              <Zap size={11} /> {t('disc.launchProposedQp', `${chainQp.icon ?? '⚡'} ${chainQp.name}`)}
            </button>
          </div>
        )}
        <div className="disc-msg-footer">
          <div className="disc-msg-time-row">
            <span className="disc-msg-time">{formattedTime}</span>
            {msg.tokens_used > 0 && <span className="disc-msg-token-count">{msg.tokens_used.toLocaleString()} tok</span>}
            {/* KT-190 — a joined CLI's spend cannot be cut per message: between
                two room messages it also reads files, runs tests, and may answer
                in another room. So this is the SESSION's running total at this
                point, labelled as such and never rendered where a per-message
                cost goes. Kronn never spawned that CLI, so `tokens_used` above
                stays 0 for it — the two are different facts, not two formats. */}
            {typeof msg.session_tokens_at_message === 'number'
              && msg.session_tokens_at_message > 0 && (
              <span
                className="disc-msg-session-tokens"
                title={
                  'Total de la session CLI à ce message, pas le coût de ce '
                  + 'message : entre deux messages la CLI a aussi lu des '
                  + 'fichiers et lancé des tests. Croissant par construction.'
                }
              >
                session&nbsp;: {msg.session_tokens_at_message.toLocaleString()} tok
              </span>
            )}
            {msg.auth_mode && <span className="disc-msg-auth-mode" data-mode={msg.auth_mode === 'override' ? 'override' : 'local'}>{msg.auth_mode === 'override' ? 'API key' : 'auth locale'}</span>}
            {durationLabel && <span className="disc-msg-duration"><Clock size={8} /> {durationLabel}</span>}
            {msg.role === 'Agent' && lintSeverity && lint && (
              <button
                type="button"
                className="disc-msg-lint-pill"
                data-severity={lintSeverity}
                onClick={() => setShowLint(v => !v)}
                aria-expanded={showLint}
                aria-controls={`lint-detail-${msg.id}`}
                title={t(lintSeverity === 'verified' ? 'disc.lintPillHintVerified' : 'disc.lintPillHint')}
                data-testid="lint-pill"
              >
                {lintSeverity === 'verified' ? <Check size={8} /> : <AlertTriangle size={8} />}{' '}
                {lintSeverity === 'fabricated'
                  ? <>{lint.fabricated_count} {t('disc.lintFabricated')}</>
                  : lintSeverity === 'unsourced'
                    ? <>{lint.unsourced_count} {t('disc.lintUnsourced')}</>
                    : lintSeverity === 'unverified'
                      ? <>{unverifiedCount} {t('disc.lintUnverified')}</>
                      : lintSeverity === 'unchecked'
                        ? <>{unverifiableCount} {t('disc.lintUnverifiable')}</>
                        : <>{verifiedCount} {t('disc.lintVerified')}</>}
              </button>
            )}
            {/* When the headline pill is a WARNING but the reply also has
                verified citations, show the verified count alongside so the
                footer never hides the good news (e.g. "1 sans source" + "14
                vérifiée(s)"). Both chips open the same detail drawer. */}
            {msg.role === 'Agent' && lint && lintSeverity && lintSeverity !== 'verified' && verifiedCount > 0 && (
              <button
                type="button"
                className="disc-msg-lint-pill"
                data-severity="verified"
                onClick={() => setShowLint(v => !v)}
                aria-expanded={showLint}
                aria-controls={`lint-detail-${msg.id}`}
                title={t('disc.lintPillHintVerified')}
                data-testid="lint-pill-verified"
              >
                <Check size={8} /> {verifiedCount} {t('disc.lintVerified')}
              </button>
            )}
          </div>
          <div className="disc-msg-footer-right">
            {replies.filter(reply => reply.role !== 'System').map(reply => {
              const agentMention = reply.agent_type
                ? AGENT_MENTIONS.find(mention => mention.type === reply.agent_type)?.trigger
                : null;
              const humanMention = reply.author_pseudo
                ? `@${reply.author_pseudo.replace(/^@/, '')}`
                : '@user';
              const author = agentMention ?? humanMention;
              const excerpt = stripAgentHandoff(reply.content).replace(/\s+/g, ' ').trim();
              return (
                <button
                  key={reply.id}
                  type="button"
                  className="disc-reply-backlink"
                  onClick={() => onReplyNavigate?.(reply.id)}
                  disabled={!onReplyNavigate}
                  title={t('disc.openReply', excerpt.slice(0, 120))}
                >
                  <Check size={8} aria-hidden="true" />
                  {t('disc.repliedBy', author)}
                </button>
              );
            })}
            {msg.role !== 'System' && !isEditing && onReply && (
              <button
                type="button"
                className="disc-icon-btn disc-reply-action"
                onClick={() => onReply(msg)}
                title={t('disc.reply')}
                aria-label={t('disc.reply')}
              >
                <Reply size={10} />
                <span>{t('disc.reply')}</span>
              </button>
            )}
            {!isEditing && onDelete && (
              <button
                type="button"
                className="disc-icon-btn disc-delete-message-action"
                onClick={() => onDelete(msg)}
                disabled={isDeleting || sending}
                title={t('disc.deleteMessage')}
                aria-label={t('disc.deleteMessage')}
              >
                {isDeleting ? <Loader2 size={10} className="spin" /> : <Trash2 size={10} />}
              </button>
            )}
            {msg.role === 'Agent' && copyBtn(9, true)}
            {msg.role === 'Agent' && hasFullAccess && (
              <span className="disc-full-access-badge">
                <AlertTriangle size={8} /> {t('config.fullAccessBadge')}
              </span>
            )}
            {msg.role === 'Agent' && msg.model_tier && (
              <span
                className="disc-model-tier-badge"
                data-tier={msg.model_tier}
                title={msg.model ? `${msg.model} (${t(`disc.tier.${msg.model_tier}`)})` : undefined}
              >
                {msg.model_tier === 'economy' ? '⚡' : '\ud83e\udde0'} {t(`disc.tier.${msg.model_tier}`)}
              </span>
            )}
            {!sending && !isEditing && (isLastUser || isLastAgent) && (
              <div className="flex-row gap-2">
                {isLastUser && !isOrchestrator && (
                  <button className="disc-icon-btn" style={{ padding: '2px 6px', fontSize: 10, color: 'var(--kr-text-dim)' }} onClick={() => onEditStart(msg.id, visibleContent)} title={t('disc.editResend')} aria-label={t('disc.editResend')}>
                    <Pencil size={10} />
                  </button>
                )}
                {isLastAgent && (
                  <button className="disc-icon-btn" style={{ padding: '2px 6px', fontSize: 10, color: 'var(--kr-text-dim)' }} onClick={onRetry} title={t('disc.retryResponse')} aria-label={t('disc.retryResponse')}>
                    <RotateCcw size={10} />
                  </button>
                )}
              </div>
            )}
          </div>
        </div>
        {showLint && lint && (
          <div className="disc-msg-lint-detail" data-severity={lintSeverity} id={`lint-detail-${msg.id}`} data-testid="lint-detail">
            {lint.sources.some(s => s.status !== 'verified' && s.status !== 'unchecked') && (
              <div className="disc-lint-group">
                <div className="disc-lint-group-title">{t('disc.lintSourcesTitle')}</div>
                {lint.sources
                  .filter(s => s.status !== 'verified' && s.status !== 'unchecked')
                  .map((s, i) => (
                    <div key={`src-${i}`} className="disc-lint-item" data-status={s.status}>
                      <code>{s.raw}</code> — {s.detail}
                    </div>
                  ))}
              </div>
            )}
            {/* Inline anchors the agent emitted that did NOT resolve — honest
                "couldn't verify" (typo? cross-repo? wrong line?), distinct from
                the red "fabricated" formal citations above. Status is
                'unchecked' so they stay out of the red bucket; we surface them
                by their detail marker. */}
            {lint.sources.some(s => s.status === 'unchecked' && s.detail.includes("couldn't verify")) && (
              <div className="disc-lint-group" data-testid="lint-unverified-group">
                <div className="disc-lint-group-title">{t('disc.lintUnverifiedTitle')}</div>
                {lint.sources
                  .filter(s => s.status === 'unchecked' && s.detail.includes("couldn't verify"))
                  .map((s, i) => (
                    <div key={`unv-${i}`} className="disc-lint-item" data-status="unverified">
                      <code>{s.raw}</code> — {s.detail}
                    </div>
                  ))}
              </div>
            )}
            {/* Positive list — every source that resolved on disk, so the user
                can audit *what* was verified. Shown whenever there's ≥1, even
                on a mixed (e.g. unverified-severity) report. */}
            {verifiedCount > 0 && (
              <div className="disc-lint-group" data-testid="lint-verified-group">
                <div className="disc-lint-group-title">{t('disc.lintVerifiedTitle')}</div>
                {lint.sources
                  .filter(s => s.status === 'verified')
                  .map((s, i) => (
                    <div key={`vsrc-${i}`} className="disc-lint-item" data-status="verified">
                      <code>{s.raw}</code> — {s.detail}
                    </div>
                  ))}
              </div>
            )}
            {lint.flagged_spans.length > 0 && (
              <div className="disc-lint-group">
                <div className="disc-lint-group-title">{t('disc.lintUnsourcedTitle')}</div>
                {lint.flagged_spans.map((sp, i) => (
                  <div key={`span-${i}`} className="disc-lint-item">“{sp.text}”</div>
                ))}
              </div>
            )}
            {/* Sources that can't be machine-checked (URL / user-confirmed /
                inferred / commit / hypothesis). Listed honestly so the user
                sees "what couldn't be tested" — Option B, never hidden. */}
            {lint.sources.some(s => s.status === 'unchecked' && !s.detail.includes("couldn't verify")) && (
              <div className="disc-lint-group" data-testid="lint-unverifiable-group">
                <div className="disc-lint-group-title">{t('disc.lintUnverifiableTitle')}</div>
                {lint.sources
                  .filter(s => s.status === 'unchecked' && !s.detail.includes("couldn't verify"))
                  .map((s, i) => (
                    <div key={`unc-${i}`} className="disc-lint-item" data-status="unchecked">
                      <code>{s.raw}</code> — {s.detail}
                    </div>
                  ))}
              </div>
            )}
            <div className="disc-lint-caveat">{t('disc.lintCaveat')}</div>
          </div>
        )}
      </div>
    </div>
  );
});

/// A `[src: url: https://…]` marker is torn in three by the markdown
/// autolinker: the opener, an `<a>`, and the closing bracket. Put back
/// together it is one marker again — otherwise the only citations that stay
/// unreadable are the ones that name a source outside the code.
function joinAutolinkedMarkers(nodes: ReactNode[]): ReactNode[] {
  const OPENER = /\[src:[ \t]*[^\][\n]*$/;
  const joined: ReactNode[] = [];
  for (let index = 0; index < nodes.length; index += 1) {
    const node = nodes[index];
    const link = nodes[index + 1] as { props?: { href?: string } } | undefined;
    const tail = nodes[index + 2];
    if (
      typeof node === 'string' && OPENER.test(node)
      && typeof link?.props?.href === 'string'
      && typeof tail === 'string' && tail.startsWith(']')
    ) {
      joined.push(`${node}${link.props.href}]`);
      // The bracket is consumed with the link; the rest of that run is prose.
      joined.push(tail.slice(1));
      index += 2;
      continue;
    }
    joined.push(node);
  }
  return joined;
}

/// Replace every `[src: …]` marker inside a run of rendered children with a
/// chip. Only string children are touched: anything ReactMarkdown already
/// turned into an element — a link, code, emphasis — is left exactly as it is.
function withSourceChips(children: ReactNode, sources?: SourceCheck[]): ReactNode {
  const nodes = joinAutolinkedMarkers(Array.isArray(children) ? children : [children]);
  if (!nodes.some(node => typeof node === 'string' && hasSourceCitation(node))) {
    return children;
  }
  return nodes.map((node, index) => {
    if (typeof node !== 'string' || !hasSourceCitation(node)) return node;
    return splitSourceCitations(node).map((part, partIndex) => (
      'citation' in part
        ? (
          <SourceCitationChip
            key={`${index}-${partIndex}`}
            citation={part.citation}
            sources={sources}
          />
        )
        : <Fragment key={`${index}-${partIndex}`}>{part.text}</Fragment>
    ));
  });
}

// ─── MarkdownContent component ───────────────────────────────────────────────

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
function CopyableBlock({ children, className, tag }: { children: ReactNode; className?: string; tag: 'table' | 'pre' }) {
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
    <div ref={ref} className={`relative ${className || ''}`}>
      {children}
      <button
        onClick={handleCopy}
        className="disc-copyable-block-btn"
        data-copied={copied}
        onMouseEnter={e => (e.currentTarget.style.opacity = '1')}
        onMouseLeave={e => (e.currentTarget.style.opacity = '0.6')}
      >
        {copied ? <>{'\u2713'}</> : <>{'\u2398'}</>}
      </button>
    </div>
  );
}

// react-markdown's component override prop type is essentially
// `{ children?: ReactNode; ...HTMLAttributes }` with a few extras the
// remark plugins inject (node, index, etc.). We don't need them — only
// `children`, `href` and `className` — so a narrow shape is enough and
// avoids 16 `any`s.
type MdProps = {
  children?: ReactNode;
  className?: string;
  href?: string;
};

function UserMentionChip() {
  // Show WHO is addressed: the configured Kronn pseudo and Gravatar. Falling
  // back to the canonical "@user" label keeps the chip meaningful on an
  // instance where no identity was set.
  const { pseudo, avatarEmail } = useLocalIdentity();
  return (
    <span
      className="disc-user-mention-chip"
      data-mention="user"
      title={pseudo ?? undefined}
    >
      {avatarEmail ? (
        <img
          src={gravatarUrl(avatarEmail, 20)}
          alt=""
          className="disc-user-mention-avatar"
        />
      ) : (
        <User size={10} aria-hidden="true" />
      )}
      {pseudo ? `@${pseudo}` : USER_MENTION_TRIGGER}
    </span>
  );
}

function MarkdownLink({ href, children }: MdProps) {
  const { mentionColors } = useLocalIdentity();
  const { t } = useT();
  const discussionAgent = useContext(MentionDiscussionAgentContext);
  if (href === USER_MENTION_URL) {
    return <UserMentionChip />;
  }
  if (href === '#kronn-all') {
    return (
      <span className="disc-agent-mention-chip disc-all-mention-chip">
        <Users size={10} aria-hidden="true" />
        {children}
      </span>
    );
  }
  const encodedAgent = href?.startsWith('#kronn-agent-')
    ? href.slice('#kronn-agent-'.length)
    : null;
  const agentType = encodedAgent
    ? encodedAgent.replace(/-cli$/, '') as AgentType
    : null;
  const mention = agentType
    ? AGENT_MENTIONS.find(candidate => candidate.type === agentType)
    : null;
  if (mention) {
    const color = agentColor(mention.type, mentionColors);
    const textColor = agentTextColor(mention.type, mentionColors);
    const isCli = encodedAgent?.endsWith('-cli') ?? false;
    const identityKind = !isCli && discussionAgent
      ? t(mention.type === discussionAgent
          ? 'disc.targetDiscussionAgent'
          : 'disc.targetPunctualAgent')
      : null;
    return (
      <span
        className="disc-agent-mention-chip"
        data-agent={mention.type}
        style={{ color: textColor, borderColor: color, backgroundColor: `${color}18` }}
      >
        <Cpu size={10} aria-hidden="true" />
        {children}
        {identityKind && (
          <span className="disc-agent-mention-kind"> · {identityKind}</span>
        )}
      </span>
    );
  }
  return <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>;
}

const mdComponents = {
  p: ({ children }: MdProps) => <p>{children}</p>,
  h1: ({ children }: MdProps) => <h1>{children}</h1>,
  h2: ({ children }: MdProps) => <h2>{children}</h2>,
  h3: ({ children }: MdProps) => <h3>{children}</h3>,
  ul: ({ children }: MdProps) => <ul>{children}</ul>,
  ol: ({ children }: MdProps) => <ol>{children}</ol>,
  li: ({ children }: MdProps) => <li>{children}</li>,
  code: ({ className, children }: MdProps) => {
    const isBlock = className?.includes('language-');
    return isBlock
      ? <code className="disc-md-pre-code">{children}</code>
      : <code>{children}</code>;
  },
  pre: ({ children }: MdProps) => (
    <CopyableBlock tag="pre">
      <pre>{children}</pre>
    </CopyableBlock>
  ),
  table: ({ children }: MdProps) => (
    <CopyableBlock tag="table" className="overflow-hidden">
      <table>{children}</table>
    </CopyableBlock>
  ),
  th: ({ children }: MdProps) => <th>{children}</th>,
  td: ({ children }: MdProps) => <td>{children}</td>,
  blockquote: ({ children }: MdProps) => <blockquote>{children}</blockquote>,
  hr: () => <hr />,
  a: MarkdownLink,
  strong: ({ children }: MdProps) => <strong>{children}</strong>,
};

// `remark-emoji` transforms GitHub-style shortcodes (`:tada:`) into the
// actual Unicode character during markdown parsing. Messages are stored
// with the shortcode intact — the conversion is purely visual, so agents
// and full-text search still see `tada` in the DB. Keeps the agent prompt
// portable (some CLIs choke on raw multi-byte emoji sequences).
const remarkPluginsList = [remarkGfm, remarkEmoji];
const mentionRemarkPluginsList = [remarkGfm, remarkEmoji, remarkAgentMentions];

/** Above this, a message is NOT sent through ReactMarkdown + remark-gfm +
 *  syntax highlight: those are super-linear in input size and a multi-MB
 *  message blows up the heap and CRASHES the browser tab. (2026-06-23: a
 *  killed Codex run persisted a 2.4 MB stderr/reasoning dump as its reply;
 *  opening that discussion froze then crashed Chrome.) ~200 KB is far above
 *  any normal agent reply (triage replies are < 10 KB) so this only ever
 *  trips on pathological dumps. */
const MAX_MARKDOWN_CHARS = 200_000;
/** How much of an oversized message we render inline as plain text. The rest
 *  is reachable via "copy full message" — plain text is cheap, but we still
 *  cap the DOM so a 50 MB dump can't lag the page either. */
const LARGE_MSG_DISPLAY_CHARS = 100_000;

/** Plain-text fallback for a pathologically large message — no markdown, no
 *  highlight, so it renders instantly instead of crashing the tab. */
const LargeMessageFallback = memo(({ content }: { content: string }) => {
  const { t } = useT();
  const kb = Math.round(content.length / 1024);
  const truncated = content.length > LARGE_MSG_DISPLAY_CHARS;
  const shown = truncated ? content.slice(0, LARGE_MSG_DISPLAY_CHARS) : content;
  return (
    <div className="disc-md">
      <div
        role="note"
        style={{
          display: 'flex', alignItems: 'center', gap: 6,
          fontSize: 11, color: 'var(--kr-text-secondary)',
          background: 'var(--kr-bg-subtle, rgba(128,128,128,0.08))',
          border: '1px solid var(--kr-border-faint)', borderRadius: 4,
          padding: '4px 8px', marginBottom: 6,
        }}
      >
        <AlertTriangle size={12} />
        <span>{t('disc.largeMessage', kb.toLocaleString())}</span>
      </div>
      <pre style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word', margin: 0 }}>
        {shown}{truncated ? `\n\n${t('disc.largeMessageTruncated')}` : ''}
      </pre>
    </div>
  );
});

interface MarkdownRenderContextValue {
  discussionId?: string;
  sourceMessageId?: string;
  sources?: SourceCheck[];
  proposalFenceLines: number[];
  questionFenceLines: number[];
}

// KT-622 — renderer functions are component TYPES, not ordinary callbacks.
// Recreating `pre` on a fresh lint array or streamed text remounts every form
// below it. Keep types stable and deliver changing values through context.
const MarkdownRenderContext = createContext<MarkdownRenderContextValue>({
  proposalFenceLines: [],
  questionFenceLines: [],
});

function MarkdownParagraph({ children }: MdProps) {
  const { sources } = useContext(MarkdownRenderContext);
  return <p>{withSourceChips(children, sources)}</p>;
}

function MarkdownListItem({ children }: MdProps) {
  const { sources } = useContext(MarkdownRenderContext);
  return <li>{withSourceChips(children, sources)}</li>;
}

function parseFenceJson(source: string): unknown {
  try {
    return JSON.parse(source);
  } catch {
    return null;
  }
}

function MarkdownPre({ children, node }: {
  children?: ReactNode;
  node?: { position?: { start?: { line?: number } } };
}) {
  const { discussionId, sourceMessageId, proposalFenceLines, questionFenceLines } = useContext(MarkdownRenderContext);
  if (discussionId) {
    // ReactMarkdown supplies <pre><code>…</code></pre>; inspect the child's
    // language class without treating an arbitrary ReactNode as an element.
    const childEl = Array.isArray(children) ? children[0] : children;
    const codeEl = childEl as { props?: { className?: string; children?: ReactNode } } | undefined;
    const className = codeEl?.props?.className ?? '';
    const raw = codeEl?.props?.children;
    const source = Array.isArray(raw) ? raw.join('') : String(raw ?? '');
    if (className.includes('language-mermaid')) {
      return <MermaidDiagram source={source.trim()} />;
    }
    if (className.includes('language-kronn-doc-preview')) {
      return <DocPreview html={source.trim()} discussionId={discussionId} />;
    }
    if (className.includes('language-kronn-doc-data')) {
      // Parse only data inside try/catch, never JSX. Malformed/unsupported
      // envelopes still fall through to the ordinary copyable code block.
      const parsed = parseFenceJson(source);
      if (parsed && typeof parsed === 'object' && 'format' in parsed) {
        const { format, ...payload } = parsed;
        if (format === 'csv' || format === 'xlsx' || format === 'pptx') {
          return <DocDataExport payload={payload} format={format} discussionId={discussionId} />;
        }
      }
    }
    // Cards read the durable row. A fresh context must update the same card;
    // a different message/fence must not inherit the previous human's draft.
    if (className.includes('language-kronn-question')) {
      const line = node?.position?.start?.line;
      const fenceIndex = line === undefined ? undefined : questionFenceLines.indexOf(line);
      return (
        <DiscussionQuestionCard
          key={JSON.stringify([discussionId, sourceMessageId, fenceIndex])}
          discussionId={discussionId}
          source={source.trim()}
          sourceMessageId={sourceMessageId}
          fenceIndex={fenceIndex !== undefined && fenceIndex >= 0 ? fenceIndex : undefined}
        />
      );
    }
    if (className.includes('language-kronn-plan-action')) {
      const proposal = parsePlanningProposal(parseFenceJson(source));
      if (proposal) {
        const line = node?.position?.start?.line;
        const fenceIndex = line === undefined ? undefined : proposalFenceLines.indexOf(line);
        return (
          <PlanningActionCard
            key={JSON.stringify([discussionId, sourceMessageId, fenceIndex])}
            proposal={proposal}
            discussionId={discussionId}
            sourceMessageId={sourceMessageId}
            fenceIndex={fenceIndex !== undefined && fenceIndex >= 0 ? fenceIndex : undefined}
          />
        );
      }
    }
  }
  return <CopyableBlock tag="pre"><pre>{children}</pre></CopyableBlock>;
}

const discussionMdComponents = {
  ...mdComponents,
  p: MarkdownParagraph,
  li: MarkdownListItem,
  pre: MarkdownPre,
};

export const MarkdownContent = memo(({
  content,
  discussionId,
  sourceMessageId,
  agentMentions = false,
  sources,
}: {
  content: string;
  discussionId?: string;
  sourceMessageId?: string;
  agentMentions?: boolean;
  /** The backend's citation verdicts; refreshes must not remount forms. */
  sources?: SourceCheck[];
}) => {
  const proposalFenceLines = useMemo(() => {
    const lines: number[] = [];
    content.split('\n').forEach((line, index) => {
      if (/^\s*`{3,}\s*kronn-plan-action\s*$/.test(line)) lines.push(index + 1);
    });
    return lines;
  }, [content]);

  // A durable question is addressed by its message and rank among fences.
  const questionFenceLines = useMemo(() => {
    const lines: number[] = [];
    content.split('\n').forEach((line, index) => {
      if (/^\s*`{3,}\s*kronn-question\s*$/.test(line)) lines.push(index + 1);
    });
    return lines;
  }, [content]);

  const renderContext = useMemo(() => ({
    discussionId, sourceMessageId, sources, proposalFenceLines, questionFenceLines,
  }), [discussionId, sourceMessageId, sources, proposalFenceLines, questionFenceLines]);

  // Guard against multi-MB messages crashing the tab — see MAX_MARKDOWN_CHARS.
  // Placed AFTER all hooks (the useMemo above) to satisfy rules-of-hooks.
  if (content.length > MAX_MARKDOWN_CHARS) {
    return <LargeMessageFallback content={content} />;
  }

  return (
    <MarkdownRenderContext.Provider value={renderContext}>
      <div className="disc-md">
        <ReactMarkdown
          remarkPlugins={agentMentions ? mentionRemarkPluginsList : remarkPluginsList}
          components={discussionMdComponents}
        >
          {content}
        </ReactMarkdown>
      </div>
    </MarkdownRenderContext.Provider>
  );
});

/** 2026-06-24 — a message segment: either the QP's own instructions (`text`)
 *  or a block of context Kronn injected at variable-substitution time
 *  (`context`, e.g. a ticket payload), wrapped server-side in a
 *  `<!-- kronn:context title="…" -->…<!-- /kronn:context -->` marker. */
/** Collapsible card for a block of injected context (a ticket, a file, …).
 *  Collapsed by default so the agent's instructions stay the visible signal;
 *  click to expand the (markdown-rendered) payload. */
const InjectedContextCard = memo(({ title, body, discussionId }: { title: string; body: string; discussionId?: string }) => {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const lineCount = useMemo(() => body.trim().split('\n').length, [body]);
  return (
    <div className="disc-injected-context">
      <button
        type="button"
        className="disc-injected-context-toggle"
        aria-expanded={open}
        onClick={() => setOpen(o => !o)}
      >
        <ChevronRight size={12} className="wf-chevron" data-expanded={open} />
        <span className="disc-injected-context-label">📋 {t('disc.injectedContext')}{title ? ` · ${title}` : ''}</span>
        {!open && <span className="disc-injected-context-meta">{t('disc.injectedContextLines', lineCount)}</span>}
      </button>
      {open && (
        <div className="disc-injected-context-body">
          <MarkdownContent content={body} discussionId={discussionId} />
        </div>
      )}
    </div>
  );
});

/** Render a message body: instructions as markdown, each injected-context
 *  block as a collapsible card. The fast path (no marker) is byte-identical
 *  to the previous direct `<MarkdownContent>` render. */
export const MessageBody = memo(({
  content,
  discussionId,
  sourceMessageId,
  agentMentions = false,
  sources,
}: {
  content: string;
  discussionId?: string;
  sourceMessageId?: string;
  agentMentions?: boolean;
  sources?: SourceCheck[];
}) => {
  const segs = useMemo(() => splitInjectedContext(content), [content]);
  if (segs.length === 1 && segs[0].kind === 'text') {
    return (
      <MarkdownContent
        content={content}
        discussionId={discussionId}
        sourceMessageId={sourceMessageId}
        agentMentions={agentMentions}
        sources={sources}
      />
    );
  }
  return (
    <>
      {segs.map((s, i) =>
        s.kind === 'context'
          ? <InjectedContextCard key={i} title={s.title} body={s.body} discussionId={discussionId} />
          : (s.body.trim()
              ? (
                  <MarkdownContent
                    key={i}
                    content={s.body}
                    discussionId={discussionId}
                    sourceMessageId={sourceMessageId}
                    agentMentions={agentMentions}
                    sources={sources}
                  />
                )
              : null),
      )}
    </>
  );
});

const MentionAwareMessageBody = memo(({
  content,
  discussionId,
  sourceMessageId,
  sources,
}: {
  content: string;
  discussionId?: string;
  sourceMessageId?: string;
  sources?: SourceCheck[];
}) => {
  return (
    <MessageBody
      content={content}
      discussionId={discussionId}
      sourceMessageId={sourceMessageId}
      agentMentions
      sources={sources}
    />
  );
});

// ─── Inline style constants removed — all styles now in DiscussionsPage.css ──
