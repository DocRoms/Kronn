import { useState, useRef, useMemo, memo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import '../pages/DiscussionsPage.css';
import type { DiscussionMessage, AgentType } from '../types/generated';
import { agentColor } from '../lib/constants';
import { gravatarUrl } from '../lib/gravatar';
import {
  Cpu, AlertTriangle, Zap, Loader2, Pause, Play,
  Key, Settings, Send, Pencil, RotateCcw, Check, Copy, Clock,
} from 'lucide-react';

// Hoisted regexes (avoid creating new RegExp objects per message per render)
const RE_AUTH_ERROR = /api.?key|invalid.*key|key.*not.*config|authenticat|unauthori|login|sign.?in/i;
const RE_PARTIAL_RESPONSE = /Réponse partielle.*interrompu|Timeout d'inactivité/i;

// ─── MessageBubble component (memo'd to avoid re-rendering all messages) ─────

export interface MessageBubbleProps {
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

export const MessageBubble = memo(function MessageBubble(props: MessageBubbleProps) {
  const { msg, isLastUser, isLastAgent, isEditing, isCopied, isTtsActive, ttsState: tts, isExpandedSummary,
    prevUserTs, defaultAgent, summaryCache, language, sending, editingText, hasFullAccess,
    onCopy, onTts, onEditStart, onEditCancel, onEditSubmit, onEditTextChange, onRetry, onExpandSummary, onNavigate, t } = props;
  const isUser = msg.role === 'User';
  const agentType = msg.agent_type ?? defaultAgent;

  const copyBtn = (size: number, showLabel: boolean) => (
    <button
      className="disc-copy-btn"
      data-copied={isCopied}
      onClick={() => onCopy(msg.id, msg.content)}
      title={t('disc.copyMessage')}
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
    <div className="disc-msg-row" data-role={isUser ? 'user' : msg.role === 'System' ? 'system' : 'agent'}>
      <div
        className="disc-msg-bubble"
        data-role={isUser ? 'user' : msg.role === 'System' ? 'system' : 'agent'}
        data-variant={msg.role === 'System' ? (msg.content.startsWith('summary cached') ? 'summary' : 'error') : undefined}
      >
        {isUser && (msg.author_pseudo || msg.author_avatar_email) && (
          <div className="disc-msg-author">
            {msg.author_avatar_email ? (
              <img src={gravatarUrl(msg.author_avatar_email, 20)} alt="" className="disc-msg-author-avatar" />
            ) : msg.author_pseudo ? (
              <span className="disc-msg-author-initials">
                {msg.author_pseudo.slice(0, 2).toUpperCase()}
              </span>
            ) : null}
            <span className="disc-msg-author-name">{msg.author_pseudo}</span>
          </div>
        )}
        {msg.role === 'Agent' && (
          <div className="disc-msg-agent-label" style={{ color: agentColor(agentType), justifyContent: 'space-between' }}>
            <span className="flex-row gap-2">
              <Cpu size={10} /> {agentType}
            </span>
            {copyBtn(9, false)}
          </div>
        )}
        {msg.role === 'System' && (
          <div className="disc-msg-agent-label" style={{ color: msg.content.startsWith('summary cached') ? '#34d399' : '#ff4d6a' }}>
            {msg.content.startsWith('summary cached') ? <Zap size={10} /> : <AlertTriangle size={10} />}
            {' '}{msg.content.startsWith('summary cached') ? t('disc.summaryCached') : t('disc.system')}
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
        {msg.role === 'System' && msg.content.startsWith('summary cached') && isExpandedSummary && summaryCache && (
          <div className="disc-summary-expanded">
            {summaryCache}
          </div>
        )}
        {isEditing ? (
          <div className="flex-col gap-4">
            <textarea
              value={editingText}
              onChange={e => onEditTextChange(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); onEditSubmit(); } }}
              className="disc-edit-textarea"
              autoFocus
            />
            <div className="disc-edit-actions">
              <button className="disc-icon-btn" style={{ fontSize: 11, padding: '4px 10px', color: 'rgba(255,255,255,0.4)' }} onClick={onEditCancel}>{t('disc.cancel')}</button>
              <button className="disc-scan-btn" style={{ fontSize: 11, padding: '4px 10px' }} onClick={onEditSubmit} disabled={!editingText.trim()}>
                <Send size={10} /> {t('disc.resend')}
                <span className="text-2xs opacity-50" style={{ marginLeft: 4 }}>Ctrl+Enter</span>
              </button>
            </div>
          </div>
        ) : (
          <MarkdownContent content={msg.content.replace(/KRONN:(BRIEFING_COMPLETE|VALIDATION_COMPLETE|BOOTSTRAP_COMPLETE)/gi, '').trim()} />
        )}
        {msg.role === 'Agent' && (
          <button
            className="disc-tts-btn"
            onClick={() => onTts(msg.id, msg.content, language)}
            title={isTtsActive ? (tts === 'loading' ? 'Chargement...' : tts === 'playing' ? 'Pause' : tts === 'paused' ? 'Reprendre' : 'Lire') : 'Lire \u00e0 voix haute'}
          >
            {isTtsActive && tts === 'loading' ? <><Loader2 size={9} style={{ animation: 'spin 1s linear infinite' }} /> TTS</>
              : isTtsActive && tts === 'playing' ? <><Pause size={9} /> Pause</>
              : isTtsActive && tts === 'paused' ? <><Play size={9} /> Reprendre</>
              : <><Play size={9} /> TTS</>}
          </button>
        )}
        {RE_AUTH_ERROR.test(msg.content) && (
          <div className="disc-auth-error-cta">
            <button className="disc-scan-btn" style={{ fontSize: 11, padding: '5px 12px' }} onClick={() => onNavigate('settings')}>
              <Key size={11} /> {t('disc.overrideKey')}
            </button>
            <span className="disc-auth-error-hint">{t('disc.orCheckAgent')}</span>
          </div>
        )}
        {RE_PARTIAL_RESPONSE.test(msg.content) && (
          <div className="disc-auth-error-cta">
            <button className="disc-scan-btn" style={{ fontSize: 11, padding: '5px 12px', borderColor: 'rgba(245,158,11,0.3)', background: 'rgba(245,158,11,0.08)', color: '#f59e0b' }} onClick={() => onNavigate('settings', { scrollTo: 'settings-server' })}>
              <Settings size={11} /> {t('disc.editTimeout')}
            </button>
          </div>
        )}
        <div className="disc-msg-footer">
          <div className="disc-msg-time-row">
            <span className="disc-msg-time">{formattedTime}</span>
            {msg.tokens_used > 0 && <span className="disc-msg-token-count">{msg.tokens_used.toLocaleString()} tok</span>}
            {msg.auth_mode && <span className="disc-msg-auth-mode" data-mode={msg.auth_mode === 'override' ? 'override' : 'local'}>{msg.auth_mode === 'override' ? 'API key' : 'auth locale'}</span>}
            {durationLabel && <span className="disc-msg-duration"><Clock size={8} /> {durationLabel}</span>}
          </div>
          <div className="disc-msg-footer-right">
            {msg.role === 'Agent' && copyBtn(9, true)}
            {msg.role === 'Agent' && hasFullAccess && (
              <span className="disc-full-access-badge">
                <AlertTriangle size={8} /> {t('config.fullAccessBadge')}
              </span>
            )}
            {msg.role === 'Agent' && msg.model_tier && (
              <span className="disc-model-tier-badge" data-tier={msg.model_tier}>
                {msg.model_tier === 'economy' ? '⚡' : '\ud83e\udde0'} {t(`disc.tier.${msg.model_tier}`)}
              </span>
            )}
            {!sending && !isEditing && (isLastUser || isLastAgent) && (
              <div className="flex-row gap-2">
                {isLastUser && (
                  <button className="disc-icon-btn" style={{ padding: '2px 6px', fontSize: 10, color: 'rgba(255,255,255,0.3)' }} onClick={() => onEditStart(msg.id, msg.content)} title={t('disc.editResend')} aria-label={t('disc.editResend')}>
                    <Pencil size={10} />
                  </button>
                )}
                {isLastAgent && (
                  <button className="disc-icon-btn" style={{ padding: '2px 6px', fontSize: 10, color: 'rgba(255,255,255,0.3)' }} onClick={onRetry} title={t('disc.retryResponse')} aria-label={t('disc.retryResponse')}>
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
function CopyableBlock({ children, className, tag }: { children: any; className?: string; tag: 'table' | 'pre' }) {
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

const mdComponents = {
  p: ({ children }: any) => <p>{children}</p>,
  h1: ({ children }: any) => <h1>{children}</h1>,
  h2: ({ children }: any) => <h2>{children}</h2>,
  h3: ({ children }: any) => <h3>{children}</h3>,
  ul: ({ children }: any) => <ul>{children}</ul>,
  ol: ({ children }: any) => <ol>{children}</ol>,
  li: ({ children }: any) => <li>{children}</li>,
  code: ({ className, children }: any) => {
    const isBlock = className?.includes('language-');
    return isBlock
      ? <code className="disc-md-pre-code">{children}</code>
      : <code>{children}</code>;
  },
  pre: ({ children }: any) => (
    <CopyableBlock tag="pre">
      <pre>{children}</pre>
    </CopyableBlock>
  ),
  table: ({ children }: any) => (
    <CopyableBlock tag="table" className="overflow-hidden">
      <table>{children}</table>
    </CopyableBlock>
  ),
  th: ({ children }: any) => <th>{children}</th>,
  td: ({ children }: any) => <td>{children}</td>,
  blockquote: ({ children }: any) => <blockquote>{children}</blockquote>,
  hr: () => <hr />,
  a: ({ href, children }: any) => <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>,
  strong: ({ children }: any) => <strong>{children}</strong>,
};

const remarkPluginsList = [remarkGfm];

export const MarkdownContent = memo(({ content }: { content: string }) => (
  <div className="disc-md">
    <ReactMarkdown remarkPlugins={remarkPluginsList} components={mdComponents}>
      {content}
    </ReactMarkdown>
  </div>
));

// ─── Inline style constants removed — all styles now in DiscussionsPage.css ──
