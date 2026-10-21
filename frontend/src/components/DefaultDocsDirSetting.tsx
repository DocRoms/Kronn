// KT-263 — the default AI doc folder, relative to every project root.
import { useEffect, useState } from 'react';
import { BookOpen } from 'lucide-react';
import { config as configApi } from '../lib/api';
import { userError } from '../lib/userError';
import { useAsyncGuard } from '../hooks/useAsyncGuard';

interface Props {
  t: (key: string, ...args: (string | number)[]) => string;
}

export function DefaultDocsDirSetting({ t }: Props) {
  const [saved, setSaved] = useState<string | null>(null);
  const [draft, setDraft] = useState('');
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => configApi.getDefaultDocsDir())
      .then(value => {
        if (!live) return;
        setSaved(value ?? null);
        setDraft(value ?? '');
        setLoaded(true);
      })
      .catch(e => { if (live) setError(userError(e)); });
    return () => { live = false; };
  }, []);

  const save = useAsyncGuard(async () => {
    setError(null);
    try {
      const value = await configApi.saveDefaultDocsDir(draft.trim() || null);
      setSaved(value ?? null);
      setDraft(value ?? '');
    } catch (e) {
      setError(userError(e));
    }
  });

  const unchanged = draft.trim() === (saved ?? '');

  return (
    <div className="mb-8 set-inner-divider" data-testid="default-docs-dir">
      <div className="flex-row gap-4 mb-3">
        <BookOpen size={12} className="text-muted" />
        <span className="label">{t('config.defaultDocsDir')}</span>
      </div>
      <p className="text-sm text-faint mb-4">{t('config.defaultDocsDirHint')}</p>
      <div className="flex-row gap-3">
        <input
          type="text"
          className="set-input set-input-sm flex-1"
          placeholder={t('config.defaultDocsDirPlaceholder')}
          aria-label={t('config.defaultDocsDir')}
          value={draft}
          disabled={!loaded}
          onChange={e => setDraft(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter' && !unchanged) void save(); }}
          data-testid="default-docs-dir-input"
        />
        <button
          type="button"
          className="set-icon-btn text-accent"
          disabled={!loaded || unchanged}
          onClick={() => void save()}
          data-testid="default-docs-dir-save"
        >
          {t('common.save')}
        </button>
      </div>
      {error && <p role="alert" className="default-docs-dir-error">{error}</p>}
    </div>
  );
}
