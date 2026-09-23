import { useCallback, useEffect, useRef, useState } from 'react';
import { setup as setupApi } from '../../lib/api';
import type { BrowseListing } from '../../types/generated';
import { useT } from '../../lib/I18nContext';
import { isTauriRuntime, invokeTauri } from '../../lib/tauri';
import { ChevronUp, Folder, GitBranch, Loader2, X } from 'lucide-react';
import './FolderPicker.css';

interface Props {
  /** Paths already chosen, so they come back checked and cannot be added twice. */
  selected: string[];
  onConfirm: (paths: string[]) => void;
  onClose: () => void;
}

/**
 * Select server-visible folder paths. Browser folder inputs return relative
 * names and cannot identify server or container scan roots.
 */
export function FolderPicker({ selected, onConfirm, onClose }: Props) {
  const { t } = useT();
  const [listing, setListing] = useState<BrowseListing | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [picked, setPicked] = useState<string[]>(selected);
  const requestState = useRef({ latest: 0 });

  const open = useCallback(async (path?: string) => {
    const requests = requestState.current;
    const request = ++requests.latest;
    setLoading(true);
    setError(null);
    try {
      const next = await setupApi.browse(path);
      if (request === requests.latest) setListing(next);
    } catch (e) {
      if (request === requests.latest) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (request === requests.latest) setLoading(false);
    }
  }, []);

  // The first listing is fetched without touching state synchronously: the
  // component already starts in its loading state, and a setState in an effect
  // body costs a cascading render.
  useEffect(() => {
    const requests = requestState.current;
    const request = ++requests.latest;
    setupApi
      .browse()
      .then(first => { if (request === requests.latest) setListing(first); })
      .catch(e => { if (request === requests.latest) setError(e instanceof Error ? e.message : String(e)); })
      .finally(() => { if (request === requests.latest) setLoading(false); });
    return () => { requests.latest++; };
  }, []);

  const toggle = (path: string) =>
    setPicked(current =>
      current.includes(path) ? current.filter(p => p !== path) : [...current, path]
    );

  // The desktop build can open the system's own dialog, which knows about
  // places no server-side listing does — a network volume, a recent folder.
  const native = isTauriRuntime();
  const pickNatively = async () => {
    try {
      const chosen = await invokeTauri<string[] | null>('pick_folders');
      if (chosen && chosen.length > 0) onConfirm(chosen);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const parent = listing?.parent ?? null;

  return (
    <div
      className="folder-picker-overlay"
      onClick={e => { if (e.target === e.currentTarget) onClose(); }}
      role="dialog"
      aria-modal="true"
      aria-label={t('setup.browseTitle')}
    >
      <div className="folder-picker">
        <div className="folder-picker-header">
          <h3>{t('setup.browseTitle')}</h3>
          <button type="button" className="btn btn-ghost btn-icon" onClick={onClose} aria-label={t('setup.browseCancel')}>
            <X size={16} />
          </button>
        </div>

        <p className="folder-picker-intro">{t('setup.browseIntro')}</p>

        {listing && listing.roots.length > 0 && (
          <div className="folder-picker-roots">
            <span className="folder-picker-label">{t('setup.browseRoots')}</span>
            {listing.roots.map(root => (
              <button
                key={root.path}
                type="button"
                className={`folder-picker-root${listing.path === root.path ? ' is-current' : ''}`}
                onClick={() => void open(root.path)}
                title={root.path}
              >
                {root.label}
              </button>
            ))}
          </div>
        )}

        {listing?.path && <code className="folder-picker-here">{listing.path}</code>}

        {error && <p className="folder-picker-error">{error}</p>}

        <div className="folder-picker-list">
          {loading ? (
            <div className="folder-picker-loading">
              <Loader2 size={20} style={{ animation: 'spin 1s linear infinite' }} />
            </div>
          ) : (
            <>
              {listing?.path && (
                <label className="folder-picker-row">
                  <input type="checkbox" checked={picked.includes(listing.path)} onChange={() => toggle(listing.path)} />
                  <span>{t('setup.browseSelectCurrent')}</span>
                </label>
              )}
              {parent && (
                <button type="button" className="folder-picker-row is-parent" onClick={() => void open(parent)}>
                  <ChevronUp size={14} />
                  <span>{t('setup.browseParent')}</span>
                </button>
              )}
              {listing && listing.entries.length === 0 && listing.path && (
                <p className="folder-picker-empty">{t('setup.browseEmpty')}</p>
              )}
              {listing?.entries.map(entry => (
                <div key={entry.path} className="folder-picker-row">
                  <input
                    type="checkbox"
                    id={`pick-${entry.path}`}
                    aria-label={t('setup.browseSelectFolder', entry.name)}
                    checked={picked.includes(entry.path)}
                    onChange={() => toggle(entry.path)}
                  />
                  <button type="button" className="folder-picker-name" onClick={() => void open(entry.path)}>
                    {entry.is_repository ? <GitBranch size={14} /> : <Folder size={14} />}
                    <span>{entry.name}</span>
                    {entry.is_repository && <em className="folder-picker-badge">{t('setup.browseIsRepo')}</em>}
                  </button>
                </div>
              ))}
              {listing?.truncated && (
                <p className="folder-picker-empty">{t('setup.browseTruncated', listing.entries.length)}</p>
              )}
            </>
          )}
        </div>

        <div className="folder-picker-footer">
          {native && (
            <button type="button" className="btn btn-ghost btn-sm" onClick={() => void pickNatively()}>
              {t('setup.browseNative')}
            </button>
          )}
          <span className="folder-picker-count">
            {t('setup.browseSelected', picked.length, picked.length > 1 ? 's' : '')}
          </span>
          <button type="button" className="btn btn-secondary" onClick={onClose}>
            {t('setup.browseCancel')}
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={picked.length === 0}
            onClick={() => onConfirm(picked)}
          >
            {t('setup.browseAdd')}
          </button>
        </div>
      </div>
    </div>
  );
}
