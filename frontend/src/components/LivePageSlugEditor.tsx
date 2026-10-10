import { useState } from 'react';
import { Link2, Loader2, Pencil, Save, X } from 'lucide-react';
import { pages as pagesApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import { standaloneLivePageUrl } from '../lib/live-page-navigation';
import { isValidPageSlug } from '../lib/live-page-slug';
import { userError } from '../lib/userError';
import type { LivePageDetail } from '../types/generated';
import './LivePageSlugEditor.css';

interface LivePageSlugEditorProps {
  page: Pick<LivePageDetail, 'id' | 'slug' | 'slug_aliases'>;
  onSaved: (updated: LivePageDetail) => void;
}

export function LivePageSlugEditor({ page, onSaved }: LivePageSlugEditorProps) {
  const { t } = useT();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(page.slug);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const aliases = page.slug_aliases ?? [];

  const start = () => {
    setDraft(page.slug);
    setError(null);
    setEditing(true);
  };
  const cancel = () => {
    setDraft(page.slug);
    setError(null);
    setEditing(false);
  };
  const save = useAsyncGuard(async () => {
    const slug = draft.trim();
    if (slug === page.slug) {
      cancel();
      return;
    }
    if (!isValidPageSlug(slug)) {
      setError(t('pages.slug.invalid'));
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const updated = await pagesApi.update(page.id, { slug });
      onSaved(updated);
      setEditing(false);
    } catch (cause) {
      setError(userError(cause));
    } finally {
      setSaving(false);
    }
  });

  if (!editing) {
    return (
      <button
        type="button"
        className="live-page-slug-pill"
        onClick={start}
        aria-label={t('pages.slug.edit', page.slug)}
        title={aliases.length > 0 ? t('pages.slug.aliases', aliases.join(', ')) : t('pages.slug.edit', page.slug)}
      >
        <Link2 size={11} />
        <span>{page.slug}</span>
        <Pencil size={10} />
      </button>
    );
  }

  const preview = draft.trim() || page.slug;
  return (
    <form
      className="live-page-slug-editor"
      onSubmit={(event) => { event.preventDefault(); void save(); }}
    >
      <label>
        <span>{t('pages.slug.label')}</span>
        <input
          value={draft}
          onChange={(event) => { setDraft(event.target.value); setError(null); }}
          onKeyDown={(event) => { if (event.key === 'Escape') cancel(); }}
          aria-label={t('pages.slug.input')}
          maxLength={101}
          spellCheck={false}
          autoFocus
          disabled={saving}
        />
      </label>
      <button type="submit" disabled={saving} aria-label={t('pages.slug.save')} title={t('pages.slug.save')}>
        {saving ? <Loader2 size={13} className="spin" /> : <Save size={13} />}
      </button>
      <button type="button" onClick={cancel} disabled={saving} aria-label={t('pages.slug.cancel')} title={t('pages.slug.cancel')}>
        <X size={13} />
      </button>
      <small className="live-page-slug-preview" data-testid="live-page-slug-preview">
        {t('pages.slug.preview', standaloneLivePageUrl(preview))}
      </small>
      <small className="live-page-slug-hint">{t('pages.slug.oldLinksKept', page.slug)}</small>
      {error && <small className="live-page-slug-error" role="alert">{error}</small>}
    </form>
  );
}
