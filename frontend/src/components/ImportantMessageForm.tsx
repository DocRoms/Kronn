import { useCallback, useEffect, useId, useRef, useSyncExternalStore } from 'react';
import { AlertCircle, Info, Send } from 'lucide-react';
import { emptyDraft, importantMessageFence, newId, pending, readDraft, subscribeImportantDraft, updateDraft } from '../lib/importantMessageDraft';
import type { Draft, FormCategory } from '../lib/importantMessageDraft';
import type { ImportantPublishOutcome, ImportantSubmission } from '../lib/submitImportantMessage';
import './ImportantMessageCard.css';


export interface ImportantMessageFormProps {
  discussionId: string;
  grant: string;
  onGrantChange: (value: string) => void;
  onPublish: (submission: ImportantSubmission) => Promise<ImportantPublishOutcome>;
  onOpenSettings: () => void;
  tasks: Array<{ reference: string; title: string }>;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ImportantMessageForm({ discussionId, grant, onGrantChange, onPublish, onOpenSettings, tasks, t }: ImportantMessageFormProps) {
  const prefix = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const subscribe = useCallback((listener: () => void) => {
    return subscribeImportantDraft(discussionId, listener);
  }, [discussionId]);
  const snapshot = useCallback(() => readDraft(discussionId), [discussionId]);
  const draft = useSyncExternalStore(subscribe, snapshot, snapshot);
  const busy = draft.state === 'preparing' || draft.state === 'sending';
  const locked = busy || draft.state === 'uncertain';
  const missingTask = !!draft.taskRef && !tasks.some(task => task.reference === draft.taskRef);

  useEffect(() => () => {
    const active = pending.get(discussionId);
    if (active && !active.sending) active.controller.abort();
  }, [discussionId]);

  const edit = (patch: Partial<Draft>) => {
    if (locked) return;
    updateDraft(discussionId, { ...patch, state: 'editing', submitted: undefined });
  };
  const close = () => {
    const active = pending.get(discussionId);
    if (active) { if (!active.sending) active.controller.abort(); return; }
    updateDraft(discussionId, { open: false });
    trigger.current?.focus();
  };
  const publish = async () => {
    const current = readDraft(discussionId);
    if (pending.has(discussionId) || !current.content.trim() || !grant.trim() || missingTask) return;
    const active = { controller: new AbortController(), sending: false };
    pending.set(discussionId, active);
    const body = current.submitted ?? importantMessageFence(current.content, current.category, current.taskRef, current.factId, t('disc.important.unspecifiedImpact'));
    updateDraft(discussionId, { state: 'preparing', submitted: body });
    let outcome: ImportantPublishOutcome;
    try {
      outcome = await onPublish({
        discussionId, content: body, grant, dedupKey: `human:${current.factId}`,
        clientMessageId: current.clientMessageId, signal: active.controller.signal,
        reconcileFirst: current.state === 'uncertain',
        onSending: () => {
          if (pending.get(discussionId) !== active) return;
          active.sending = true;
          updateDraft(discussionId, { state: 'sending' });
        },
      });
    } catch { outcome = active.sending ? 'uncertain' : 'refused'; }
    if (pending.get(discussionId) !== active) return;
    pending.delete(discussionId);
    if (outcome === 'confirmed') {
      updateDraft(discussionId, emptyDraft());
      if (trigger.current?.isConnected) trigger.current.focus();
    } else {
      updateDraft(discussionId, {
        state: outcome,
        // A known text-only row cannot be retroactively made important. An
        // explicit retry is a new message; an uncertain write keeps its UUID.
        ...(outcome === 'text-only' ? { clientMessageId: newId() } : {}),
        ...(outcome !== 'uncertain' ? { submitted: undefined } : {}),
      });
    }
  };

  return <section className="disc-important-publish" aria-label={t('disc.important.formLabel')}>
    <button ref={trigger} type="button" className="disc-important-trigger" disabled={draft.state === 'sending'} aria-expanded={draft.open} aria-controls={`${prefix}-form`} onClick={() => draft.open ? close() : updateDraft(discussionId, { open: true })}>
      <Info size={15} aria-hidden="true" /> {t('disc.important.openForm')}
    </button>
    {draft.open && <div id={`${prefix}-form`} className="disc-important-publish-body" onKeyDown={event => {
      if (event.key === 'Escape') { event.stopPropagation(); close(); }
    }}>
      <p className="settings-hint">{t('disc.important.formHint')}</p>
      <label htmlFor={`${prefix}-content`}>{t('disc.important.contentLabel')}</label>
      <textarea id={`${prefix}-content`} value={draft.content} onChange={event => edit({ content: event.target.value })} maxLength={2500} autoFocus disabled={locked} />
      <label htmlFor={`${prefix}-category`}>{t('disc.important.categoryLabel')}</label>
      <select id={`${prefix}-category`} value={draft.category} disabled={locked} onChange={event => edit({ category: event.target.value as FormCategory })}>
        <option value="information">{t('disc.important.category.information')}</option>
        <option value="decision">{t('disc.important.category.decision')}</option>
        <option value="blocking">{t('disc.important.category.blocking')}</option>
      </select>
      <label htmlFor={`${prefix}-task`}>{t('disc.important.taskLabel')}</label>
      <select id={`${prefix}-task`} value={draft.taskRef} disabled={locked} onChange={event => edit({ taskRef: event.target.value })}>
        <option value="">{t('disc.important.noTask')}</option>
        {missingTask && <option value={draft.taskRef} disabled>{draft.taskRef} — {t('disc.important.taskUnavailable')}</option>}
        {tasks.map(task => <option key={task.reference} value={task.reference}>{task.reference} — {task.title}</option>)}
      </select>
      <div className="disc-important-credential">
        <label htmlFor={`${prefix}-grant`}>{t('disc.important.grantLabel')}</label>
        <input id={`${prefix}-grant`} type="password" autoComplete="off" value={grant} disabled={busy} onChange={event => onGrantChange(event.target.value)} placeholder={t('disc.important.grantPlaceholder')} />
        <p className="settings-hint"><AlertCircle size={14} aria-hidden="true" /> {t('disc.important.grantHint')}</p>
        <button type="button" disabled={busy} onClick={onOpenSettings}>{t('disc.important.openSettings')}</button>
      </div>
      {draft.state !== 'editing' && <p role="status" className="settings-hint">{t(`disc.important.status.${draft.state}`)}</p>}
      <div className="disc-important-form-actions">
        <button type="button" disabled={draft.state === 'sending'} onClick={close}>{draft.state === 'preparing' ? t('disc.important.cancelPreparing') : t('common.cancel')}</button>
        <button type="button" disabled={!draft.content.trim() || !grant.trim() || busy || missingTask} onClick={() => void publish()}><Send size={14} aria-hidden="true" /> {t('disc.important.publish')}</button>
      </div>
    </div>}
  </section>;
}
