import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Bot, X, Send, Sparkles, Loader2, Minus, Maximize2 } from 'lucide-react';
import { discussions as discussionsApi } from '../../lib/api';
import { AGENT_LABELS, agentColor } from '../../lib/constants';
import type { AgentType, McpServer, WorkflowStep } from '../../types/generated';
import { authSlotsForServer, managedHeaderNames, managedQueryNames, stripManagedHeaders, stripManagedQuery } from './apiCallAuth';
import { tipsForSlug } from './apiCallPluginTips';

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
): string {
  const baseUrl = server?.api_spec?.base_url ?? '(inconnu)';
  const apiName = server?.name ?? '(aucun plugin sélectionné)';
  const currentEndpoint = step.api_endpoint_path ?? '(aucun)';
  const currentMethod = step.api_method ?? '(par défaut)';
  const currentQuery = step.api_query ? JSON.stringify(step.api_query) : '(vide)';
  const currentHeaders = step.api_headers ? JSON.stringify(step.api_headers) : '(aucun)';
  const currentBody = step.api_body ? step.api_body : '(aucun)';
  const currentExtract = step.api_extract?.path ?? '(aucun)';

  const lines = [
    '### CONTEXTE COURANT (snapshot au moment de cet envoi)',
    `- API : ${apiName} (${baseUrl})`,
    `- endpoint : ${currentEndpoint}`,
    `- method   : ${currentMethod}`,
    `- query    : ${currentQuery}`,
    `- headers  : ${currentHeaders}`,
    `- body     : ${currentBody}`,
    `- extract  : ${currentExtract}`,
  ];

  if (lastTestError) {
    lines.push('', '### DERNIER TEST → ÉCHEC', lastTestError);
  } else if (lastTestResponse !== undefined && lastTestResponse !== null) {
    lines.push(
      '',
      '### DERNIER TEST → SUCCÈS (extrait de réponse)',
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
function buildAuthBlock(server: McpServer | null): string {
  const slots = authSlotsForServer(server);
  if (slots.length === 0) return '';
  const lines = slots.map(s => {
    const where = s.kind === 'query' ? 'query param' : 'header';
    return `- ${where} \`${s.name}\` (valeur tirée de l'env \`${s.envKey}\`, déjà configurée dans Kronn)`;
  });
  return `### AUTH — gérée automatiquement par Kronn (NE PAS suggérer)
Le backend injecte ces champs au moment de la requête, avec la clé que le
user a entrée dans Settings → APIs. ⚠️ Ne JAMAIS les remettre dans une
suggestion KRONN:APPLY ; cela écraserait la vraie valeur par un placeholder
("VOTRE_API_KEY", etc.) et casserait l'auth.
${lines.join('\n')}`;
}

/** Build the initial system prompt that bootstraps the helper conversation.
 *  Bundles the static API spec (endpoints list — agent must NOT hallucinate
 *  outside this list) with the meta-role briefing, plugin-specific lore,
 *  the auth-managed disclaimer, a debugging method, and a fresh context
 *  block of the current step state and any test result. */
function buildSystemPrompt(
  server: McpServer | null,
  step: WorkflowStep,
  lastTestResponse?: unknown,
  lastTestError?: string | null,
): string {
  const endpoints = (server?.api_spec?.endpoints ?? [])
    .map(ep => `- ${ep.method} ${ep.path}${ep.description ? ` — ${ep.description}` : ''}`)
    .join('\n');
  const authBlock = buildAuthBlock(server);
  const contextBlock = buildContextBlock(server, step, lastTestResponse, lastTestError);
  const tips = tipsForSlug(server?.id ?? null);
  const docsUrl = server?.api_spec?.docs_url ?? tips?.docsUrl ?? null;

  return `# Rôle
Tu es un assistant de configuration pour une étape "Récupérer des données" (\`StepType::ApiCall\`)
dans un workflow Kronn. Cette étape fait un appel HTTP direct (sans LLM consommé) et extrait
une valeur via JSONPath, qui pipe vers le step suivant.

# Ce que tu peux et ne peux PAS faire
- ✅ Lire la spec API ci-dessous, l'état courant du step, le résultat du dernier test.
- ✅ Proposer des modifications de config (endpoint, method, query, headers, body, extract).
- ❌ Tu n'as **AUCUN** outil pour appeler l'API toi-même. Pas de Bash, pas de MCP, pas de fetch.
     C'est Kronn (backend Rust) qui fait l'appel quand l'utilisateur clique "Test the call".
- ❌ Ne propose JAMAIS d'aller "vérifier en ligne" via un MCP — ce n'est pas dispo dans cette
     conversation. Si la cause d'un échec t'échappe, oriente vers la doc officielle ou le
     dashboard du fournisseur.

# Ta seule action utile : proposer un \`KRONN:APPLY\`
Quand tu suggères une config, écris EXACTEMENT ce format (sinon l'UI ne te lit pas) :

KRONN:APPLY
\`\`\`json
{ "endpoint": "/path/v4", "query": { "k": "v" }, "extract": "$.data[*]" }
\`\`\`

Champs autorisés : \`endpoint\`, \`method\`, \`query\`, \`headers\`, \`body\`, \`extract\`.
Tu peux ne mettre QU'UN champ — le reste du step est préservé.
Pas plus d'un bloc \`KRONN:APPLY\` par message (sinon l'utilisateur ne sait plus lequel choisir).

# Méthode de debug (à appliquer dès qu'un test échoue)
1. Lis le bloc \`### DERNIER TEST → ÉCHEC\` — il contient \`HTTP <status> on <method> <url> — <body excerpt>\`.
2. Note le **status** (4xx vs 5xx → 4xx = ta config est en cause, 5xx = côté serveur).
3. Note les **query params** dans l'URL composée (l'apikey est masquée — c'est normal et ATTENDU).
4. Croise avec les TIPS du plugin ci-dessous + les endpoints autorisés.
5. Propose **UNE** modification ciblée (un seul KRONN:APPLY). Pas de salve de 3 hypothèses simultanées.
6. Si l'échec persiste après 2 tentatives, recommande à l'utilisateur de vérifier dans le dashboard
   du fournisseur (URL de doc plus bas) — ne tourne pas en rond.

# Endpoints AUTORISÉS (n'invente jamais hors de cette liste)
${endpoints || '(aucun — l\'utilisateur n\'a pas encore choisi d\'API)'}

${authBlock ? authBlock + '\n\n' : ''}${tips ? `# TIPS PLUGIN — ${server?.name ?? ''}\n${tips.body}\n\n` : ''}${docsUrl ? `# Doc officielle\n${docsUrl}\n\n` : ''}${contextBlock}

# Style
- Français, ≤ 3 lignes par message.
- Pas de blabla, pas de "je vais analyser", pas de "voilà ce que je suggère :" — direct au but.
- L'apikey/token affichés en \`••••••••\` ou \`***\` dans les logs sont MASQUÉS UNIQUEMENT À L'AFFICHAGE.
  La vraie valeur configurée par l'utilisateur EST envoyée par Kronn. Si on te demande "tu es sûr
  que la clé est bien envoyée ?" — réponds OUI sans réserve, et redirige le diag ailleurs (host,
  endpoint, scope du token).

Pour démarrer, demande au user ce qu'il veut récupérer.`;
}

export function ApiCallAiHelper({
  step,
  onApply,
  selectedServer,
  projectId,
  installedAgents,
  lastTestResponse,
  lastTestError,
  t,
}: ApiCallAiHelperProps) {
  const [phase, setPhase] = useState<Phase>('closed');
  const [discussionId, setDiscussionId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [streaming, setStreaming] = useState(false);
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
        initial_prompt: buildSystemPrompt(selectedServer, step, lastTestResponse, lastTestError),
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
  }, [projectId, selectedServer, step, lastTestResponse, lastTestError, t]);

  const sendMessage = useCallback(async () => {
    const userText = input.trim();
    if (!userText || !discussionId || streaming) return;
    setInput('');
    setError(null);
    // What we display in the chat = what the user actually typed. What we
    // send to the backend gets a fresh context block prepended so the agent
    // can reason about the *current* step state and the most recent test
    // result, not whatever was true when the discussion started.
    setMessages(prev => [...prev, { role: 'user', text: userText }, { role: 'assistant', text: '' }]);
    setStreaming(true);

    const contextBlock = buildContextBlock(selectedServer, step, lastTestResponse, lastTestError);
    const enriched = `${contextBlock}\n\n### QUESTION DU USER\n${userText}`;

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
      () => setStreaming(false),
      err => {
        console.error('[ApiCallAiHelper] sendMessageStream error:', err);
        setError(err);
        setStreaming(false);
      },
      controller.signal,
    );
  }, [input, discussionId, streaming, selectedServer, step, lastTestResponse, lastTestError]);

  const stopStream = useCallback(() => {
    abortRef.current?.abort();
    if (discussionId) {
      discussionsApi.stop(discussionId).catch(() => {});
    }
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
                if (e.key === 'Enter' && !e.shiftKey) {
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
