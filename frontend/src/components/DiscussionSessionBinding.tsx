import { useCallback, useEffect, useRef, useState } from 'react';
import { Check, Link2, Loader2, Unlink, X } from 'lucide-react';
import { discussions as discussionsApi } from '../lib/api';
import type { DiscSessionStatusResponse } from '../types/generated';
import type { ToastFn } from '../hooks/useToast';
import { CopyIdPill } from './CopyIdPill';

interface SourceBinding {
  binding_version: number;
  disc_id: string;
  source_agent: string;
  source_session_id: string;
  imported_at?: string | null;
  diverged_at?: string | null;
}

interface SourceHistory {
  binding_version: number;
  source_agent: string;
  source_session_id: string;
  linked_at: string;
  unlinked_at?: string | null;
}

interface Props {
  discussionId: string;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

const SOURCE_AGENTS = [
  'ClaudeCode',
  'Codex',
  'GeminiCli',
  'Kiro',
  'CopilotCli',
  'Vibe',
  'Ollama',
  'Custom',
] as const;

export function DiscussionSessionBinding({ discussionId, toast, t }: Props) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [current, setCurrent] = useState<SourceBinding | null>(null);
  const [history, setHistory] = useState<SourceHistory[]>([]);
  const [status, setStatus] = useState<DiscSessionStatusResponse | null>(null);
  const [sourceAgent, setSourceAgent] = useState('ClaudeCode');
  const [sessionId, setSessionId] = useState('');
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);

  const refresh = useCallback(async () => {
    const detail = await discussionsApi.sourceDetail(discussionId);
    const binding = detail.current ?? null;
    setCurrent(binding);
    setHistory(detail.history);
    if (binding) {
      setSourceAgent(binding.source_agent);
      setSessionId(binding.source_session_id);
      const nextStatus = await discussionsApi
        .sourceSessionStatus(binding.source_agent, binding.source_session_id)
        .catch(() => null);
      setStatus(nextStatus);
    } else {
      setStatus(null);
    }
  }, [discussionId]);

  useEffect(() => {
    setOpen(false);
    setError('');
    void refresh().catch(() => {
      // The binding is optional metadata; a failed read must not break chat.
    });
  }, [refresh]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open]);

  const save = async () => {
    const agent = sourceAgent.trim();
    const session = sessionId.trim();
    if (!agent || !session) {
      setError(t('disc.session.required'));
      return;
    }
    setSaving(true);
    setError('');
    try {
      const nextStatus = await discussionsApi.sourceSessionStatus(agent, session);
      if (nextStatus.bound_disc_id && nextStatus.bound_disc_id !== discussionId) {
        setError(t('disc.session.alreadyLinked', nextStatus.bound_disc_id.slice(0, 8)));
        return;
      }
      await discussionsApi.linkSourceSession({
        disc_id: discussionId,
        source_agent: agent,
        source_session_id: session,
        force_reassign: false,
      });
      await refresh();
      window.dispatchEvent(new Event('kronn:disc-source-changed'));
      toast(t('disc.session.linked'), 'success');
    } catch {
      setError(t('disc.session.linkFailed'));
    } finally {
      setSaving(false);
    }
  };

  const unlink = async () => {
    setSaving(true);
    setError('');
    try {
      await discussionsApi.unlinkSourceSession(discussionId);
      setCurrent(null);
      setStatus(null);
      setSessionId('');
      await refresh();
      window.dispatchEvent(new Event('kronn:disc-source-changed'));
      toast(t('disc.session.unlinked'), 'success');
    } catch {
      setError(t('disc.session.unlinkFailed'));
    } finally {
      setSaving(false);
    }
  };

  const connected = Boolean(status?.connected_disc_id);

  return (
    <div className="disc-session-binding" ref={rootRef}>
      <button
        type="button"
        className="disc-session-binding-trigger"
        data-bound={Boolean(current)}
        data-connected={connected}
        onClick={() => setOpen(value => !value)}
        aria-expanded={open}
        aria-haspopup="dialog"
        title={current
          ? t('disc.session.boundTooltip', current.source_agent, current.source_session_id)
          : t('disc.session.link')}
      >
        <Link2 size={9} aria-hidden="true" />
        {current ? (
          <span>{current.source_agent} · {current.source_session_id.slice(0, 8)}</span>
        ) : (
          <span>{t('disc.session.short')}</span>
        )}
      </button>

      {open && (
        <div className="disc-session-binding-popover" role="dialog" aria-label={t('disc.session.title')}>
          <header>
            <div>
              <strong>{t('disc.session.title')}</strong>
              <span>{t('disc.session.contractVersion', current?.binding_version ?? 1)}</span>
            </div>
            <button type="button" onClick={() => setOpen(false)} aria-label={t('common.close')}>
              <X size={13} />
            </button>
          </header>

          {current && (
            <div className="disc-session-current">
              <span className="disc-session-status" data-connected={connected}>
                {connected ? <Check size={10} /> : <Unlink size={10} />}
                {connected ? t('disc.session.connected') : t('disc.session.offline')}
              </span>
              <CopyIdPill
                id={current.source_session_id}
                label={current.source_session_id}
                title={t('disc.session.copy', current.source_session_id)}
              />
            </div>
          )}

          <label>
            <span>{t('disc.session.agent')}</span>
            <select value={sourceAgent} onChange={event => setSourceAgent(event.target.value)}>
              {SOURCE_AGENTS.map(agent => <option key={agent} value={agent}>{agent}</option>)}
            </select>
          </label>
          <label>
            <span>{t('disc.session.id')}</span>
            <input
              value={sessionId}
              onChange={event => setSessionId(event.target.value)}
              placeholder={t('disc.session.idPlaceholder')}
              maxLength={512}
              autoComplete="off"
              spellCheck={false}
            />
          </label>

          {error && <div className="disc-session-error" role="alert">{error}</div>}

          <div className="disc-session-actions">
            {current && (
              <button type="button" onClick={() => void unlink()} disabled={saving}>
                <Unlink size={12} />
                {t('disc.session.unlink')}
              </button>
            )}
            <button
              type="button"
              className="primary"
              onClick={() => void save()}
              disabled={saving || !sessionId.trim()}
            >
              {saving ? <Loader2 size={12} className="spin" /> : <Link2 size={12} />}
              {current ? t('disc.session.update') : t('disc.session.link')}
            </button>
          </div>

          {history.length > 1 && (
            <details className="disc-session-history">
              <summary>{t('disc.session.history', history.length)}</summary>
              {history.slice(0, 5).map(item => (
                <div key={`${item.source_agent}:${item.source_session_id}:${item.linked_at}`}>
                  <span>{item.source_agent}</span>
                  <code>{item.source_session_id.slice(0, 12)}</code>
                  <span>{item.unlinked_at ? t('disc.session.closed') : t('disc.session.current')}</span>
                </div>
              ))}
            </details>
          )}
        </div>
      )}
    </div>
  );
}
