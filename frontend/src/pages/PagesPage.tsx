import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Activity, Archive, CheckCircle2, CheckSquare2, ChevronDown, ChevronRight,
  Braces, Database, Download, ExternalLink, FileCode2, FileDown, GitCompare,
  History, ListChecks, Loader2, MessageSquare, Pencil, Play, RefreshCw,
  RotateCcw, Save, Star, Table2, Trash2, Workflow, X,
} from 'lucide-react';
import type {
  LivePage, LivePageDetail, LivePageDiscussionLink, LivePagePublication,
  LivePageRevision, LivePageWorkflowLink,
} from '../types/generated';
import { docs as docsApi, pages as pagesApi, workflows as workflowsApi } from '../lib/api';
import { datasetRecords, recordsToRows } from '../lib/live-page-csv';
import {
  buildSandboxDocument,
  createLivePageOpenLinkRelay,
  requestRenderedPageHtml,
  runtimeData,
} from '../lib/live-page-sandbox';
import { formatRelativeTime } from '../lib/relativeTime';
import { CopyIdPill } from '../components/CopyIdPill';
import { FavoriteToggle } from '../components/FavoriteToggle';
import { CollectionShell, CollectionSidebarCollapseButton } from '../components/CollectionShell';
import { HtmlCodeEditor, HtmlRevisionDiff } from '../components/HtmlCodeEditor';
import { useT } from '../lib/I18nContext';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import { userError } from '../lib/userError';
import { standaloneLivePageUrl } from '../lib/live-page-navigation';
import './DiscussionsPage.css';
import './PagesPage.css';

const REFRESH_MS = 30_000;
const PAGE_NAVIGATION_STORAGE_KEY = 'kronn:pageNavigation';
const PAGE_COLLAPSED_STORAGE_KEY = 'kronn:pageCollapsedSections';
const PAGE_SECTIONS = new Set(['favorites', 'pages', 'archives']);

interface PageNavigationPreference {
  resourceId: string | null;
}

function readPageNavigation(): PageNavigationPreference {
  try {
    const parsed = JSON.parse(localStorage.getItem(PAGE_NAVIGATION_STORAGE_KEY) ?? '{}') as {
      resourceId?: unknown;
    };
    return { resourceId: typeof parsed.resourceId === 'string' ? parsed.resourceId : null };
  } catch {
    return { resourceId: null };
  }
}

function readCollapsedPageSections(): Set<string> {
  try {
    const parsed = JSON.parse(localStorage.getItem(PAGE_COLLAPSED_STORAGE_KEY) ?? '[]') as unknown;
    if (!Array.isArray(parsed)) return new Set(['archives']);
    return new Set(parsed.filter((section): section is string => (
      typeof section === 'string' && PAGE_SECTIONS.has(section)
    )));
  } catch {
    return new Set(['archives']);
  }
}

function channelId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `page-${Date.now()}-${Math.random()}`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

// KT-463 — the Export and Sync popovers are native <details>/<summary>,
// uncontrolled by React. Native <details> never closes itself on an
// outside click or Escape; only clicking <summary> again toggles it. This
// mutates the DOM's own `open` property directly instead of introducing
// React state, since nothing else needs to observe or drive that state.
function useDismissibleDetails<T extends HTMLDetailsElement>() {
  const ref = useRef<T>(null);
  useEffect(() => {
    const close = () => {
      const el = ref.current;
      if (el?.open) el.open = false;
    };
    const onPointerDown = (event: MouseEvent) => {
      const el = ref.current;
      if (el?.open && !el.contains(event.target as Node)) close();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      const el = ref.current;
      if (el?.open && event.key === 'Escape') {
        close();
        // Keyboard users lose their place once the popover's content
        // collapses out from under a focused element inside it.
        el.querySelector('summary')?.focus();
      }
    };
    document.addEventListener('mousedown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, []);
  return ref;
}

interface PagesPageProps {
  initialSelectedPageId?: string | null;
  onInitialSelectionConsumed?: () => void;
  onNavigateWorkflow?: (workflowId: string, runId?: string) => void;
  onNavigateDiscussion?: (discussionId: string) => void;
}

export function PagesPage({
  initialSelectedPageId,
  onInitialSelectionConsumed,
  onNavigateWorkflow,
  onNavigateDiscussion,
}: PagesPageProps) {
  const { locale, t } = useT();
  const [initialPageNavigation] = useState(readPageNavigation);
  const [pages, setPages] = useState<LivePage[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(
    initialSelectedPageId ?? initialPageNavigation.resourceId,
  );
  const [detail, setDetail] = useState<LivePageDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [linkedWorkflows, setLinkedWorkflows] = useState<LivePageWorkflowLink[]>([]);
  const [recentPublications, setRecentPublications] = useState<LivePagePublication[]>([]);
  const [linkedDiscussions, setLinkedDiscussions] = useState<LivePageDiscussionLink[]>([]);
  const [revisions, setRevisions] = useState<LivePageRevision[]>([]);
  const [query, setQuery] = useState('');
  const [collapsedSections, setCollapsedSections] = useState<Set<string>>(readCollapsedPageSections);
  const [selectionMode, setSelectionMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [bulkBusy, setBulkBusy] = useState(false);
  const [editingHtml, setEditingHtml] = useState(false);
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState('');
  const [titleError, setTitleError] = useState<string | null>(null);
  const [savingTitle, setSavingTitle] = useState(false);
  const [htmlDraft, setHtmlDraft] = useState('');
  const [comparisonRevisionId, setComparisonRevisionId] = useState<string | null>(null);
  const [savingHtml, setSavingHtml] = useState(false);
  const [runningWorkflowId, setRunningWorkflowId] = useState<string | null>(null);
  const [workflowRunFeedback, setWorkflowRunFeedback] = useState<{
    workflowId: string;
    kind: 'success' | 'error';
    message: string;
  } | null>(null);
  const [exportBusy, setExportBusy] = useState<'pdf' | 'docx' | null>(null);
  const [exportResult, setExportResult] = useState<{ url: string; filename: string } | null>(null);
  const exportMenuRef = useDismissibleDetails<HTMLDetailsElement>();
  const refreshMenuRef = useDismissibleDetails<HTMLDetailsElement>();
  const [selectedDatasetId, setSelectedDatasetId] = useState<string | null>(null);
  const [datasetExportBusy, setDatasetExportBusy] = useState(false);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const linkRelayRef = useRef<ReturnType<typeof createLivePageOpenLinkRelay> | null>(null);
  const [bridgeChannel] = useState(channelId);
  const requestedPageIdRef = useRef(initialSelectedPageId);
  const selectionConsumedRef = useRef(onInitialSelectionConsumed);

  useEffect(() => {
    if (initialSelectedPageId) requestedPageIdRef.current = initialSelectedPageId;
    selectionConsumedRef.current = onInitialSelectionConsumed;
  }, [initialSelectedPageId, onInitialSelectionConsumed]);

  useEffect(() => {
    try {
      localStorage.setItem(PAGE_NAVIGATION_STORAGE_KEY, JSON.stringify({ resourceId: selectedId }));
    } catch {
      // localStorage may be unavailable in private/restricted browser modes.
    }
  }, [selectedId]);

  useEffect(() => {
    try {
      localStorage.setItem(PAGE_COLLAPSED_STORAGE_KEY, JSON.stringify([...collapsedSections]));
    } catch {
      // localStorage may be unavailable in private/restricted browser modes.
    }
  }, [collapsedSections]);

  const loadDetail = useCallback(async (pageId: string) => {
    const [nextDetail, workflows, publications, discussions, pageRevisions] = await Promise.all([
      pagesApi.get(pageId),
      pagesApi.workflows(pageId),
      pagesApi.publications(pageId),
      pagesApi.discussions(pageId),
      pagesApi.revisions(pageId),
    ]);
    setDetail(nextDetail);
    setLinkedWorkflows(workflows);
    setRecentPublications(publications);
    setLinkedDiscussions(discussions);
    setRevisions(pageRevisions);
    setComparisonRevisionId(null);
    setHtmlDraft(nextDetail.revision.html);
  }, []);

  const refresh = useCallback(async (selectedOverride?: string | null) => {
    try {
      const list = await pagesApi.list();
      setPages(list);
      const requestedId = requestedPageIdRef.current;
      const requested = requestedId && list.some(page => page.id === requestedId)
        ? requestedId
        : null;
      const candidate = selectedOverride === undefined ? selectedId : selectedOverride;
      const target = requested
        ?? (candidate && list.some(page => page.id === candidate) ? candidate : null)
        ?? list.find(page => !page.archived)?.id
        ?? list[0]?.id
        ?? null;
      setSelectedId(target);
      if (target) {
        await loadDetail(target);
      } else {
        setDetail(null);
        setLinkedWorkflows([]);
        setRecentPublications([]);
        setLinkedDiscussions([]);
        setRevisions([]);
      }
      if (requested) {
        requestedPageIdRef.current = null;
        selectionConsumedRef.current?.();
      }
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, [loadDetail, selectedId]);

  // Initial remote-library synchronization; the state updates happen after
  // the request resolves, not synchronously in the effect body.
  // eslint-disable-next-line react-hooks/set-state-in-effect
  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    if (editingHtml) return undefined;
    const timer = window.setInterval(() => { void refresh(); }, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [editingHtml, refresh]);

  const select = useCallback(async (page: LivePage) => {
    if (selectionMode) {
      setSelectedIds(current => {
        const next = new Set(current);
        if (next.has(page.id)) next.delete(page.id);
        else next.add(page.id);
        return next;
      });
      return;
    }
    setSelectedId(page.id);
    setLoading(true);
    try {
      await loadDetail(page.id);
      setEditingHtml(false);
      setEditingTitle(false);
      setTitleError(null);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, [loadDetail, selectionMode]);

  const runLinkedWorkflow = useCallback(async (workflow: LivePageWorkflowLink) => {
    if (!workflow.enabled || runningWorkflowId) return;
    setRunningWorkflowId(workflow.id);
    setWorkflowRunFeedback(null);
    await workflowsApi.triggerStream(
      workflow.id,
      () => undefined,
      () => undefined,
      result => {
        setRunningWorkflowId(null);
        setWorkflowRunFeedback({
          workflowId: workflow.id,
          kind: result.status === 'Completed' ? 'success' : 'error',
          message: result.status === 'Completed'
            ? t('pages.workflowRunSuccess')
            : t('pages.workflowRunFailed', result.status),
        });
        if (selectedId) void loadDetail(selectedId);
      },
      message => {
        setRunningWorkflowId(null);
        setWorkflowRunFeedback({ workflowId: workflow.id, kind: 'error', message });
      },
    );
  }, [loadDetail, runningWorkflowId, selectedId, t]);

  const persistHtml = useAsyncGuard(async (pageId: string, html: string) => {
    setSavingHtml(true);
    try {
      await pagesApi.updateHtml(pageId, { html, created_by_agent: null });
      await loadDetail(pageId);
      setEditingHtml(false);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSavingHtml(false);
    }
  });

  const saveHtml = useCallback(() => {
    if (!detail || !htmlDraft.trim()) return;
    void persistHtml(detail.id, htmlDraft);
  }, [detail, htmlDraft, persistHtml]);

  const exportPage = useCallback(async (format: 'pdf' | 'docx') => {
    if (!detail) return;
    setExportBusy(format);
    setExportResult(null);
    try {
      const frame = iframeRef.current;
      if (!frame) throw new Error(t('pages.exportPreviewUnavailable'));
      let rendered: Awaited<ReturnType<typeof requestRenderedPageHtml>>;
      try {
        rendered = await requestRenderedPageHtml(frame, bridgeChannel);
      } catch {
        throw new Error(t('pages.exportPreviewUnavailable'));
      }
      const filename = `${detail.slug}.${format}`;
      const result = format === 'pdf'
        ? await docsApi.generatePdf({ discussion_id: detail.id, html: rendered.html, page_images: rendered.pageImages, filename, page_size: 'A4' })
        : await docsApi.generateDocx({ discussion_id: detail.id, html: rendered.html, page_images: rendered.pageImages, filename });
      setExportResult({ url: result.download_url, filename: result.path.split('/').pop() ?? filename });
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setExportBusy(null);
    }
  }, [bridgeChannel, detail, t]);

  const selectedDataset = detail?.datasets.find(dataset => dataset.id === selectedDatasetId) ?? null;
  const selectedDatasetRecords = useMemo(
    () => selectedDataset ? datasetRecords(selectedDataset) : [],
    [selectedDataset],
  );

  const exportDatasetCsv = useCallback(async () => {
    if (!detail || !selectedDataset) return;
    const rows = recordsToRows(selectedDatasetRecords);
    if (rows.length === 0) return;
    setDatasetExportBusy(true);
    try {
      const filename = `${detail.slug}-${selectedDataset.name}.csv`;
      const delimiter = locale === 'fr' || locale === 'es' ? ';' : ',';
      const result = await docsApi.generateCsv({
        discussion_id: detail.id,
        rows,
        delimiter,
        filename,
      });
      const anchor = window.document.createElement('a');
      anchor.href = result.download_url;
      anchor.download = result.path.split('/').pop() ?? filename;
      anchor.click();
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setDatasetExportBusy(false);
    }
  }, [detail, locale, selectedDataset, selectedDatasetRecords]);

  const exportDatasetJson = useCallback(() => {
    if (!detail || !selectedDataset) return;
    const content = selectedDataset.kind === 'time_series' ? selectedDataset.points : selectedDataset.current;
    const blob = new Blob([JSON.stringify(content, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const anchor = window.document.createElement('a');
    anchor.href = url;
    anchor.download = `${detail.slug}-${selectedDataset.name}.json`;
    anchor.click();
    URL.revokeObjectURL(url);
  }, [detail, selectedDataset]);

  const updatePage = useAsyncGuard(async (page: LivePage, patch: { pinned?: boolean; archived?: boolean }) => {
    try {
      const updated = await pagesApi.update(page.id, patch);
      setPages(current => current.map(item => item.id === page.id ? updated : item));
      setDetail(current => current?.id === page.id ? updated : current);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  });

  const persistTitle = useAsyncGuard(async (pageId: string, previousTitle: string, title: string) => {
    setSavingTitle(true);
    setTitleError(null);
    try {
      const updated = await pagesApi.update(pageId, { title });
      setPages(current => current.map(page => page.id === pageId ? updated : page));
      setDetail(current => current?.id === pageId ? updated : current);
      setTitleDraft(updated.title);
      setEditingTitle(false);
      setError(null);
    } catch (cause) {
      // Keep the persisted title as the visible source of truth. The failed
      // draft must never leak into the sidebar or masquerade as a saved name.
      setTitleDraft(previousTitle);
      setTitleError(userError(cause));
    } finally {
      setSavingTitle(false);
    }
  });

  const saveTitle = useCallback(() => {
    if (!detail) return;
    const title = titleDraft.trim();
    if (title.length === 0) {
      setTitleError(t('pages.titleRequired'));
      return;
    }
    if ([...title].length > 200) {
      setTitleError(t('pages.titleTooLong'));
      return;
    }
    void persistTitle(detail.id, detail.title, title);
  }, [detail, persistTitle, t, titleDraft]);

  const startTitleEdit = useCallback(() => {
    if (!detail) return;
    setTitleDraft(detail.title);
    setTitleError(null);
    setEditingTitle(true);
  }, [detail]);

  const cancelTitleEdit = useCallback(() => {
    setTitleDraft(detail?.title ?? '');
    setTitleError(null);
    setEditingTitle(false);
  }, [detail]);

  const leaveSelectionMode = useCallback(() => {
    setSelectionMode(false);
    setSelectedIds(new Set());
  }, []);

  const runBulkAction = useAsyncGuard(async (action: 'archive' | 'restore' | 'delete', ids: string[]) => {
    if (ids.length === 0) return;
    const confirmationKey = action === 'delete'
      ? 'pages.bulk.confirmDelete'
      : action === 'archive' ? 'pages.bulk.confirmArchive' : null;
    if (confirmationKey && !window.confirm(t(confirmationKey, ids.length))) return;
    setBulkBusy(true);
    try {
      if (action === 'delete') {
        await Promise.all(ids.map(id => pagesApi.delete(id)));
      } else {
        await Promise.all(ids.map(id => pagesApi.update(id, { archived: action === 'archive' })));
      }
      leaveSelectionMode();
      await refresh(null);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : t('pages.bulk.error'));
    } finally {
      setBulkBusy(false);
    }
  });

  const isSectionCollapsed = useCallback(
    (section: string) => !query.trim() && collapsedSections.has(section),
    [collapsedSections, query],
  );
  const toggleSection = useCallback((section: string) => {
    setCollapsedSections(current => {
      const next = new Set(current);
      if (next.has(section)) next.delete(section);
      else next.add(section);
      return next;
    });
  }, []);

  const revisionHtml = detail?.revision.html ?? '';
  const latestPublication = recentPublications[0] ?? null;
  const totalDatasetSize = detail?.datasets.reduce(
    (total, dataset) => total + dataset.data_size_bytes,
    0,
  ) ?? 0;
  const comparisonRevision = revisions.find(revision => revision.id === comparisonRevisionId) ?? null;
  const document = useMemo(
    () => revisionHtml ? buildSandboxDocument(revisionHtml, bridgeChannel) : '',
    [bridgeChannel, revisionHtml],
  );
  const publishToFrame = useCallback(() => {
    if (!detail) return;
    const target = iframeRef.current?.contentWindow ?? null;
    if (!target) return;
    linkRelayRef.current?.connect(target);
    target.postMessage({
      type: 'kronn:page-data',
      version: 1,
      channel_id: bridgeChannel,
      data: runtimeData(detail),
    }, '*');
  }, [bridgeChannel, detail]);
  useEffect(() => {
    const relay = createLivePageOpenLinkRelay(bridgeChannel);
    linkRelayRef.current = relay;
    return () => {
      if (linkRelayRef.current === relay) linkRelayRef.current = null;
      relay.dispose();
    };
  }, [bridgeChannel]);
  useEffect(() => { publishToFrame(); }, [publishToFrame]);

  return (
    <div className="live-pages" data-testid="live-pages-page">
      <CollectionShell<LivePage>
        sidebarOnly
        sidebarClassName="disc-sidebar live-pages-list"
        ariaLabel={t('pages.title')}
        items={pages}
        getId={page => page.id}
        getLabel={page => `${page.title} ${page.slug}`}
        persistence={{ query, onQueryChange: setQuery, favoritesOnly: false, onFavoritesOnlyChange: () => {} }}
        selectedId={selectedId}
        onSelect={id => { const page = pages.find(item => item.id === id); if (page) void select(page); }}
        selectedIds={selectedIds}
        onSelectedIdsChange={setSelectedIds}
        globalSearchShortcut
        showSearchClear
        sidebarOpen={sidebarOpen}
        onSidebarOpenChange={setSidebarOpen}
        labels={{ search: t('pages.search'), favorites: t('pages.filter.favorites'), clearFilters: t('pages.clearSearch'), moreActions: t('pages.title'), openCollection: t('collection.openCollection'), closeCollection: t('collection.closeCollection'), selectItem: t('pages.bulk.selected', 1) }}
        slots={{
          beforeSidebarHeader: <div className="disc-sidebar-header" data-selection-mode={selectionMode}>
          <span className="disc-sidebar-header-title">
            {selectionMode ? t('pages.bulk.selected', selectedIds.size) : <>{t('pages.title')}<span className="disc-sidebar-header-count">{' · '}{pages.length}</span></>}
          </span>
          <div className="disc-sidebar-header-actions">
            {selectionMode ? (
              <>
                <button type="button" className="disc-icon-btn" onClick={() => void runBulkAction('archive', [...selectedIds])} disabled={bulkBusy || selectedIds.size === 0} title={t('pages.archive')} aria-label={t('pages.archive')}>
                  {bulkBusy ? <Loader2 size={14} className="spin" /> : <Archive size={14} />}
                </button>
                <button type="button" className="disc-icon-btn" onClick={() => void runBulkAction('restore', [...selectedIds])} disabled={bulkBusy || selectedIds.size === 0} title={t('pages.restore')} aria-label={t('pages.restore')}><RotateCcw size={14} /></button>
                <button type="button" className="disc-icon-btn disc-bulk-delete-btn" onClick={() => void runBulkAction('delete', [...selectedIds])} disabled={bulkBusy || selectedIds.size === 0} title={t('pages.delete')} aria-label={t('pages.delete')}><Trash2 size={14} /></button>
                <button type="button" className="disc-icon-btn" onClick={leaveSelectionMode} disabled={bulkBusy} title={t('pages.bulk.cancel')} aria-label={t('pages.bulk.cancel')}><X size={14} /></button>
              </>
            ) : (
              <>
                <button type="button" className="disc-icon-btn" onClick={() => setSelectionMode(true)} title={t('pages.bulk.start')} aria-label={t('pages.bulk.start')}><ListChecks size={16} /></button>
                <button type="button" className="disc-icon-btn collection-shell-primary-action" onClick={() => void refresh()} title={t('pages.refresh')} aria-label={t('pages.refresh')}><RefreshCw size={15} className={loading ? 'spin' : undefined} /></button>
                <CollectionSidebarCollapseButton label={t('collection.closeCollection')} onCollapse={() => setSidebarOpen(false)} />
              </>
            )}
          </div>
          </div>,
          renderList: ({ visibleItems, getRowProps }) => {
            const visibleActive = visibleItems.filter(page => !page.archived);
            const visibleFavorites = selectionMode ? [] : visibleActive.filter(page => page.pinned);
            const visibleArchived = visibleItems.filter(page => page.archived);
            const row = (page: LivePage, keyPrefix: string) => {
              const rowProps = getRowProps(page);
              return <div className="disc-swipe-wrap live-page-row" key={`${keyPrefix}-${page.id}`}>
                <div className="disc-item" data-active={page.id === selectedId} data-selected={selectedIds.has(page.id)}>
                  <button type="button" {...rowProps} className={`${rowProps.className} disc-item-open`} aria-label={t('pages.open', page.title)} role={selectionMode ? 'checkbox' : undefined} aria-checked={selectionMode ? selectedIds.has(page.id) : undefined}>
                    {selectionMode && <span className="disc-item-selection-box" data-selected={selectedIds.has(page.id)} aria-hidden="true">{selectedIds.has(page.id) && <CheckSquare2 size={12} />}</span>}
                    <span className="disc-item-content"><span className="disc-item-title"><span className="disc-item-title-text">{page.title}</span></span><span className="disc-item-meta"><span className="disc-item-meta-summary">{page.slug}</span></span></span>
                  </button>
                  {!selectionMode && <div className="disc-item-actions"><FavoriteToggle active={page.pinned} onToggle={() => void updatePage(page, { pinned: !page.pinned })} activeLabel={t('pages.unfavorite')} inactiveLabel={t('pages.favorite')} itemName={page.title} /></div>}
                </div>
              </div>;
            };
            return <div className="disc-sidebar-list live-pages-items">
          {visibleFavorites.length > 0 && (
            <div className="disc-sidebar-section disc-sidebar-favorites" data-expanded={!isSectionCollapsed('favorites')}>
              <button type="button" className="disc-group-btn" data-no-border="true" onClick={() => toggleSection('favorites')} aria-expanded={!isSectionCollapsed('favorites')}>
                <ChevronRight size={10} className="disc-chevron" data-expanded={!isSectionCollapsed('favorites')} />
                <Star size={10} className="live-page-group-star" fill="currentColor" />
                <span>{t('pages.filter.favorites')}</span><span className="disc-group-count">{visibleFavorites.length}</span>
              </button>
              {!isSectionCollapsed('favorites') && visibleFavorites.map(page => row(page, 'favorite'))}
            </div>
          )}

          {visibleActive.length > 0 && (
            <div className="disc-sidebar-section disc-sidebar-projects" data-expanded={!isSectionCollapsed('pages')}>
              <button type="button" className="disc-group-btn" data-no-border="true" onClick={() => toggleSection('pages')} aria-expanded={!isSectionCollapsed('pages')}>
                <ChevronRight size={10} className="disc-chevron" data-expanded={!isSectionCollapsed('pages')} />
                <FileCode2 size={10} />
                <span>{t('pages.filter.active')}</span><span className="disc-group-count">{visibleActive.length}</span>
              </button>
              {!isSectionCollapsed('pages') && visibleActive.map(page => row(page, 'page'))}
            </div>
          )}

          {visibleArchived.length > 0 && (
            <div className="disc-sidebar-section disc-sidebar-archives" data-expanded={!isSectionCollapsed('archives')}>
              <button type="button" className="disc-group-btn" data-variant="archive" onClick={() => toggleSection('archives')} aria-expanded={!isSectionCollapsed('archives')}>
                <ChevronRight size={10} className="disc-chevron" data-expanded={!isSectionCollapsed('archives')} />
                <Archive size={10} /><span>{t('pages.filter.archived')}</span><span className="disc-group-count">{visibleArchived.length}</span>
              </button>
              {!isSectionCollapsed('archives') && visibleArchived.map(page => row(page, 'archive'))}
            </div>
          )}

          {visibleItems.length === 0 && <div className="disc-empty">{t(query ? 'pages.noSearchResults' : 'pages.empty')}</div>}
            </div>;
          },
          sidebarFooter: <div className="disc-sidebar-footer"><span>{t('pages.sidebar.hint')}</span><span><kbd>/</kbd> {t('pages.sidebar.search')}</span></div>,
          renderDetail: () => null,
        }}
      />

      <section className="live-pages-viewer">
        {error && <div className="live-pages-error" role="alert">{error}</div>}
        {detail ? (
          <>
            <header className="live-pages-viewer-header">
              <div className="live-pages-title-block">
                {editingTitle ? (
                  <form
                    className="live-pages-title-editor"
                    onSubmit={(event) => { event.preventDefault(); saveTitle(); }}
                  >
                    <input
                      value={titleDraft}
                      onChange={(event) => { setTitleDraft(event.target.value); setTitleError(null); }}
                      onKeyDown={(event) => { if (event.key === 'Escape') cancelTitleEdit(); }}
                      aria-label={t('pages.titleInput')}
                      maxLength={201}
                      autoFocus
                      disabled={savingTitle}
                    />
                    <button type="submit" disabled={savingTitle} aria-label={t('pages.saveTitle')} title={t('pages.saveTitle')}>
                      {savingTitle ? <Loader2 size={14} className="spin" /> : <Save size={14} />}
                    </button>
                    <button type="button" onClick={cancelTitleEdit} disabled={savingTitle} aria-label={t('pages.cancelTitle')} title={t('pages.cancelTitle')}>
                      <X size={14} />
                    </button>
                    {titleError && <small role="alert">{titleError}</small>}
                  </form>
                ) : (
                  <div className="live-pages-title-display">
                    <h1>{detail.title}</h1>
                    <button type="button" onClick={startTitleEdit} aria-label={t('pages.renameTitle', detail.title)} title={t('pages.renameTitle', detail.title)}>
                      <Pencil size={13} />
                    </button>
                  </div>
                )}
                <div className="live-pages-identity">
                  <button
                    type="button"
                    className="live-pages-dataset-trigger"
                    onClick={() => setSelectedDatasetId(selectedDataset?.id ?? detail.datasets[0]?.id ?? null)}
                    disabled={detail.datasets.length === 0}
                    title={t('pages.datasetStorageTitle', formatBytes(totalDatasetSize))}
                    aria-label={t('pages.datasetStorage')}
                  >
                    <span>{t('pages.datasetCount', detail.datasets.length)}</span>
                    <span className="live-pages-data-size"><Database size={11} /> {formatBytes(totalDatasetSize)}</span>
                    <ChevronDown size={11} />
                  </button>
                  {detail.archived && <span className="live-pages-archived-badge"><Archive size={11} />{t('pages.archived')}</span>}
                  <CopyIdPill id={detail.id} title={t('pages.copyId', detail.title)} />
                </div>
              </div>
              <div className="live-pages-header-actions">
                <a
                  className="live-pages-open-tab"
                  href={standaloneLivePageUrl(detail.id)}
                  target="_blank"
                  rel="noopener noreferrer"
                  aria-label={t('pages.openInNewTab', detail.title)}
                  title={t('pages.openInNewTab', detail.title)}
                >
                  <ExternalLink size={13} />
                  <span>{t('pages.openInNewTabLabel')}</span>
                </a>
                <button type="button" className="live-pages-header-icon" onClick={() => void updatePage(detail, { pinned: !detail.pinned })} title={detail.pinned ? t('pages.unfavorite') : t('pages.favorite')}>
                  <Star size={14} fill={detail.pinned ? 'currentColor' : 'none'} />
                </button>
                <div className="live-pages-revision">
                  <Activity size={13} /> data r{detail.data_revision} · HTML r{detail.revision.revision}
                </div>
                <details className="live-pages-export-menu" data-testid="live-page-export-menu" ref={exportMenuRef}>
                  <summary><Download size={13} />{t('pages.export')}<ChevronDown size={12} /></summary>
                  <div className="live-pages-export-popover">
                    <button type="button" onClick={() => void exportPage('pdf')} disabled={exportBusy !== null}>
                      {exportBusy === 'pdf' ? <Loader2 size={13} className="spin" /> : <FileDown size={13} />} PDF
                    </button>
                    <button type="button" onClick={() => void exportPage('docx')} disabled={exportBusy !== null}>
                      {exportBusy === 'docx' ? <Loader2 size={13} className="spin" /> : <FileDown size={13} />} DOCX
                    </button>
                    {exportResult && <a href={exportResult.url} download={exportResult.filename} target="_blank" rel="noopener noreferrer"><ExternalLink size={12} />{exportResult.filename}</a>}
                  </div>
                </details>
                <details className="live-pages-refresh-menu" data-testid="live-page-refresh-menu" ref={refreshMenuRef}>
                  <summary
                    aria-label={t('pages.autoRefresh')}
                    data-state={latestPublication ? 'synced' : 'waiting'}
                  >
                    <CheckCircle2 size={14} />
                    <span>{latestPublication ? t('pages.refreshSynced') : t('pages.refreshWaiting')}</span>
                    {latestPublication && (
                      <time title={new Date(latestPublication.published_at).toLocaleString(locale)}>
                        {formatRelativeTime(latestPublication.published_at, locale)}
                      </time>
                    )}
                    <ChevronDown size={13} className="live-pages-refresh-chevron" />
                  </summary>
                  <section className="live-pages-refresh-popover" data-testid="live-page-recent-refreshes">
                    <header className="live-pages-refresh-popover-header">
                      <span className="live-pages-refresh-title-icon"><History size={15} /></span>
                      <div>
                        <strong>{t('pages.autoRefresh')}</strong>
                        <span>{t('pages.recentRefreshesHint')}</span>
                      </div>
                    </header>

                    <div className="live-pages-refresh-workflows">
                      <div><Workflow size={12} /><span>{t('pages.linkedWorkflows')}</span></div>
                      {linkedWorkflows.length > 0 ? linkedWorkflows.map(workflow => (
                        <span key={workflow.id} className="live-pages-linked-workflow">
                          <button type="button" className="live-pages-linked-workflow-open" onClick={() => onNavigateWorkflow?.(workflow.id)} disabled={!onNavigateWorkflow}>
                            {workflow.name}{workflow.enabled ? '' : ` ${t('pages.disabledSuffix')}`}
                          </button>
                          <button
                            type="button"
                            className="live-pages-linked-workflow-run"
                            onClick={() => void runLinkedWorkflow(workflow)}
                            disabled={!workflow.enabled || runningWorkflowId !== null}
                            data-state={runningWorkflowId === workflow.id
                              ? 'running'
                              : workflowRunFeedback?.workflowId === workflow.id
                                ? workflowRunFeedback.kind
                                : 'idle'}
                            title={workflow.enabled ? t('pages.runLinkedWorkflow', workflow.name) : t('pages.runDisabledWorkflow')}
                            aria-label={workflow.enabled ? t('pages.runLinkedWorkflow', workflow.name) : t('pages.runDisabledWorkflow')}
                          >
                            {runningWorkflowId === workflow.id
                              ? <Loader2 size={12} className="spin" />
                              : workflowRunFeedback?.workflowId === workflow.id
                                ? workflowRunFeedback.kind === 'success'
                                  ? <CheckCircle2 size={12} data-testid="live-page-sync-success-icon" />
                                  : <X size={12} data-testid="live-page-sync-error-icon" />
                                : <Play size={12} />}
                          </button>
                          {workflowRunFeedback?.workflowId === workflow.id && (
                            <small
                              data-kind={workflowRunFeedback.kind}
                              role={workflowRunFeedback.kind === 'error' ? 'alert' : 'status'}
                            >
                              {workflowRunFeedback.kind === 'success'
                                ? <CheckCircle2 size={11} aria-hidden="true" />
                                : <X size={11} aria-hidden="true" />}
                              {workflowRunFeedback.message}
                            </small>
                          )}
                        </span>
                      )) : <small>{t('pages.noLinkedWorkflows')}</small>}
                    </div>

                    <div className="live-pages-dataset-sizes">
                      <div><Database size={12} /><span>{t('pages.datasetStorage')}</span></div>
                      <div>
                        {detail.datasets.map(dataset => (
                          <button type="button" key={dataset.id} onClick={() => setSelectedDatasetId(dataset.id)} title={t('pages.datasetSizeTitle', dataset.name, formatBytes(dataset.data_size_bytes))}>
                            <strong>{dataset.name}</strong>
                            {formatBytes(dataset.data_size_bytes)}
                          </button>
                        ))}
                      </div>
                    </div>

                    <div className="live-pages-refresh-timeline">
                      {recentPublications.length > 0 ? recentPublications.map(publication => {
                        const workflowName = publication.workflow_name ?? t('pages.directPublication');
                        const workflowId = publication.workflow_id;
                        // Rolling upgrades may briefly serve the pre-delta Page
                        // response. Treat its touched datasets as changed instead
                        // of crashing or falsely claiming that they were equal.
                        const contentChanged = publication.content_changed ?? true;
                        const changedDatasets = publication.changed_datasets ?? publication.datasets_updated;
                        const unchangedDatasets = publication.unchanged_datasets ?? [];
                        const content = (
                          <>
                            <span className="live-pages-refresh-marker" data-changed={contentChanged} aria-hidden="true" />
                            <span className="live-pages-refresh-event">
                              <span className="live-pages-refresh-event-head">
                                <strong>
                                  {contentChanged
                                    ? t('pages.refreshChanged')
                                    : t('pages.refreshUnchanged')}
                                </strong>
                                <time title={new Date(publication.published_at).toLocaleString(locale)}>
                                  {formatRelativeTime(publication.published_at, locale)}
                                </time>
                              </span>
                              <span className="live-pages-refresh-event-meta">
                                <span>{workflowName}</span>
                                <span>data r{publication.data_revision}</span>
                                {publication.workflow_run_id && <span>{t('pages.refreshRun')}</span>}
                              </span>
                              <span className="live-pages-refresh-deltas">
                                {changedDatasets.map(dataset => (
                                  <span key={`changed-${dataset}`} data-kind="changed">{dataset}</span>
                                ))}
                                {unchangedDatasets.map(dataset => (
                                  <span key={`unchanged-${dataset}`} data-kind="unchanged">{t('pages.unchangedDataset', dataset)}</span>
                                ))}
                                {publication.points_added > 0 && (
                                  <span data-kind="points">{t('pages.pointsAdded', publication.points_added)}</span>
                                )}
                                {publication.points_removed > 0 && (
                                  <span data-kind="retention">{t('pages.pointsRemoved', publication.points_removed)}</span>
                                )}
                              </span>
                            </span>
                            {workflowId && onNavigateWorkflow && <ChevronRight size={14} className="live-pages-refresh-open" />}
                          </>
                        );
                        return workflowId && onNavigateWorkflow ? (
                          <button
                            key={publication.id}
                            type="button"
                            className="live-pages-refresh-row"
                            onClick={() => onNavigateWorkflow(
                              workflowId,
                              publication.workflow_run_id ?? undefined,
                            )}
                            aria-label={t('pages.openRefreshRun', workflowName)}
                          >
                            {content}
                          </button>
                        ) : (
                          <div key={publication.id} className="live-pages-refresh-row">{content}</div>
                        );
                      }) : <small>{t('pages.noRecentRefreshes')}</small>}
                    </div>
                  </section>
                </details>
                <button
                  type="button"
                  className="live-pages-editor-toggle"
                  data-active={editingHtml}
                  onClick={() => {
                    setEditingHtml(value => !value);
                    setComparisonRevisionId(null);
                  }}
                >
                  {editingHtml ? <X size={13} /> : <Pencil size={13} />}
                  {editingHtml ? t('pages.closeEditor') : t('pages.editHtml')}
                </button>
              </div>
            </header>
            <div className="live-pages-relations">
              {linkedDiscussions.length > 0 && (
                <div className="live-pages-workflows">
                  <div><MessageSquare size={13} /><strong>{t('pages.linkedDiscussions')}</strong></div>
                  {linkedDiscussions.map(discussion => (
                    <button key={discussion.discussion_id} type="button" onClick={() => onNavigateDiscussion?.(discussion.discussion_id)} disabled={!onNavigateDiscussion}>
                      {discussion.title}{discussion.relation === 'created_from' ? ` · ${t('pages.createdFrom')}` : ''}
                    </button>
                  ))}
                </div>
              )}
            </div>
            {selectedDataset && (
              <section className="live-pages-dataset-viewer" aria-label={t('pages.datasetView', selectedDataset.name)}>
                <header>
                  <div className="live-pages-dataset-selector">
                    <Database size={14} />
                    <select
                      value={selectedDataset.id}
                      onChange={event => setSelectedDatasetId(event.target.value)}
                      aria-label={t('pages.datasetView', selectedDataset.name)}
                    >
                      {detail.datasets.map(dataset => (
                        <option key={dataset.id} value={dataset.id}>{dataset.name} · {formatBytes(dataset.data_size_bytes)}</option>
                      ))}
                    </select>
                    <span>{selectedDataset.kind}</span>
                  </div>
                  <div>
                    <button type="button" onClick={() => void exportDatasetCsv()} disabled={datasetExportBusy || selectedDatasetRecords.length === 0}><Table2 size={13} />CSV</button>
                    <button type="button" onClick={exportDatasetJson}><Braces size={13} />JSON</button>
                    <button type="button" onClick={() => setSelectedDatasetId(null)} aria-label={t('pages.closeDataset')}><X size={13} /></button>
                  </div>
                </header>
                {selectedDatasetRecords.length > 0 ? (
                  <div className="live-pages-dataset-table-wrap">
                    <table>
                      <thead><tr>{recordsToRows(selectedDatasetRecords)[0]?.map(header => <th key={String(header)}>{header}</th>)}</tr></thead>
                      <tbody>{recordsToRows(selectedDatasetRecords).slice(1, 101).map((row, rowIndex) => <tr key={rowIndex}>{row.map((cell, cellIndex) => <td key={cellIndex}>{String(cell ?? '')}</td>)}</tr>)}</tbody>
                    </table>
                    {selectedDatasetRecords.length > 100 && <small>{t('pages.datasetPreviewLimit', selectedDatasetRecords.length)}</small>}
                  </div>
                ) : <pre>{JSON.stringify(selectedDataset.current, null, 2)}</pre>}
              </section>
            )}
            {editingHtml ? (
              <div className="live-pages-editor">
                <div className="live-pages-editor-head">
                  <div><strong>{t('pages.htmlTitle')}</strong><span>{t('pages.htmlRevisionHint')}</span></div>
                  <div className="live-pages-editor-actions">
                    <label className="live-pages-revision-picker">
                      <History size={13} />
                      <select
                        value={comparisonRevisionId ?? ''}
                        onChange={event => setComparisonRevisionId(event.target.value || null)}
                        aria-label={t('pages.compareRevision')}
                      >
                        <option value="">{t('pages.revisionHistory', revisions.length)}</option>
                        {revisions.filter(revision => revision.id !== detail.revision.id).map(revision => (
                          <option key={revision.id} value={revision.id}>
                            r{revision.revision} · {new Date(revision.created_at).toLocaleString(locale)}
                          </option>
                        ))}
                      </select>
                    </label>
                    {comparisonRevision && (
                      <button
                        type="button"
                        className="live-pages-restore-revision"
                        onClick={() => {
                          setHtmlDraft(comparisonRevision.html);
                          setComparisonRevisionId(null);
                        }}
                      >
                        <RotateCcw size={13} /> {t('pages.restoreRevision', comparisonRevision.revision)}
                      </button>
                    )}
                    <button type="button" className="live-pages-create-revision" onClick={saveHtml} disabled={savingHtml || !htmlDraft.trim()}>
                      <Save size={13} /> {savingHtml ? t('pages.saving') : t('pages.saveRevision')}
                    </button>
                  </div>
                </div>
                {comparisonRevision ? (
                  <div className="live-pages-editor-comparison">
                    <div><GitCompare size={14} />{t('pages.revisionComparison')}</div>
                    <HtmlRevisionDiff
                      previous={comparisonRevision.html}
                      current={htmlDraft}
                      previousLabel={`r${comparisonRevision.revision}`}
                      currentLabel={t('pages.currentDraft', detail.revision.revision)}
                    />
                  </div>
                ) : (
                  <HtmlCodeEditor
                    value={htmlDraft}
                    onChange={setHtmlDraft}
                    ariaLabel={t('pages.htmlTitle')}
                  />
                )}
              </div>
            ) : (
              <iframe
                ref={iframeRef}
                title={detail.title}
                sandbox="allow-scripts"
                srcDoc={document}
                onLoad={publishToFrame}
                data-testid="live-page-frame"
              />
            )}
          </>
        ) : !loading && (
          <div className="live-pages-empty">{t('pages.empty')}</div>
        )}
      </section>
    </div>
  );
}
