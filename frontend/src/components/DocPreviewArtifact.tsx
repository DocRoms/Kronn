import { useRef, useState } from 'react';
import { ExternalLink, FilePlus2, Loader2 } from 'lucide-react';
import { pages } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { standaloneLivePageUrl } from '../lib/live-page-navigation';
import { userError } from '../lib/userError';
import type { LivePage } from '../types/generated';
import './DocPreviewArtifact.css';

export function DocPreviewArtifact({ html, discussionId, sourceMessageId }: {
  html: string; discussionId: string; sourceMessageId: string;
}) {
  const { t } = useT();
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState('');
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<{ html: string; discussionId: string; sourceMessageId: string; page: LivePage } | null>(null);
  const current = created?.html === html && created.discussionId === discussionId && created.sourceMessageId === sourceMessageId ? created.page : null;

  const create = async () => {
    if (inFlight.current || !title.trim() || current) return;
    inFlight.current = true; setBusy(true); setError(null);
    try {
      const page = await pages.create({ title: title.trim(), html, discussion_id: discussionId, source_message_id: sourceMessageId,
        project_id: null, slug: null, created_by_agent: null, datasets: [] });
      setCreated({ html, discussionId, sourceMessageId, page });
      setEditing(false);
      window.dispatchEvent(new Event('kronn:pages-activated'));
    } catch (cause) { setError(userError(cause)); }
    finally { inFlight.current = false; setBusy(false); }
  };

  if (current) return <a className="doc-preview-download" href={standaloneLivePageUrl(current.id)} target="_blank" rel="noopener noreferrer">
    <ExternalLink size={12} />{t('disc.docArtifactOpen', current.title)}
  </a>;
  return <div className="doc-preview-artifact">
    {!editing ? <button type="button" className="doc-preview-btn" disabled={busy} onClick={() => {
      setTitle(t('disc.docArtifactDefaultTitle')); setEditing(true); setError(null);
    }}><FilePlus2 size={12} />{t('disc.docArtifactCreate')}</button> : <form onSubmit={event => { event.preventDefault(); void create(); }}>
      <label>{t('disc.docArtifactTitle')}<input autoFocus maxLength={200} value={title} disabled={busy} onChange={event => setTitle(event.target.value)} /></label>
      <button type="submit" className="doc-preview-btn" disabled={busy || !title.trim()}>
        {busy ? <Loader2 size={12} className="doc-preview-spinner" /> : <FilePlus2 size={12} />}{t('disc.docArtifactConfirm')}
      </button>
      <button type="button" className="doc-preview-btn" disabled={busy} onClick={() => setEditing(false)}>{t('common.cancel')}</button>
    </form>}
    {error && <p className="doc-preview-error" role="alert">{error}</p>}
  </div>;
}
