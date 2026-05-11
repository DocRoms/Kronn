import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Bot, X, Send, Sparkles, Loader2, Minus, Maximize2, ChevronDown, ClipboardPaste, Link2, MessageSquareText } from 'lucide-react';
import '../aiHelper.css';
import { discussions as discussionsApi } from '../../lib/api';
import { AGENT_LABELS, agentColor } from '../../lib/constants';
import { t as translate, type UILocale } from '../../lib/i18n';
import type { AgentType, McpServer, WorkflowStep } from '../../types/generated';
import { authSlotsForServer, managedHeaderNames, managedQueryNames, stripManagedHeaders, stripManagedQuery } from './apiCallAuth';
import { tipsForSlug } from './apiCallPluginTips';

type Translator = (key: string, ...args: (string | number)[]) => string;

/** Coerce a backend "output language" string to a UI locale supported by the
 *  i18n dictionary. Backend allows fr/en/es/zh/br but only fr/en/es have
 *  translations; for the rest we fall back to English (the most universal
 *  language that all dictionaries cover). The agent will still reply in the
 *  full backend language because the discussion's `language` param is
 *  forwarded as-is. */
function toUILocale(lang: string | undefined): UILocale {
  if (lang === 'fr' || lang === 'en' || lang === 'es') return lang;
  return 'en';
}

export interface ApiCallAiHelperProps {
  step: WorkflowStep;
  /** Apply a partial update to the parent step. */
  onApply: (updates: Partial<WorkflowStep>) => void;
  /** Currently selected plugin (used for the system prompt). */
  selectedServer: McpServer | null;
  /** Project the step belongs to — passed through as `null` when absent.
   *  The helper does NOT require a project; it just forwards what it gets. */
  projectId: string | null;
  /** Agent types installed locally — used to populate the picker. */
  installedAgents: AgentType[];
  /** Last successful "Test the call" response body (any shape). Re-injected
   *  into every outgoing user message so the agent can reason about the
   *  actual JSON the API returned, not just guess from the spec. */
  lastTestResponse?: unknown;
  /** Last "Test the call" error message (HTTP status + body excerpt). When
   *  present, the helper forwards it so the agent can debug "why 400 ?". */
  lastTestError?: string | null;
  /** Backend "output language" (Settings → Output language). Drives the
   *  language of the system prompt + the discussion's `language` param
   *  (which the backend uses to instruct the agent on its reply language).
   *  Distinct from the UI `t` translator, which targets the user's
   *  interface labels — `t` stays UI-locale, the agent stays config-locale. */
  configLanguage?: string;
  t: (key: string, ...args: (string | number)[]) => string;
}

// 0.8.1 UX redesign: collapsed from 3 phases (closed / picking-agent /
// chatting) to 2. The agent picker is no longer a separate modal phase:
// the chat opens immediately on the first installed agent, and the
// header dropdown lets the user switch on the fly. Reduces friction:
// "click trigger → see chat" instead of "click → pick → see chat".
type Phase = 'closed' | 'chatting';

interface ChatMessage {
  role: 'user' | 'assistant';
  text: string;
}

interface ApplySuggestion {
  /** Position in the streaming text — used to deduplicate when we re-parse. */
  signature: string;
  parsed: Record<string, unknown>;
  applied: boolean;
}

// Matches blocks of the form
//   KRONN:APPLY
//   ```json
//   { ... }
//   ```
// The whitespace is intentionally permissive — agents tend to format slightly
// differently each time, but the marker word + fenced JSON pair is the
// invariant we lock onto.
const KRONN_APPLY_RX = /KRONN:APPLY\s*```json\s*([\s\S]*?)```/g;

/** Extract the structured suggestion blocks from a streaming assistant
 *  message. Malformed JSON is silently skipped — the agent will get the
 *  formatting right on the next attempt and we don't want to spam the user
 *  with parse errors. Exported only so the parser can be unit-tested
 *  directly; the React component uses it internally. */
export function parseApplyBlocks(text: string): ApplySuggestion[] {
  const out: ApplySuggestion[] = [];
  for (const m of text.matchAll(KRONN_APPLY_RX)) {
    try {
      const parsed = JSON.parse(m[1]) as Record<string, unknown>;
      out.push({ signature: m[1].trim(), parsed, applied: false });
    } catch {
      /* incomplete or malformed block — wait for the next chunk */
    }
  }
  return out;
}

/** Map a parsed KRONN:APPLY object onto a `WorkflowStep` partial. We
 *  intentionally only accept the documented surface (endpoint, method, query,
 *  headers, body, extract) so a hallucinating agent cannot rewrite arbitrary
 *  fields like `agent` or `prompt_template`.
 *
 *  When `server` is provided, auth-managed slots (e.g. Chartbeat's `apikey`
 *  query param) are stripped silently. The backend already injects them
 *  from the plugin's encrypted env at request build time — letting an agent
 *  push `apikey: 'VOTRE_API_KEY'` into the user's step would shadow the
 *  real value and produce a 401 the user can't easily diagnose. */
export function applyToStep(
  parsed: Record<string, unknown>,
  step: WorkflowStep,
  server: McpServer | null = null,
): Partial<WorkflowStep> {
  const updates: Partial<WorkflowStep> = {};
  const managedQ = managedQueryNames(server);
  const managedH = managedHeaderNames(server);

  if (typeof parsed.endpoint === 'string') {
    updates.api_endpoint_path = parsed.endpoint;
  }
  if (typeof parsed.method === 'string') {
    updates.api_method = parsed.method.toUpperCase();
  }
  if (parsed.query && typeof parsed.query === 'object' && !Array.isArray(parsed.query)) {
    const raw: Record<string, string> = {};
    for (const [k, v] of Object.entries(parsed.query as Record<string, unknown>)) {
      raw[k] = typeof v === 'string' ? v : JSON.stringify(v);
    }
    updates.api_query = stripManagedQuery(raw, managedQ);
  }
  if (parsed.headers && typeof parsed.headers === 'object' && !Array.isArray(parsed.headers)) {
    const raw: Record<string, string> = {};
    for (const [k, v] of Object.entries(parsed.headers as Record<string, unknown>)) {
      raw[k] = typeof v === 'string' ? v : JSON.stringify(v);
    }
    updates.api_headers = stripManagedHeaders(raw, managedH);
  }
  if (parsed.body !== undefined && parsed.body !== null) {
    updates.api_body = typeof parsed.body === 'string' ? parsed.body : JSON.stringify(parsed.body);
  }
  if (typeof parsed.extract === 'string') {
    updates.api_extract = {
      path: parsed.extract,
      fallback: step.api_extract?.fallback ?? null,
      fail_on_empty: step.api_extract?.fail_on_empty ?? false,
    };
  }
  return updates;
}

/** Truncate a JSON value when echoed into a prompt. The agent doesn't need
 *  the entire 500-page Jira response — first ~1.5kB is plenty to recognise
 *  the shape and pick fields. */
function truncateJson(value: unknown, max = 1500): string {
  const s = JSON.stringify(value, null, 2);
  if (s == null) return '(null)';
  return s.length > max ? `${s.slice(0, max)}\n… [truncated, ${s.length - max} chars omitted]` : s;
}

/** Snapshot of "what the user is currently looking at" — re-built fresh on
 *  every send so the agent's view of the step is never stale. Format kept
 *  human-readable on purpose; the agent reasons better with prose than with
 *  raw JSON dumps for small things like a 3-line query. */
export function buildContextBlock(
  server: McpServer | null,
  step: WorkflowStep,
  lastTestResponse?: unknown,
  lastTestError?: string | null,
  t?: Translator,
): string {
  // Fallback translator used by unit tests that don't pass one — keeps the
  // pure-function tests independent of the i18n provider.
  const tr: Translator = t ?? ((k: string) => k);
  const baseUrl = server?.api_spec?.base_url ?? tr('wf.apicall.helper.sys.unknown');
  const apiName = server?.name ?? tr('wf.apicall.helper.sys.noPlugin');
  const currentEndpoint = step.api_endpoint_path ?? tr('wf.apicall.helper.sys.none');
  const currentMethod = step.api_method ?? tr('wf.apicall.helper.sys.default');
  const currentQuery = step.api_query ? JSON.stringify(step.api_query) : tr('wf.apicall.helper.sys.empty');
  const currentHeaders = step.api_headers ? JSON.stringify(step.api_headers) : tr('wf.apicall.helper.sys.none');
  const currentBody = step.api_body ? step.api_body : tr('wf.apicall.helper.sys.none');
  const currentExtract = step.api_extract?.path ?? tr('wf.apicall.helper.sys.none');

  const lines = [
    tr('wf.apicall.helper.sys.ctxHeader'),
    `- API : ${apiName} (${baseUrl})`,
    `- endpoint : ${currentEndpoint}`,
    `- method   : ${currentMethod}`,
    `- query    : ${currentQuery}`,
    `- headers  : ${currentHeaders}`,
    `- body     : ${currentBody}`,
    `- extract  : ${currentExtract}`,
  ];

  if (lastTestError) {
    lines.push('', tr('wf.apicall.helper.sys.ctxLastFail'), lastTestError);
  } else if (lastTestResponse !== undefined && lastTestResponse !== null) {
    lines.push(
      '',
      tr('wf.apicall.helper.sys.ctxLastOk'),
      '```json',
      truncateJson(lastTestResponse),
      '```',
    );
  }

  return lines.join('\n');
}

/** Build the auth-info block injected into every prompt: tells the agent
 *  what's already wired so it doesn't keep suggesting `apikey: 'YOUR_KEY'`
 *  in query params. Empty string when auth is `None`. */
function buildAuthBlock(server: McpServer | null, t: Translator): string {
  const slots = authSlotsForServer(server);
  if (slots.length === 0) return '';
  const lines = slots.map(s => {
    const key = s.kind === 'query'
      ? 'wf.apicall.helper.sys.authQueryItem'
      : 'wf.apicall.helper.sys.authHeaderItem';
    return t(key, s.name, s.envKey);
  });
  return `${t('wf.apicall.helper.sys.authHeader')}\n${lines.join('\n')}`;
}

/** Build the initial system prompt that bootstraps the helper conversation.
 *  Bundles the static API spec (endpoints list — agent must NOT hallucinate
 *  outside this list) with the meta-role briefing, plugin-specific lore,
 *  the auth-managed disclaimer, a debugging method, and a fresh context
 *  block of the current step state and any test result. */
function buildSystemPrompt(
  server: McpServer | null,
  step: WorkflowStep,
  lastTestResponse: unknown,
  lastTestError: string | null | undefined,
  t: Translator,
): string {
  const endpoints = (server?.api_spec?.endpoints ?? [])
    .map(ep => `- ${ep.method} ${ep.path}${ep.description ? ` — ${ep.description}` : ''}`)
    .join('\n');
  const authBlock = buildAuthBlock(server, t);
  const contextBlock = buildContextBlock(server, step, lastTestResponse, lastTestError, t);
  const tips = tipsForSlug(server?.id ?? null);
  const docsUrl = server?.api_spec?.docs_url ?? tips?.docsUrl ?? null;

  return `${t('wf.apicall.helper.sys.role')}

${t('wf.apicall.helper.sys.boundaries')}

${t('wf.apicall.helper.sys.action')}

${t('wf.apicall.helper.sys.debug')}

${t('wf.apicall.helper.sys.endpointsHeader')}
${endpoints || t('wf.apicall.helper.sys.endpointsEmpty')}

${authBlock ? authBlock + '\n\n' : ''}${tips ? `${t('wf.apicall.helper.sys.tipsHeader', server?.name ?? '')}\n${tips.body}\n\n` : ''}${docsUrl ? `${t('wf.apicall.helper.sys.docsHeader')}\n${docsUrl}\n\n` : ''}${contextBlock}

${t('wf.apicall.helper.sys.style')}

${t('wf.apicall.helper.sys.starter')}`;
}

export function ApiCallAiHelper({
  step,
  onApply,
  selectedServer,
  projectId,
  installedAgents,
  lastTestResponse,
  lastTestError,
  configLanguage,
  t,
}: ApiCallAiHelperProps) {
  // The agent's reply language follows the backend "output language" config
  // (Settings → Output language) — separate from the UI locale. We build a
  // dedicated translator for the system prompt + injected context that
  // resolves keys against `configLanguage`, while `t` keeps driving UI labels.
  const agentT = useCallback<Translator>(
    (key, ...args) => translate(toUILocale(configLanguage), key, ...args),
    [configLanguage],
  );
  const [phase, setPhase] = useState<Phase>('closed');
  // Active agent for the current chat. Defaults to the first installed
  // agent and is mutable mid-conversation via the header dropdown (when
  // the user switches, the discussion resets — see `switchAgent` below).
  const [activeAgent, setActiveAgent] = useState<AgentType | null>(null);
  const [agentMenuOpen, setAgentMenuOpen] = useState(false);
  const [discussionId, setDiscussionId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [streaming, setStreaming] = useState(false);
  // Race-free re-entry guard. Two fast Enter presses on the input
  // textarea both read `streaming === false` from the same closure
  // (state hasn't re-rendered yet) and fire `sendMessageStream` twice
  // in parallel — duplicate user bubble in the chat + 2 agent runs on
  // the same ephemeral discussion. The ref reads/writes synchronously
  // so the second invocation bails out.
  const streamingRef = useRef(false);
  const [input, setInput] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [minimized, setMinimized] = useState(false);
  const [appliedSignatures, setAppliedSignatures] = useState<Set<string>>(new Set());
  const abortRef = useRef<AbortController | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // Auto-focus the textarea every time the bubble (re)becomes visible
  // — opening the helper, restoring from minimized, finishing the
  // initial stream. The user opened the bubble to type, give them the
  // cursor without an extra click.
  useEffect(() => {
    if (phase === 'chatting' && !minimized) {
      // RAF defer because the textarea may not be in the DOM yet on
      // the same tick as the phase change.
      const id = requestAnimationFrame(() => inputRef.current?.focus());
      return () => cancelAnimationFrame(id);
    }
  }, [phase, minimized, streaming]);

  // Close the agent menu when clicking outside. The menu is a small
  // floating list under the chevron; without this it stays open after
  // selecting and the next click anywhere just dismisses it instead of
  // performing the user's actual intent.
  useEffect(() => {
    if (!agentMenuOpen) return;
    const handler = (e: MouseEvent) => {
      const target = e.target as HTMLElement;
      if (!target.closest('.wf-apicall-ai-agent-selector')) {
        setAgentMenuOpen(false);
      }
    };
    window.addEventListener('mousedown', handler);
    return () => window.removeEventListener('mousedown', handler);
  }, [agentMenuOpen]);

  // Auto-scroll to the bottom on new content. Use `behavior: 'auto'` (jump,
  // not smooth) so streaming feels instantaneous and doesn't fight against
  // chunks landing back-to-back.
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'auto', block: 'end' });
  }, [messages]);

  // Cleanup the ephemeral discussion when the helper unmounts. Best-effort:
  // we ignore the result because the user has already moved on.
  useEffect(() => {
    return () => {
      abortRef.current?.abort();
      if (discussionId) {
        discussionsApi.delete(discussionId).catch(() => {});
      }
    };
  }, [discussionId]);

  const close = useCallback(() => {
    abortRef.current?.abort();
    if (discussionId) {
      discussionsApi.delete(discussionId).catch(() => {});
    }
    setDiscussionId(null);
    setMessages([]);
    setInput('');
    setError(null);
    setAppliedSignatures(new Set());
    setStreaming(false);
    setMinimized(false);
    setAgentMenuOpen(false);
    setActiveAgent(null);
    setPhase('closed');
  }, [discussionId]);

  // We forward-declare the switchAgent ref because startWithAgent is
  // defined just below and we need to call it from switchAgent without a
  // circular dep in useCallback. Set after startWithAgent is defined.
  const startWithAgentRef = useRef<((agent: AgentType) => Promise<void>) | null>(null);

  // Triggered by the agent dropdown in the bubble header. Replaces the
  // standalone 'picking-agent' phase from 0.8.0: the user always sees the
  // chat, and switching agents is one click in the header rather than a
  // full modality change. Reset is brutal (kill old discussion, start a
  // new one with the same system prompt) because partial migration of an
  // in-flight conversation across agents is messy and not what users want
  // when they switch — they're saying "this agent isn't getting it, let
  // me try another from scratch".
  const switchAgent = useCallback((agent: AgentType) => {
    setAgentMenuOpen(false);
    if (agent === activeAgent || streaming) return;
    abortRef.current?.abort();
    if (discussionId) {
      discussionsApi.delete(discussionId).catch(() => {});
    }
    setDiscussionId(null);
    setMessages([]);
    setInput('');
    setError(null);
    setAppliedSignatures(new Set());
    // Prime a fresh discussion for the new agent. Phase stays 'chatting'
    // because the bubble is already open; only the contents reset.
    void startWithAgentRef.current?.(agent);
  }, [activeAgent, streaming, discussionId]);

  const startWithAgent = useCallback(async (agent: AgentType) => {
    // 0.8.1 UX: open the bubble and prime an ephemeral discussion with
    // the system prompt baked in, but DON'T fire the agent right away.
    // The user sees the welcome state (starter chips) and only spends
    // tokens once they pick a chip or type their first question. Saves
    // ~200 tokens per helper-open + removes the "thinking…" spinner
    // on a chat the user hasn't even read yet.
    setPhase('chatting');
    setActiveAgent(agent);
    setMessages([]);
    setError(null);
    try {
      // `project_id: null` is accepted — this is an ephemeral, one-shot
      // helper conversation. The real context the agent needs is the API
      // spec (`selectedServer`), which is already baked into the system
      // prompt.
      const disc = await discussionsApi.create({
        project_id: projectId,
        title: `🤖 ${t('wf.apicall.helper.discTitle')}`,
        agent,
        // Backend reads `language` to inject "Respond in {lang}" into the
        // agent's prompt. Without this the agent defaulted to French
        // regardless of the user's Output Language config.
        language: configLanguage ?? 'fr',
        initial_prompt: buildSystemPrompt(selectedServer, step, lastTestResponse, lastTestError, agentT),
      });
      setDiscussionId(disc.id);
    } catch (e) {
      console.error('[ApiCallAiHelper] startWithAgent failed:', e);
      setError(String(e));
    }
  }, [projectId, selectedServer, step, lastTestResponse, lastTestError, t, configLanguage, agentT]);

  // Re-bind the ref every time startWithAgent's identity changes (when
  // any of its deps change). switchAgent reads this ref so it can call
  // the freshest version without listing all deps and re-creating itself.
  useEffect(() => {
    startWithAgentRef.current = startWithAgent;
  }, [startWithAgent]);

  const sendMessage = useCallback(async () => {
    const userText = input.trim();
    if (!userText || !discussionId || streamingRef.current) return;
    streamingRef.current = true;
    setInput('');
    setError(null);
    // What we display in the chat = what the user actually typed. What we
    // send to the backend gets a fresh context block prepended so the agent
    // can reason about the *current* step state and the most recent test
    // result, not whatever was true when the discussion started.
    setMessages(prev => [...prev, { role: 'user', text: userText }, { role: 'assistant', text: '' }]);
    setStreaming(true);

    // The agent reads this context — must be in its reply language, not the UI's.
    const contextBlock = buildContextBlock(selectedServer, step, lastTestResponse, lastTestError, agentT);
    const enriched = `${contextBlock}\n\n${agentT('wf.apicall.helper.sys.userQuestion')}\n${userText}`;

    const controller = new AbortController();
    abortRef.current = controller;

    await discussionsApi.sendMessageStream(
      discussionId,
      { content: enriched },
      chunk => {
        setMessages(prev => {
          const last = prev[prev.length - 1];
          if (last?.role !== 'assistant') {
            return [...prev, { role: 'assistant', text: chunk }];
          }
          return [...prev.slice(0, -1), { ...last, text: last.text + chunk }];
        });
      },
      () => { streamingRef.current = false; setStreaming(false); },
      err => {
        console.error('[ApiCallAiHelper] sendMessageStream error:', err);
        setError(err);
        streamingRef.current = false;
        setStreaming(false);
      },
      controller.signal,
    );
  }, [input, discussionId, selectedServer, step, lastTestResponse, lastTestError, agentT]);

  const stopStream = useCallback(() => {
    abortRef.current?.abort();
    if (discussionId) {
      discussionsApi.stop(discussionId).catch(() => {});
    }
    streamingRef.current = false;
    setStreaming(false);
  }, [discussionId]);

  const handleApply = useCallback((sig: string, parsed: Record<string, unknown>) => {
    onApply(applyToStep(parsed, step, selectedServer));
    setAppliedSignatures(prev => {
      const next = new Set(prev);
      next.add(sig);
      return next;
    });
  }, [onApply, step, selectedServer]);

  // ─── Phase: closed (just the trigger button) ────────────────────────
  if (phase === 'closed') {
    // Without an API selected the agent has no spec, no endpoints list, no
    // auth context — its advice degenerates to "go pick an API". Hard-
    // disable the button rather than letting the user open an empty helper
    // and feel stuck.
    const noApi = !selectedServer;
    const triggerHint = noApi
      ? t('wf.apicall.helper.triggerDisabled')
      : t('wf.apicall.helper.triggerHint');
    return (
      <>
        <button
          type="button"
          className="wf-apicall-ai-trigger"
          disabled={noApi}
          onClick={() => {
            if (installedAgents.length === 0) {
              setError(t('wf.apicall.helper.noAgents'));
              return;
            }
            // 0.8.1 UX: skip the standalone picker. Open the chat
            // directly with the first installed agent. The user can
            // switch via the header dropdown once the bubble is open.
            void startWithAgent(installedAgents[0]);
          }}
          title={triggerHint}
        >
          <Sparkles size={11} /> {t('wf.apicall.helper.trigger')}
        </button>
        {error && (
          <span className="wf-apicall-ai-inline-error" role="alert">
            {error}
          </span>
        )}
      </>
    );
  }

  // ─── Phase: chatting (floating bubble) ──────────────────────────────
  return (
    <>
      <button
        type="button"
        className="wf-apicall-ai-trigger wf-apicall-ai-trigger-active"
        onClick={() => setMinimized(m => !m)}
      >
        <Sparkles size={11} /> {t('wf.apicall.helper.trigger')}
      </button>
      {!minimized && (
        <div className="wf-apicall-ai-bubble" role="dialog" aria-label={t('wf.apicall.helper.bubbleTitle')}>
          <div className="wf-apicall-ai-bubble-header">
            <Bot size={13} />
            {/* 0.8.1 UX: agent selector lives directly in the header, replacing
                the standalone picking-agent phase. The chevron toggles a small
                dropdown with all installed agents; click switches and resets
                the conversation. */}
            <div className="wf-apicall-ai-agent-selector">
              <button
                type="button"
                className="wf-apicall-ai-agent-trigger"
                onClick={() => setAgentMenuOpen(o => !o)}
                disabled={streaming}
                aria-haspopup="listbox"
                aria-expanded={agentMenuOpen}
                title={t('wf.apicall.helper.switchAgent')}
              >
                <span
                  className="wf-apicall-ai-agent-dot"
                  style={{ background: activeAgent ? agentColor(activeAgent) : 'var(--kr-text-ghost)' }}
                />
                <span>{activeAgent ? (AGENT_LABELS[activeAgent] ?? activeAgent) : t('wf.apicall.helper.bubbleTitle')}</span>
                <ChevronDown size={11} />
              </button>
              {agentMenuOpen && (
                <div className="wf-apicall-ai-agent-menu" role="listbox">
                  {installedAgents.map(agent => (
                    <button
                      key={agent}
                      type="button"
                      role="option"
                      aria-selected={agent === activeAgent}
                      className={`wf-apicall-ai-agent-option${agent === activeAgent ? ' wf-apicall-ai-agent-option-active' : ''}`}
                      onClick={() => switchAgent(agent)}
                    >
                      <span className="wf-apicall-ai-agent-dot" style={{ background: agentColor(agent) }} />
                      {AGENT_LABELS[agent] ?? agent}
                    </button>
                  ))}
                </div>
              )}
            </div>
            <span className="wf-apicall-ai-bubble-eph">{t('wf.apicall.helper.ephemeral')}</span>
            <button
              type="button"
              className="wf-apicall-ai-icon-btn"
              onClick={() => setMinimized(true)}
              title={t('wf.apicall.helper.minimize')}
              aria-label={t('wf.apicall.helper.minimize')}
            >
              <Minus size={12} />
            </button>
            <button
              type="button"
              className="wf-apicall-ai-icon-btn"
              onClick={close}
              title={t('wf.apicall.helper.close')}
              aria-label={t('wf.apicall.helper.close')}
            >
              <X size={12} />
            </button>
          </div>

          {/* 0.8.1 UX: context chip moved to top so users see what the agent
              already knows BEFORE the chat content scrolls into view. */}
          <div className="wf-apicall-ai-context-chip wf-apicall-ai-context-chip-top" title={t('wf.apicall.helper.contextHint')}>
            <span>📎</span>
            <span className="wf-apicall-ai-context-chip-label">
              {selectedServer?.name ?? t('wf.apicall.helper.contextNoApi')}
              {step.api_endpoint_path ? ` · ${step.api_endpoint_path}` : ''}
              {lastTestError ? ` · ${t('wf.apicall.helper.contextLastErr')}` : ''}
              {!lastTestError && lastTestResponse !== undefined && lastTestResponse !== null
                ? ` · ${t('wf.apicall.helper.contextLastOk')}`
                : ''}
            </span>
          </div>

          <div className="wf-apicall-ai-bubble-messages">
            {/* Welcome state: when no user message has been sent yet, show a
                short intro + 3 clickable starter chips. Clicking a chip
                fills the input with a template so the user can adjust and
                send. Cleaner than auto-firing the agent on open (which
                burns tokens for a generic "how can I help" reply nobody
                reads). */}
            {messages.length === 0 && !streaming && (
              <div className="wf-apicall-ai-welcome">
                <div className="wf-apicall-ai-welcome-title">
                  {t('wf.apicall.helper.welcome')}
                </div>
                <div className="wf-apicall-ai-welcome-chips">
                  <button
                    type="button"
                    className="wf-apicall-ai-starter-chip"
                    onClick={() => {
                      setInput(t('wf.apicall.helper.starter.buildPrompt'));
                      inputRef.current?.focus();
                    }}
                  >
                    <MessageSquareText size={11} /> {t('wf.apicall.helper.starter.build')}
                  </button>
                  <button
                    type="button"
                    className="wf-apicall-ai-starter-chip"
                    onClick={() => {
                      setInput(t('wf.apicall.helper.starter.endpointPrompt'));
                      inputRef.current?.focus();
                    }}
                  >
                    <Link2 size={11} /> {t('wf.apicall.helper.starter.endpoint')}
                  </button>
                  <button
                    type="button"
                    className="wf-apicall-ai-starter-chip"
                    onClick={() => {
                      setInput(t('wf.apicall.helper.starter.bodyPrompt'));
                      inputRef.current?.focus();
                    }}
                  >
                    <ClipboardPaste size={11} /> {t('wf.apicall.helper.starter.body')}
                  </button>
                </div>
              </div>
            )}
            {messages.map((msg, idx) => (
              <ChatMessageView
                key={idx}
                msg={msg}
                appliedSignatures={appliedSignatures}
                onApply={handleApply}
                t={t}
              />
            ))}
            {streaming && messages[messages.length - 1]?.text === '' && (
              <div className="wf-apicall-ai-typing">
                <Loader2 size={11} className="spin" /> {t('wf.apicall.helper.thinking')}
              </div>
            )}
            <div ref={messagesEndRef} />
          </div>

          {error && (
            <div className="wf-apicall-ai-error" role="alert">
              {error}
            </div>
          )}

          <div className="wf-apicall-ai-bubble-input">
            <textarea
              ref={inputRef}
              value={input}
              onChange={e => setInput(e.target.value)}
              onKeyDown={e => {
                // `nativeEvent.isComposing` skips the IME-confirmation
                // Enter that fires while a CJK input method is composing
                // a candidate — pressing Enter to validate the
                // composition would otherwise send a half-finished
                // message.
                if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  void sendMessage();
                }
              }}
              placeholder={t('wf.apicall.helper.inputPlaceholder')}
              rows={2}
              disabled={streaming}
              autoFocus
            />
            {streaming ? (
              <button
                type="button"
                className="wf-apicall-ai-send-btn wf-apicall-ai-stop-btn"
                onClick={stopStream}
                title={t('wf.apicall.helper.stop')}
                aria-label={t('wf.apicall.helper.stop')}
              >
                <Loader2 size={11} className="spin" />
              </button>
            ) : (
              <button
                type="button"
                className="wf-apicall-ai-send-btn"
                onClick={() => void sendMessage()}
                disabled={!input.trim() || !discussionId}
                title={t('wf.apicall.helper.send')}
                aria-label={t('wf.apicall.helper.send')}
              >
                <Send size={11} />
              </button>
            )}
          </div>
        </div>
      )}
      {minimized && (
        <button
          type="button"
          className="wf-apicall-ai-restore"
          onClick={() => setMinimized(false)}
          title={t('wf.apicall.helper.restore')}
          aria-label={t('wf.apicall.helper.restore')}
        >
          <Maximize2 size={11} />
        </button>
      )}
    </>
  );
}

// ─── Sub-components ────────────────────────────────────────────────────

interface ChatMessageViewProps {
  msg: ChatMessage;
  appliedSignatures: Set<string>;
  onApply: (sig: string, parsed: Record<string, unknown>) => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

/** Render a single chat message. For assistant messages we extract any
 *  KRONN:APPLY blocks and replace them with inline Apply cards — this keeps
 *  the chat tidy: the user sees the prose explanation followed by a clear
 *  one-click button, instead of a wall of fenced JSON. */
function ChatMessageView({ msg, appliedSignatures, onApply, t }: ChatMessageViewProps) {
  const blocks = useMemo(
    () => (msg.role === 'assistant' ? parseApplyBlocks(msg.text) : []),
    [msg.role, msg.text],
  );
  // Strip the KRONN:APPLY chunks from the displayed prose — the SuggestionCard
  // takes their place visually.
  const prose = useMemo(
    () => msg.role === 'assistant' ? msg.text.replace(KRONN_APPLY_RX, '').trim() : msg.text,
    [msg.role, msg.text],
  );

  return (
    <div className={`wf-apicall-ai-msg wf-apicall-ai-msg-${msg.role}`}>
      {prose && <div className="wf-apicall-ai-msg-text">{prose}</div>}
      {blocks.map(block => (
        <SuggestionCard
          key={block.signature}
          parsed={block.parsed}
          applied={appliedSignatures.has(block.signature)}
          onApply={() => onApply(block.signature, block.parsed)}
          t={t}
        />
      ))}
    </div>
  );
}

interface SuggestionCardProps {
  parsed: Record<string, unknown>;
  applied: boolean;
  onApply: () => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

function SuggestionCard({ parsed, applied, onApply, t }: SuggestionCardProps) {
  const fields = Object.entries(parsed).filter(([, v]) => v !== undefined && v !== null);
  return (
    <div className={`wf-apicall-ai-suggestion${applied ? ' wf-apicall-ai-suggestion-applied' : ''}`}>
      <div className="wf-apicall-ai-suggestion-header">
        <Sparkles size={11} />
        <span>{t('wf.apicall.helper.suggestion')}</span>
      </div>
      <ul className="wf-apicall-ai-suggestion-list">
        {fields.map(([k, v]) => (
          <li key={k}>
            <strong>{k}</strong>: <code>{typeof v === 'string' ? v : JSON.stringify(v)}</code>
          </li>
        ))}
      </ul>
      <button
        type="button"
        className="wf-apicall-ai-apply-btn"
        onClick={onApply}
        disabled={applied}
      >
        {applied ? t('wf.apicall.helper.applied') : t('wf.apicall.helper.apply')}
      </button>
    </div>
  );
}
