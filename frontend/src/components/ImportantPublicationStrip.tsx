// KT-619 — publishing a steering card in your own name, from the composer.
//
// A `kronn-important` fence typed by a human publishes nothing on its own: the
// card needs an enrolled credential and a single-use proof bound to this room
// and this exact body. Without this strip the whole human path existed only on
// the API — a reviewer's words, and they were right: enrolling a grant in
// Settings and then reaching for curl is not a path a person has.
//
// The credential is typed here and held for as long as the page is open. Not
// localStorage, not a URL, not a query string: reloading asks again, which is
// the point — a secret that survives a reload is a secret sitting somewhere.

import { useRef, useState } from 'react';
import { AlertCircle, Info, Send } from 'lucide-react';
import type { ImportantCategory } from '../types/generated';
import './ImportantMessageCard.css';

export interface ImportantPublicationStripProps {
  grant: string;
  onGrantChange: (value: string) => void;
  onPublish: (content: string) => void;
  onOpenSettings: () => void;
  tasks: Array<{ reference: string; title: string }>;
  t: (key: string, ...args: (string | number)[]) => string;
}

type FormCategory = 'information' | 'decision' | 'blocking';

function generatedKey(): string {
  return typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function cardCategory(category: FormCategory): ImportantCategory {
  return category === 'blocking' ? 'blocking_alert' : category;
}

export function ImportantPublicationStrip({
  grant,
  onGrantChange,
  onPublish,
  onOpenSettings,
  tasks,
  t,
}: ImportantPublicationStripProps) {
  const [open, setOpen] = useState(false);
  const [content, setContent] = useState('');
  const [category, setCategory] = useState<FormCategory>('information');
  const [taskRef, setTaskRef] = useState('');
  const publishingRef = useRef(false);
  const dedupKeyRef = useRef(generatedKey());
  const trimmed = content.trim();

  const publish = () => {
    if (!trimmed || publishingRef.current) return;
    publishingRef.current = true;
    const spec = {
      version: 1,
      category: cardCategory(category),
      dedup_key: `human:${dedupKeyRef.current}`,
      title: trimmed.split(/\r?\n/, 1)[0].slice(0, 200),
      highlight: trimmed.slice(0, 500),
      context: trimmed.length > 500 ? trimmed.slice(500, 2500) : undefined,
      impact: trimmed.slice(0, 1000),
      action_required: { required: false },
      references: taskRef ? { task_ref: taskRef } : {},
    };
    onPublish(`\`\`\`kronn-important\n${JSON.stringify(spec)}\n\`\`\``);
    // Keep this draft and key while the page handles retry, preventing a
    // lost receipt from becoming a second publication.
  };

  const cancel = () => {
    setOpen(false);
    setContent('');
    setTaskRef('');
    dedupKeyRef.current = generatedKey();
    publishingRef.current = false;
  };

  return (
    <section className="disc-important-publish" aria-label={t('disc.important.formLabel')}>
      <button type="button" className="disc-important-trigger" onClick={() => setOpen(true)}>
        <Info size={15} aria-hidden="true" /> {t('disc.important.openForm')}
      </button>
      {open && <div className="disc-important-publish-body">
        <label htmlFor="important-content">{t('disc.important.contentLabel')}</label>
        <textarea id="important-content" value={content} onChange={(event) => setContent(event.target.value)} maxLength={2500} autoFocus />
        <label htmlFor="important-category">{t('disc.important.categoryLabel')}</label>
        <select id="important-category" value={category} onChange={(event) => setCategory(event.target.value as FormCategory)}>
          <option value="information">{t('disc.important.category.information')}</option>
          <option value="decision">{t('disc.important.category.decision')}</option>
          <option value="blocking">{t('disc.important.category.blocking')}</option>
        </select>
        <label htmlFor="important-task">{t('disc.important.taskLabel')}</label>
        <select id="important-task" value={taskRef} onChange={(event) => setTaskRef(event.target.value)}>
          <option value="">{t('disc.important.noTask')}</option>
          {tasks.map(task => <option key={task.reference} value={task.reference}>{task.reference} — {task.title}</option>)}
        </select>
        <div className="disc-important-credential">
          <label htmlFor="important-publish-grant">{t('disc.important.grantLabel')}</label>
          <input id="important-publish-grant" type="password" autoComplete="off" value={grant} onChange={(event) => onGrantChange(event.target.value)} placeholder={t('disc.important.grantPlaceholder')} />
          <p className="settings-hint"><AlertCircle size={14} aria-hidden="true" /> {t('disc.important.grantHint')}</p>
          <button type="button" onClick={onOpenSettings}>{t('disc.important.openSettings')}</button>
        </div>
        <div className="disc-important-form-actions">
          <button type="button" onClick={cancel}>{t('common.cancel')}</button>
          <button type="button" disabled={!trimmed || !grant.trim()} onClick={publish}><Send size={14} aria-hidden="true" /> {t('disc.important.publish')}</button>
        </div>
      </div>}
    </section>
  );
}
