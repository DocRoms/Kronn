/** Where a discussion came from — KT-255.
 *
 *  `source_agent` / `source_session_id` is the CROSS-AGENT MEMORY: it answers
 *  "where does this thread come from", not "who is speaking now". It is what lets
 *  Codex pick up a discussion Claude started, and what `disc_find_by_session` uses
 *  to find the discussion of a CLI that restarted.
 *
 *  So this is READ-ONLY. The binding is established at join, automatically. The
 *  form this replaces asked a human to pick from eight agents and type an opaque
 *  uuid — a gesture that is almost always either wrong or unnecessary, and which
 *  left users unsure what the control even was.
 *
 *  Unlink survives, alone. A stale binding needs a human escape hatch and there is
 *  no other way to clear one from the UI; removing it with the form would have
 *  deleted a repair path without an alternative. */
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

export function DiscussionSessionBinding({ discussionId, toast, t }: Props) {
  return (
    <DiscussionSessionBindingContent
      key={discussionId}
      discussionId={discussionId}
      toast={toast}
      t={t}
    />
  );
}

function DiscussionSessionBindingContent({ discussionId, toast, t }: Props) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [current, setCurrent] = useState<SourceBinding | null>(null);
  const [history, setHistory] = useState<SourceHistory[]>([]);
  const [status, setStatus] = useState<DiscSessionStatusResponse | null>(null);
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);

  const readBinding = useCallback(async () => {
    const detail = await discussionsApi.sourceDetail(discussionId);
    const binding = detail.current ?? null;
    const nextStatus = binding
      ? await discussionsApi
          .sourceSessionStatus(binding.source_agent, binding.source_session_id)
          .catch(() => null)
      : null;
    return { binding, history: detail.history, status: nextStatus };
  }, [discussionId]);

  const applyBinding = useCallback((result: Awaited<ReturnType<typeof readBinding>>) => {
    setCurrent(result.binding);
    setHistory(result.history);
    setStatus(result.status);
  }, []);

  const refresh = useCallback(async () => {
    applyBinding(await readBinding());
  }, [applyBinding, readBinding]);

  useEffect(() => {
    let active = true;
    readBinding()
      .then(result => {
        if (active) applyBinding(result);
      })
      .catch(() => {
        // The binding is optional metadata; a failed read must not break chat.
      });
    return () => { active = false; };
  }, [applyBinding, readBinding]);

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

  const unlink = async () => {
    setSaving(true);
    setError('');
    try {
      await discussionsApi.unlinkSourceSession(discussionId);
      setCurrent(null);
      setStatus(null);
      await refresh();
      window.dispatchEvent(new Event('kronn:disc-source-changed'));
      toast(t('disc.session.unlinked'), 'success');
    } catch {
      setError(t('disc.session.unlinkFailed'));
    } finally {
      setSaving(false);
    }
  };

  // No binding means nothing to explain. The old control still offered itself here
  // as an empty form, which is what left people wondering what it was for.
  if (!current) return null;

  const connected = Boolean(status?.connected_disc_id);

  return (
    <div className="disc-session-binding" ref={rootRef}>
      <button
        type="button"
        className="disc-session-binding-trigger"
        data-bound="true"
        data-connected={connected}
        onClick={() => setOpen(value => !value)}
        aria-expanded={open}
        aria-haspopup="dialog"
        title={t('disc.session.boundTooltip', current.source_agent, current.source_session_id)}
      >
        <Link2 size={9} aria-hidden="true" />
        <span>{current.source_agent} · {current.source_session_id.slice(0, 8)}</span>
      </button>

      {open && (
        <div className="disc-session-binding-popover" role="dialog" aria-label={t('disc.session.title')}>
          <header>
            <div>
              <strong>{t('disc.session.title')}</strong>
              <span>{t('disc.session.contractVersion', current.binding_version)}</span>
            </div>
            <button type="button" onClick={() => setOpen(false)} aria-label={t('common.close')}>
              <X size={13} />
            </button>
          </header>

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

          {/* Says where the binding comes from, so nobody looks for the control
              that used to create one. */}
          <p className="disc-session-origin">{t('disc.session.automatic')}</p>

          {error && <div className="disc-session-error" role="alert">{error}</div>}

          <div className="disc-session-actions">
            <button type="button" onClick={() => void unlink()} disabled={saving}>
              {saving ? <Loader2 size={12} className="spin" /> : <Unlink size={12} />}
              {t('disc.session.unlink')}
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
