// Mirror of ApiCallAiHelper, scoped to the "Custom API plugin" creation
// form in McpPage. Shares the same UX shell (single-phase chat, header
// agent dropdown, top context chip, welcome state with starter chips)
// and the same KRONN:APPLY block protocol — but the system prompt
// targets Custom API spec extraction instead of ApiCall step editing,
// and Apply suggestions land in form state instead of a workflow step.
//
// TD-helpers-unify: ApiCallAiHelper and CustomApiAiHelper share ~60% of
// their lifecycle code (phases, streaming, agent dropdown, welcome
// state, KRONN:APPLY parsing). A future refactor should extract a
// shared `<AiChatHelperShell>` that both consume via injected
// buildSystemPrompt / buildContext / onApply slots.

import { useCallback, useEffect, useRef, useState } from 'react';
import {
  Bot, X, Send, Sparkles, Loader2, Minus, Maximize2, ChevronDown,
  ClipboardPaste, Link2, MessageSquareText,
} from 'lucide-react';
import { discussions as discussionsApi } from '../lib/api';
import { AGENT_LABELS, agentColor } from '../lib/constants';
import { t as translate, type UILocale } from '../lib/i18n';
import type { AgentType, CustomApiField, CustomApiPayload } from '../types/generated';
import { parseApplyBlocks } from './workflows/ApiCallAiHelper';
import './aiHelper.css';

type Translator = (key: string, ...args: (string | number)[]) => string;

function toUILocale(lang: string | undefined): UILocale {
  if (lang === 'fr' || lang === 'en' || lang === 'es') return lang;
  return 'en';
}

/** Snapshot of the in-progress form passed to the helper, plus the apply
 *  callback that lets the helper push the agent's suggestions back into
 *  the parent form state. Shape mirrors `CustomApiPayload`. */
export interface CustomApiAiHelperProps {
  /** Current form values — rendered into the context block so the agent
   *  sees what the user has typed so far and only fills the gaps. */
  formSnapshot: {
    name: string;
    base_url: string;
    description: string;
    docs_url: string;
    fields: CustomApiField[];
  };
  /** Apply a partial Custom API spec back to the parent form state. */
  onApply: (updates: Partial<CustomApiPayload>) => void;
  /** Agents installed locally — used to pre-select & populate the picker. */
  installedAgents: AgentType[];
  /** Backend output-language (Settings → Output language) drives the
   *  agent's reply language. UI labels stay UI-locale. */
  configLanguage?: string;
  t: Translator;
}

type Phase = 'closed' | 'chatting';

interface ChatMessage {
  role: 'user' | 'assistant';
  text: string;
}

/** System prompt for the Custom API creation flow. Tells the agent what
 *  it can produce (fenced KRONN:APPLY JSON), what fields the form
 *  expects, and how to ask follow-up questions when the user's input is
 *  ambiguous. The prompt is intentionally short — the agent will see the
 *  current form state on every user message via the context block. */
export function buildSystemPrompt(t: Translator): string {
  return `${t('mcp.custom.helper.sys.role')}

${t('mcp.custom.helper.sys.boundaries')}

${t('mcp.custom.helper.sys.action')}

${t('mcp.custom.helper.sys.format')}

KRONN:APPLY
\`\`\`json
{
  "name": "Salesforce Sales API",
  "base_url": "https://my-org.salesforce.com/services/data/v59.0",
  "description": "REST API for Salesforce Sales objects (Account, Contact, Opportunity)",
  "docs_url": "https://developer.salesforce.com/docs/atlas.en-us.api_rest.meta/api_rest/",
  "fields": [
    {"label": "Bearer Token", "value": ""},
    {"label": "Org ID", "value": ""}
  ]
}
\`\`\`

${t('mcp.custom.helper.sys.partial')}

${t('mcp.custom.helper.sys.style')}

${t('mcp.custom.helper.sys.starter')}`;
}

/** Render the current form state as a short context block prepended to
 *  every user message so the agent knows what's already filled vs. blank. */
export function buildContextBlock(
  snapshot: CustomApiAiHelperProps['formSnapshot'],
  t: Translator,
): string {
  const fieldsLine = snapshot.fields.length === 0
    ? t('mcp.custom.helper.ctx.noFields')
    : snapshot.fields.map(f => `  - ${f.label || '(blank)'}${f.value ? ' ✓' : ' (empty)'}`).join('\n');
  return `${t('mcp.custom.helper.ctx.header')}
- name        : ${snapshot.name || t('mcp.custom.helper.ctx.empty')}
- base_url    : ${snapshot.base_url || t('mcp.custom.helper.ctx.empty')}
- description : ${snapshot.description || t('mcp.custom.helper.ctx.empty')}
- docs_url    : ${snapshot.docs_url || t('mcp.custom.helper.ctx.empty')}
- fields      :
${fieldsLine}`;
}

/** Map a parsed KRONN:APPLY object onto a `Partial<CustomApiPayload>`.
 *  We strictly whitelist the known fields so a hallucinating agent can't
 *  inject extra keys; `value` is stripped from incoming fields (the user
 *  fills credentials in, the agent should never propose a value). */
export function applyToCustomForm(parsed: Record<string, unknown>): Partial<CustomApiPayload> {
  const updates: Partial<CustomApiPayload> = {};
  if (typeof parsed.name === 'string') updates.name = parsed.name;
  if (typeof parsed.base_url === 'string') updates.base_url = parsed.base_url;
  if (typeof parsed.description === 'string') updates.description = parsed.description;
  if (typeof parsed.docs_url === 'string') updates.docs_url = parsed.docs_url;
  if (Array.isArray(parsed.fields)) {
    const fields: CustomApiField[] = [];
    for (const raw of parsed.fields) {
      if (raw && typeof raw === 'object' && 'label' in raw) {
        const f = raw as Record<string, unknown>;
        if (typeof f.label === 'string' && f.label.trim()) {
          fields.push({
            label: f.label,
            // Never trust the agent's value — credentials are user-supplied.
            value: typeof f.value === 'string' ? f.value : '',
          });
        }
      }
    }
    if (fields.length > 0) updates.fields = fields;
  }
  return updates;
}

export function CustomApiAiHelper({
  formSnapshot,
  onApply,
  installedAgents,
  configLanguage,
  t,
}: CustomApiAiHelperProps) {
  const agentT = useCallback<Translator>(
    (key, ...args) => translate(toUILocale(configLanguage), key, ...args),
    [configLanguage],
  );

  const [phase, setPhase] = useState<Phase>('closed');
  const [activeAgent, setActiveAgent] = useState<AgentType | null>(null);
  const [agentMenuOpen, setAgentMenuOpen] = useState(false);
  const [discussionId, setDiscussionId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [streaming, setStreaming] = useState(false);
  const streamingRef = useRef(false);
  const [input, setInput] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [minimized, setMinimized] = useState(false);
  const [appliedSignatures, setAppliedSignatures] = useState<Set<string>>(new Set());
  const abortRef = useRef<AbortController | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (phase === 'chatting' && !minimized) {
      const id = requestAnimationFrame(() => inputRef.current?.focus());
      return () => cancelAnimationFrame(id);
    }
  }, [phase, minimized, streaming]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'auto', block: 'end' });
  }, [messages]);

  useEffect(() => {
    return () => {
      abortRef.current?.abort();
      if (discussionId) {
        discussionsApi.delete(discussionId).catch(() => {});
      }
    };
  }, [discussionId]);

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

  const startWithAgentRef = useRef<((agent: AgentType) => Promise<void>) | null>(null);

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
    void startWithAgentRef.current?.(agent);
  }, [activeAgent, streaming, discussionId]);

  const startWithAgent = useCallback(async (agent: AgentType) => {
    setPhase('chatting');
    setActiveAgent(agent);
    setMessages([]);
    setError(null);
    try {
      const disc = await discussionsApi.create({
        project_id: null,
        title: `🤖 ${t('mcp.custom.helper.discTitle')}`,
        agent,
        language: configLanguage ?? 'fr',
        initial_prompt: buildSystemPrompt(agentT),
      });
      setDiscussionId(disc.id);
    } catch (e) {
      console.error('[CustomApiAiHelper] startWithAgent failed:', e);
      setError(String(e));
    }
  }, [t, configLanguage, agentT]);

  useEffect(() => {
    startWithAgentRef.current = startWithAgent;
  }, [startWithAgent]);

  const sendMessage = useCallback(async (overrideText?: string) => {
    const userText = (overrideText ?? input).trim();
    if (!userText || !discussionId || streamingRef.current) return;
    streamingRef.current = true;
    setInput('');
    setError(null);
    setMessages(prev => [...prev, { role: 'user', text: userText }, { role: 'assistant', text: '' }]);
    setStreaming(true);

    const contextBlock = buildContextBlock(formSnapshot, agentT);
    const enriched = `${contextBlock}\n\n${agentT('mcp.custom.helper.sys.userQuestion')}\n${userText}`;

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
        console.error('[CustomApiAiHelper] sendMessageStream error:', err);
        setError(err);
        streamingRef.current = false;
        setStreaming(false);
      },
      controller.signal,
    );
  }, [input, discussionId, formSnapshot, agentT]);

  const stopStream = useCallback(() => {
    abortRef.current?.abort();
    if (discussionId) {
      discussionsApi.stop(discussionId).catch(() => {});
    }
    streamingRef.current = false;
    setStreaming(false);
  }, [discussionId]);

  const handleApply = useCallback((sig: string, parsed: Record<string, unknown>) => {
    onApply(applyToCustomForm(parsed));
    setAppliedSignatures(prev => {
      const next = new Set(prev);
      next.add(sig);
      return next;
    });
  }, [onApply]);

  // ─── Phase: closed ────────────────────────────────────────────────────
  if (phase === 'closed') {
    return (
      <>
        <button
          type="button"
          className="wf-apicall-ai-trigger"
          onClick={() => {
            if (installedAgents.length === 0) {
              setError(t('mcp.custom.helper.noAgents'));
              return;
            }
            void startWithAgent(installedAgents[0]);
          }}
          title={t('mcp.custom.helper.triggerHint')}
        >
          <Sparkles size={11} /> {t('mcp.custom.helper.trigger')}
        </button>
        {error && (
          <span className="wf-apicall-ai-inline-error" role="alert">
            {error}
          </span>
        )}
      </>
    );
  }

  // ─── Phase: chatting ──────────────────────────────────────────────────
  return (
    <>
      <button
        type="button"
        className="wf-apicall-ai-trigger wf-apicall-ai-trigger-active"
        onClick={() => setMinimized(m => !m)}
      >
        <Sparkles size={11} /> {t('mcp.custom.helper.trigger')}
      </button>
      {!minimized && (
        <div className="wf-apicall-ai-bubble" role="dialog" aria-label={t('mcp.custom.helper.bubbleTitle')}>
          <div className="wf-apicall-ai-bubble-header">
            <Bot size={13} />
            <div className="wf-apicall-ai-agent-selector">
              <button
                type="button"
                className="wf-apicall-ai-agent-trigger"
                onClick={() => setAgentMenuOpen(o => !o)}
                disabled={streaming}
                aria-haspopup="listbox"
                aria-expanded={agentMenuOpen}
                title={t('mcp.custom.helper.switchAgent')}
              >
                <span
                  className="wf-apicall-ai-agent-dot"
                  style={{ background: activeAgent ? agentColor(activeAgent) : 'var(--kr-text-ghost)' }}
                />
                <span>{activeAgent ? (AGENT_LABELS[activeAgent] ?? activeAgent) : t('mcp.custom.helper.bubbleTitle')}</span>
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
            <span className="wf-apicall-ai-bubble-eph">{t('mcp.custom.helper.ephemeral')}</span>
            <button
              type="button"
              className="wf-apicall-ai-icon-btn"
              onClick={() => setMinimized(true)}
              title={t('mcp.custom.helper.minimize')}
              aria-label={t('mcp.custom.helper.minimize')}
            >
              <Minus size={12} />
            </button>
            <button
              type="button"
              className="wf-apicall-ai-icon-btn"
              onClick={close}
              title={t('mcp.custom.helper.close')}
              aria-label={t('mcp.custom.helper.close')}
            >
              <X size={12} />
            </button>
          </div>

          <div className="wf-apicall-ai-context-chip wf-apicall-ai-context-chip-top" title={t('mcp.custom.helper.contextHint')}>
            <span>📎</span>
            <span className="wf-apicall-ai-context-chip-label">
              {formSnapshot.name || t('mcp.custom.helper.ctx.unnamed')}
              {formSnapshot.base_url ? ` · ${formSnapshot.base_url}` : ''}
              {formSnapshot.fields.filter(f => f.label.trim()).length > 0
                ? ` · ${t('mcp.custom.helper.ctx.fieldsCount', formSnapshot.fields.filter(f => f.label.trim()).length)}`
                : ''}
            </span>
          </div>

          <div className="wf-apicall-ai-bubble-messages">
            {messages.length === 0 && !streaming && (
              <div className="wf-apicall-ai-welcome">
                <div className="wf-apicall-ai-welcome-title">
                  {t('mcp.custom.helper.welcome')}
                </div>
                <div className="wf-apicall-ai-welcome-chips">
                  <button
                    type="button"
                    className="wf-apicall-ai-starter-chip"
                    onClick={() => {
                      setInput(t('mcp.custom.helper.starter.curlPrompt'));
                      inputRef.current?.focus();
                    }}
                  >
                    <ClipboardPaste size={11} /> {t('mcp.custom.helper.starter.curl')}
                  </button>
                  <button
                    type="button"
                    className="wf-apicall-ai-starter-chip"
                    onClick={() => {
                      setInput(t('mcp.custom.helper.starter.docsPrompt'));
                      inputRef.current?.focus();
                    }}
                  >
                    <Link2 size={11} /> {t('mcp.custom.helper.starter.docs')}
                  </button>
                  <button
                    type="button"
                    className="wf-apicall-ai-starter-chip"
                    onClick={() => {
                      setInput(t('mcp.custom.helper.starter.describePrompt'));
                      inputRef.current?.focus();
                    }}
                  >
                    <MessageSquareText size={11} /> {t('mcp.custom.helper.starter.describe')}
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
                <Loader2 size={11} className="spin" /> {t('mcp.custom.helper.thinking')}
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
                if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  void sendMessage();
                }
              }}
              placeholder={t('mcp.custom.helper.inputPlaceholder')}
              rows={2}
              disabled={streaming}
              autoFocus
            />
            {streaming ? (
              <button
                type="button"
                className="wf-apicall-ai-send-btn wf-apicall-ai-stop-btn"
                onClick={stopStream}
                title={t('mcp.custom.helper.stop')}
                aria-label={t('mcp.custom.helper.stop')}
              >
                <Loader2 size={11} className="spin" />
              </button>
            ) : (
              <button
                type="button"
                className="wf-apicall-ai-send-btn"
                onClick={() => void sendMessage()}
                disabled={!input.trim() || !discussionId}
                title={t('mcp.custom.helper.send')}
                aria-label={t('mcp.custom.helper.send')}
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
          title={t('mcp.custom.helper.restore')}
          aria-label={t('mcp.custom.helper.restore')}
        >
          <Maximize2 size={11} />
        </button>
      )}
    </>
  );
}

// ─── Sub-components (mirrored from ApiCallAiHelper, kept local to avoid
// a fragile cross-component dependency). The shared parser primitive
// `parseApplyBlocks` IS imported from ApiCallAiHelper since it's already
// exported and represents the KRONN:APPLY wire contract. ────────────────

interface ChatMessageViewProps {
  msg: ChatMessage;
  appliedSignatures: Set<string>;
  onApply: (sig: string, parsed: Record<string, unknown>) => void;
  t: Translator;
}

const KRONN_APPLY_RX = /KRONN:APPLY\s*```json\s*([\s\S]*?)```/g;

function ChatMessageView({ msg, appliedSignatures, onApply, t }: ChatMessageViewProps) {
  const blocks = msg.role === 'assistant' ? parseApplyBlocks(msg.text) : [];
  const prose = msg.role === 'assistant' ? msg.text.replace(KRONN_APPLY_RX, '').trim() : msg.text;

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
  t: Translator;
}

function SuggestionCard({ parsed, applied, onApply, t }: SuggestionCardProps) {
  const fields = Object.entries(parsed).filter(([, v]) => v !== undefined && v !== null);
  return (
    <div className={`wf-apicall-ai-suggestion${applied ? ' wf-apicall-ai-suggestion-applied' : ''}`}>
      <div className="wf-apicall-ai-suggestion-header">
        <Sparkles size={11} />
        <span>{t('mcp.custom.helper.suggestion')}</span>
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
        {applied ? t('mcp.custom.helper.applied') : t('mcp.custom.helper.apply')}
      </button>
    </div>
  );
}
