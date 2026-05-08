import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Bot, X, Send, Sparkles, Loader2, Minus, Maximize2 } from 'lucide-react';
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

type Phase = 'closed' | 'picking-agent' | 'chatting';

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
    setPhase('closed');
  }, [discussionId]);

  const startWithAgent = useCallback(async (agent: AgentType) => {
    // Transition to the chat phase first so the user gets immediate visual
    // feedback — without this the picker silently stayed open while the
    // discussion was being created (or while we surfaced an error), making
    // it look like the click did nothing.
    setPhase('chatting');
    setMessages([]);
    setError(null);
    setStreaming(true);
    setMessages([{ role: 'assistant', text: '' }]);
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

      const controller = new AbortController();
      abortRef.current = controller;

      await discussionsApi.runAgent(
        disc.id,
        chunk => {
          setMessages(prev => {
            if (prev.length === 0) return [{ role: 'assistant', text: chunk }];
            const last = prev[prev.length - 1];
            if (last.role !== 'assistant') {
              return [...prev, { role: 'assistant', text: chunk }];
            }
            return [...prev.slice(0, -1), { ...last, text: last.text + chunk }];
          });
        },
        () => setStreaming(false),
        err => {
          // Surface the error in the bubble; also log so a power user can
          // inspect it in DevTools without re-running.
          console.error('[ApiCallAiHelper] runAgent error:', err);
          setError(err);
          setStreaming(false);
        },
        controller.signal,
      );
    } catch (e) {
      console.error('[ApiCallAiHelper] startWithAgent failed:', e);
      setError(String(e));
      setStreaming(false);
    }
  }, [projectId, selectedServer, step, lastTestResponse, lastTestError, t, configLanguage, agentT]);

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
            if (installedAgents.length === 1) {
              void startWithAgent(installedAgents[0]);
              return;
            }
            setPhase('picking-agent');
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

  // ─── Phase: picking-agent (popover) ─────────────────────────────────
  if (phase === 'picking-agent') {
    return (
      <>
        <button
          type="button"
          className="wf-apicall-ai-trigger wf-apicall-ai-trigger-active"
          onClick={() => setPhase('closed')}
        >
          <Sparkles size={11} /> {t('wf.apicall.helper.trigger')}
        </button>
        <div className="wf-apicall-ai-agent-popover" role="dialog" aria-label={t('wf.apicall.helper.pickAgent')}>
          <div className="wf-apicall-ai-popover-header">{t('wf.apicall.helper.pickAgent')}</div>
          {installedAgents.map(agent => (
            <button
              key={agent}
              type="button"
              className="wf-apicall-ai-agent-option"
              onClick={() => void startWithAgent(agent)}
            >
              <span className="wf-apicall-ai-agent-dot" style={{ background: agentColor(agent) }} />
              {AGENT_LABELS[agent] ?? agent}
            </button>
          ))}
          <button
            type="button"
            className="wf-apicall-ai-agent-cancel"
            onClick={() => setPhase('closed')}
          >
            {t('wf.apicall.helper.cancel')}
          </button>
        </div>
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
            <strong>{t('wf.apicall.helper.bubbleTitle')}</strong>
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

          <div className="wf-apicall-ai-bubble-messages">
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

          <div className="wf-apicall-ai-context-chip" title={t('wf.apicall.helper.contextHint')}>
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
