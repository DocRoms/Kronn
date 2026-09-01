import { useEffect, useMemo, useState } from 'react';
import { Clapperboard, FileText, Images, Search, Sparkles, X } from 'lucide-react';
import type { ContextFile } from '../types/generated';
import type { ExternalApiConnectionView } from '../lib/api';
import { MessageAttachments } from './MessageAttachments';
import { MediaGenerateForm } from './MediaGenerateForm';
import { mediaKind } from '../lib/mediaKind';

type T = (key: string, ...args: (string | number)[]) => string;
type AssetFilter = 'all' | 'images' | 'videos' | 'files' | 'pending';

const PAGE_SIZE = 40;

// Kind detection is shared with the carousel, so the inventory and the viewer
// never disagree about what a file is. A generated clip used to land under
// "Fichiers" next to a CSV.
function isImage(file: ContextFile): boolean {
  return mediaKind(file) === 'image';
}

function isVideo(file: ContextFile): boolean {
  return mediaKind(file) === 'video';
}

export function DiscussionAssetsPanel({
  discussionId,
  files,
  onClose,
  onNavigateMessage,
  t,
  connections = [],
}: {
  discussionId: string;
  files: ContextFile[];
  onClose: () => void;
  onNavigateMessage: (messageId: string) => void;
  t: T;
  /// External API connections, so a generation can be launched from the tab
  /// that will hold its result. Empty hides the launcher entirely.
  connections?: ExternalApiConnectionView[];
}) {
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<AssetFilter>('all');
  const [visibleCount, setVisibleCount] = useState(PAGE_SIZE);
  const [showGenerate, setShowGenerate] = useState(false);

  useEffect(() => {
    setQuery('');
    setFilter('all');
    setVisibleCount(PAGE_SIZE);
    setShowGenerate(false);
  }, [discussionId]);

  useEffect(() => setVisibleCount(PAGE_SIZE), [query, filter]);

  const counts = useMemo(() => ({
    all: files.length,
    images: files.filter(isImage).length,
    videos: files.filter(isVideo).length,
    files: files.filter(file => !isImage(file) && !isVideo(file)).length,
    pending: files.filter(file => !file.message_id).length,
  }), [files]);

  const filteredFiles = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return [...files]
      .sort((left, right) => right.created_at.localeCompare(left.created_at))
      .filter(file => {
        if (filter === 'images' && !isImage(file)) return false;
        if (filter === 'videos' && !isVideo(file)) return false;
        if (filter === 'files' && (isImage(file) || isVideo(file))) return false;
        if (filter === 'pending' && file.message_id) return false;
        return !needle || file.filename.toLocaleLowerCase().includes(needle);
      });
  }, [files, filter, query]);

  const visibleFiles = filteredFiles.slice(0, visibleCount);
  // The carousel walks the whole discussion, not the current page or filter:
  // opening one asset must reach every image and clip that was generated,
  // which is the point of the tab. Same order as the grid above (newest
  // first), so the counter matches what the eye just clicked.
  const carouselScope = useMemo(
    () => [...files].sort((left, right) => right.created_at.localeCompare(left.created_at)),
    [files],
  );
  // Whether any connection can actually serve a modality. It gates the FORM,
  // not the entry point: hiding the whole block made the feature invisible and
  // left no clue that a media slot has to be configured first — the same
  // mistake as a disabled selector that explains nothing.
  const canGenerate = connections.some(
    connection =>
      (connection.image_model && connection.image_model.trim())
      || (connection.video_model && connection.video_model.trim()),
  );
  const filters: Array<{ id: AssetFilter; label: string; icon?: typeof Images }> = [
    { id: 'all', label: t('disc.assets.filterAll') },
    { id: 'images', label: t('disc.assets.filterImages'), icon: Images },
    { id: 'videos', label: t('disc.assets.filterVideos'), icon: Clapperboard },
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

      <div className="disc-assets-generate">
        <button
          type="button"
          className="btn btn-sm"
          onClick={() => setShowGenerate(open => !open)}
          aria-expanded={showGenerate}
          data-testid="assets-generate-toggle"
        >
          <Sparkles size={13} aria-hidden="true" />
          <span>{showGenerate ? t('disc.media.closeForm') : t('disc.media.newAsset')}</span>
        </button>
        {!canGenerate && (
          // Stated without a click: the reason it cannot run yet, and where to
          // fix it. Discovering that through an empty form would be worse.
          <p className="disc-assets-generate-hint" data-testid="assets-generate-hint">
            {t('disc.media.noSlot')}
          </p>
        )}
        {showGenerate && (
          <MediaGenerateForm
            discussionId={discussionId}
            connections={connections}
            t={t}
          />
        )}
      </div>

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
              carouselScope={carouselScope}
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
