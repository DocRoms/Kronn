import { useEffect, useMemo, useState } from 'react';
import { FileText, Images, Search, X } from 'lucide-react';
import type { ContextFile } from '../types/generated';
import { MessageAttachments } from './MessageAttachments';

type T = (key: string, ...args: (string | number)[]) => string;
type AssetFilter = 'all' | 'images' | 'files' | 'pending';

const PAGE_SIZE = 40;
const IMAGE_FILENAME = /\.(?:png|jpe?g|gif|webp|svg|bmp|tiff?|ico)$/i;

function isImage(file: ContextFile): boolean {
  return file.mime_type.startsWith('image/') || IMAGE_FILENAME.test(file.filename);
}

export function DiscussionAssetsPanel({
  discussionId,
  files,
  onClose,
  onNavigateMessage,
  t,
}: {
  discussionId: string;
  files: ContextFile[];
  onClose: () => void;
  onNavigateMessage: (messageId: string) => void;
  t: T;
}) {
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<AssetFilter>('all');
  const [visibleCount, setVisibleCount] = useState(PAGE_SIZE);

  useEffect(() => {
    setQuery('');
    setFilter('all');
    setVisibleCount(PAGE_SIZE);
  }, [discussionId]);

  useEffect(() => setVisibleCount(PAGE_SIZE), [query, filter]);

  const counts = useMemo(() => ({
    all: files.length,
    images: files.filter(isImage).length,
    files: files.filter(file => !isImage(file)).length,
    pending: files.filter(file => !file.message_id).length,
  }), [files]);

  const filteredFiles = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return [...files]
      .sort((left, right) => right.created_at.localeCompare(left.created_at))
      .filter(file => {
        if (filter === 'images' && !isImage(file)) return false;
        if (filter === 'files' && isImage(file)) return false;
        if (filter === 'pending' && file.message_id) return false;
        return !needle || file.filename.toLocaleLowerCase().includes(needle);
      });
  }, [files, filter, query]);

  const visibleFiles = filteredFiles.slice(0, visibleCount);
  const filters: Array<{ id: AssetFilter; label: string; icon?: typeof Images }> = [
    { id: 'all', label: t('disc.assets.filterAll') },
    { id: 'images', label: t('disc.assets.filterImages'), icon: Images },
    { id: 'files', label: t('disc.assets.filterFiles'), icon: FileText },
    { id: 'pending', label: t('disc.assets.filterPending') },
  ];

  return (
    <aside className="disc-assets-panel" aria-label={t('disc.assets.title')}>
      <header className="disc-assets-panel-header">
        <div>
          <h2>{t('disc.assets.title')}</h2>
          <p>{t('disc.assets.subtitle', files.length)}</p>
        </div>
        <button
          type="button"
          className="disc-icon-btn"
          onClick={onClose}
          aria-label={t('disc.assets.close')}
          title={t('disc.assets.close')}
        >
          <X size={16} />
        </button>
      </header>

      <div className="disc-assets-panel-tools">
        <label className="disc-assets-search">
          <Search size={14} aria-hidden="true" />
          <input
            type="search"
            value={query}
            onChange={event => setQuery(event.target.value)}
            placeholder={t('disc.assets.search')}
            aria-label={t('disc.assets.search')}
          />
        </label>
        <div className="disc-assets-filters" role="group" aria-label={t('disc.assets.filters')}>
          {filters.map(item => {
            const Icon = item.icon;
            return (
              <button
                key={item.id}
                type="button"
                data-active={filter === item.id}
                onClick={() => setFilter(item.id)}
              >
                {Icon && <Icon size={12} aria-hidden="true" />}
                <span>{item.label}</span>
                <span className="disc-assets-filter-count">{counts[item.id]}</span>
              </button>
            );
          })}
        </div>
      </div>

      <div className="disc-assets-panel-content">
        {visibleFiles.length > 0 ? (
          <>
            <MessageAttachments
              files={visibleFiles}
              discussionId={discussionId}
              t={t}
              variant="library"
              onNavigateMessage={onNavigateMessage}
            />
            {visibleCount < filteredFiles.length && (
              <button
                type="button"
                className="btn btn-sm disc-assets-load-more"
                onClick={() => setVisibleCount(count => count + PAGE_SIZE)}
              >
                {t('disc.assets.loadMore', filteredFiles.length - visibleCount)}
              </button>
            )}
          </>
        ) : (
          <div className="disc-assets-empty">
            <Images size={28} aria-hidden="true" />
            <strong>{t('disc.assets.empty')}</strong>
            <span>{query ? t('disc.assets.emptySearch') : t('disc.assets.emptyHint')}</span>
          </div>
        )}
      </div>
    </aside>
  );
}
