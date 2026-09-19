import { useCallback, useEffect, useMemo, useState } from 'react';
import { Clapperboard, FileText, Film, Images, LayoutGrid, Search, Sparkles, X } from 'lucide-react';
import type { ContextFile } from '../types/generated';
import type { ExternalApiConnectionView } from '../lib/api';
import { MessageAttachments, type ImageGenerationRequest } from './MessageAttachments';
import { MediaGenerateForm } from './MediaGenerateForm';
import { VideoSequenceEditor } from './VideoSequenceEditor';
import { mediaKind } from '../lib/mediaKind';

type T = (key: string, ...args: (string | number)[]) => string;
type AssetFilter = 'all' | 'images' | 'videos' | 'files' | 'pending';
/// Three uses of one inventory: finding an asset, making one, and putting the
/// clips in the order they play as a film.
type AssetTab = 'all' | 'creator' | 'editor';

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
  onMediaLaunched,
  openAssetRequest,
  onAssetDeleted,
  onAssetExtracted,
}: {
  discussionId: string;
  files: ContextFile[];
  onClose: () => void;
  onNavigateMessage: (messageId: string) => void;
  t: T;
  /// External API connections, so a generation can be launched from the tab
  /// that will hold its result. Empty hides the launcher entirely.
  connections?: ExternalApiConnectionView[];
  /// Fired once the backend accepted a job, so the discussion can reveal the
  /// fresh anchor message the inline placeholder renders at.
  onMediaLaunched?: (jobId: string, messageId: string) => void;
  /** One-shot request from a transcript media bubble. The nonce lets the same
   * asset be deliberately opened again after the viewer was closed. */
  openAssetRequest?: { assetId: string; nonce: number } | null;
  /// Fired once the server confirmed a deletion, so the discussion drops the
  /// file from its own inventory. Absent, the panel offers no deletion.
  onAssetDeleted?: (fileId: string) => void;
  /// Fired once an image the server accepted joins this discussion — a frame
  /// extracted from a clip, or a picture attached from the launcher — so it
  /// enters the inventory and can be picked as a starting point right away.
  /// Absent, the panel offers neither extraction nor attachment.
  onAssetExtracted?: (file: ContextFile) => void;
}) {
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<AssetFilter>('all');
  // KT-587 — keyed by the filter it counts for, so a narrower search starts at
  // one page again without an effect writing that reset one render late.
  const [paging, setPaging] = useState({ key: '', count: PAGE_SIZE });
  // Floor imposed by a targeted open request, so the grid behind the viewer has
  // really scrolled to the asset. Kept apart from `visibleCount`: the same
  // request clears the query and filter, and their own reset would undo it.
  const [pinnedCount, setPinnedCount] = useState(0);
  const [tab, setTab] = useState<AssetTab>('all');
  const [generationRequest, setGenerationRequest] = useState<(ImageGenerationRequest & { nonce: number }) | null>(null);
  const clearPagination = useCallback(() => setPinnedCount(0), []);

  useEffect(() => {
    setQuery('');
    setFilter('all');
    setPaging({ key: '', count: PAGE_SIZE });
    setPinnedCount(0);
    setTab('all');
    setGenerationRequest(null);
  }, [discussionId]);

  // KT-587 — a request to reveal one asset is an event, not state to keep in
  // step. React's own shape for that is an adjustment during render: the reset
  // lands in the same pass, where an effect showed the old filter first and the
  // asset appeared to be missing for a frame.
  // Unanswered at mount: the panel is often opened BY the request, so seeding
  // this with it would swallow the very request that opened the panel.
  const [answeredRequest, setAnsweredRequest] = useState<typeof openAssetRequest>(null);
  if (openAssetRequest && openAssetRequest !== answeredRequest) {
    setAnsweredRequest(openAssetRequest);
    // The viewer lives in the inventory, so an asset opened from the
    // transcript lands there whatever tab was showing.
    setTab('all');
    setQuery('');
    setFilter('all');
    const ordered = [...files].sort((left, right) => right.created_at.localeCompare(left.created_at));
    const index = ordered.findIndex(file => file.id === openAssetRequest.assetId);
    setPinnedCount(index >= 0 ? index + 1 : 0);
  }

  const videos = useMemo(() => files.filter(isVideo), [files]);
  // An order needs at least two clips; with fewer the tab would only ever
  // show a list of one.
  const editorAvailable = videos.length >= 2;
  const activeTab: AssetTab = tab === 'editor' && !editorAvailable ? 'all' : tab;

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

  // The floor only ever widens the page, never narrows it.
  const pagingKey = `${filter}\u0000${query}`;
  const visibleCount = paging.key === pagingKey ? paging.count : PAGE_SIZE;
  const shownCount = Math.max(visibleCount, pinnedCount);
  const visibleFiles = filteredFiles.slice(0, shownCount);
  // The carousel walks the whole discussion, not the current page or filter:
  // opening one asset must reach every image and clip that was generated,
  // which is the point of the tab. Same order as the grid above (newest
  // first), so the counter matches what the eye just clicked.
  const carouselScope = useMemo(
    () => [...files].sort((left, right) => right.created_at.localeCompare(left.created_at)),
    [files],
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

      <div className="disc-assets-tabs" role="tablist" aria-label={t('disc.assets.tabs')}>
        {([
          { id: 'all', label: t('disc.assets.tabAll'), icon: LayoutGrid },
          { id: 'creator', label: t('disc.assets.tabCreator'), icon: Sparkles },
          ...(editorAvailable ? [{ id: 'editor', label: t('disc.assets.tabEditor'), icon: Film }] : []),
        ] as Array<{ id: AssetTab; label: string; icon: typeof Film }>).map(item => {
          const Icon = item.icon;
          return (
            <button
              key={item.id}
              type="button"
              role="tab"
              aria-selected={activeTab === item.id}
              data-active={activeTab === item.id}
              data-testid={`assets-tab-${item.id}`}
              onClick={() => setTab(item.id)}
            >
              <Icon size={13} aria-hidden="true" />
              <span>{item.label}</span>
              {item.id === 'editor' && <span className="disc-assets-filter-count">{videos.length}</span>}
            </button>
          );
        })}
      </div>

      {activeTab === 'creator' && (
        <div className="disc-assets-generate" role="tabpanel">
          {/* Always offered: with no media slot configured, the form itself
              says so and where to fix it, which a hidden entry never did. */}
          <MediaGenerateForm
            key={generationRequest?.nonce ?? 0}
            discussionId={discussionId}
            connections={connections}
            /* The images already in this room are the only ones a generation
               may start from, so the launcher gets the inventory it stands in
               rather than a separate picker of its own. */
            images={files.filter(isImage)}
            t={t}
            onLaunched={onMediaLaunched}
            onImageAttached={onAssetExtracted}
            initialSlotKey={generationRequest?.slotKey}
            initialReference={generationRequest
              ? { assetId: generationRequest.assetId, mode: generationRequest.referenceMode }
              : null}
          />
        </div>
      )}

      {activeTab === 'editor' && (
        <div className="disc-assets-panel-content" role="tabpanel">
          <VideoSequenceEditor key={discussionId} discussionId={discussionId} videos={videos} t={t} />
        </div>
      )}

      {activeTab === 'all' && (
        <>
          <div className="disc-assets-panel-tools">
            <label className="disc-assets-search">
              <Search size={14} aria-hidden="true" />
              <input
                type="search"
                value={query}
                onChange={event => { clearPagination(); setQuery(event.target.value); }}
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
                    onClick={() => { clearPagination(); setFilter(item.id); }}
                  >
                    {Icon && <Icon size={12} aria-hidden="true" />}
                    <span>{item.label}</span>
                    <span className="disc-assets-filter-count">{counts[item.id]}</span>
                  </button>
                );
              })}
            </div>
          </div>

          <div className="disc-assets-panel-content" role="tabpanel">
            {visibleFiles.length > 0 ? (
              <>
                <MessageAttachments
                  files={visibleFiles}
                  discussionId={discussionId}
                  t={t}
                  variant="library"
                  onNavigateMessage={onNavigateMessage}
                  carouselScope={carouselScope}
                  openRequest={openAssetRequest}
                  onDeleted={onAssetDeleted}
                  onExtracted={onAssetExtracted}
                  generationConnections={connections}
                  onGenerateFromImage={request => {
                    setGenerationRequest(previous => ({ ...request, nonce: (previous?.nonce ?? 0) + 1 }));
                    setTab('creator');
                  }}
                />
                {shownCount < filteredFiles.length && (
                  <button
                    type="button"
                    className="btn btn-sm disc-assets-load-more"
                    onClick={() => setPaging({ key: pagingKey, count: shownCount + PAGE_SIZE })}
                  >
                    {t('disc.assets.loadMore', filteredFiles.length - shownCount)}
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
        </>
      )}
    </aside>
  );
}
