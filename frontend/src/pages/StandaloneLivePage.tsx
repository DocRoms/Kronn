import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { LivePageDetail } from '../types/generated';
import { pages as pagesApi } from '../lib/api';
import {
  buildSandboxDocument,
  hostTheme,
  hostThemeTokens,
  createLivePageOpenLinkRelay,
  type LivePageEmbedPlacement,
  runtimeData,
} from '../lib/live-page-sandbox';
import { openStandaloneDiscussion } from '../lib/live-page-navigation';
import {
  useActionStatesInFrame,
  useLivePageActions,
  useLivePageActionSlot,
  useLivePageTheme,
  usePublishPageDataWhenChanged,
} from '../hooks/useLivePageActions';
import { useLivePageDataPush } from '../hooks/useLivePageDataPush';
import { LivePageActionOverlay } from '../components/LivePageActionOverlay';
import { LivePageEmbedOverlay } from '../components/LivePageEmbedOverlay';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import './StandaloneLivePage.css';

// Same cadence as the embedded Pages view.
const REFRESH_MS = 30_000;

const NO_EMBEDS: LivePageEmbedPlacement[] = [];

function channelId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `standalone-page-${Date.now()}-${Math.random()}`;
}

export function StandaloneLivePage({ pageId, params }: { pageId: string; params?: Record<string, string> }) {
  const { t } = useT();
  // Keyed by content: a parent re-render with an equal object must not re-post data.
  const paramsKey = JSON.stringify(params ?? {});
  const viewParams = useMemo(() => JSON.parse(paramsKey) as Record<string, string>, [paramsKey]);
  const [detail, setDetail] = useState<LivePageDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [actionUnavailable, setActionUnavailable] = useState(false);
  const [bridgeChannel] = useState(channelId);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const linkRelayRef = useRef<ReturnType<typeof createLivePageOpenLinkRelay> | null>(null);
  const {
    activeAction: pageActiveAction,
    selectedAction: pageSelectedAction,
    selectedOffer: pageSelectedOffer,
    launches: pageLaunches,
    close: closePageAction,
    handleIntent: handlePageActionIntent,
    moveAnchor: movePageActionAnchor,
    handleChanged: handlePageActionChanged,
    reload: reloadPageActions,
  } = useLivePageActions(() => setActionUnavailable(true));

  useEffect(() => {
    let active = true;
    Promise.all([pagesApi.get(pageId), reloadPageActions(pageId)]).then(([page]) => {
      if (!active) return;
      setDetail(page);
      setError(null);
    }).catch(cause => {
      if (!active) return;
      setError(userError(cause));
    });
    return () => { active = false; };
  }, [pageId, reloadPageActions]);

  // Keep reading the Page while the tab is open. New data reaches the frame by
  // postMessage; the document is only rebuilt when the Page's HTML changes, so
  // scroll and open rows survive a refresh.
  useEffect(() => {
    let active = true;
    const refresh = () => {
      if (document.visibilityState === 'hidden') return;
      Promise.all([pagesApi.get(pageId), reloadPageActions(pageId)]).then(([page]) => {
        if (active) setDetail(page);
      }).catch(() => { /* keep the last view; the next tick retries */ });
    };
    const timer = window.setInterval(refresh, REFRESH_MS);
    const onVisibility = () => { if (document.visibilityState === 'visible') refresh(); };
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      active = false;
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [pageId, reloadPageActions]);

  // A publish reaches the open tab at once, without waiting for the poll.
  useLivePageDataPush(pageId, () => {
    Promise.all([pagesApi.get(pageId), reloadPageActions(pageId)]).then(([page]) => {
      setDetail(current => (current && current.id !== page.id ? current : page));
    }).catch(() => { /* the poll retries */ });
  });

  useEffect(() => {
    if (!detail) return undefined;
    const previousTitle = document.title;
    document.title = `${detail.title} · Kronn`;
    return () => { document.title = previousTitle; };
  }, [detail]);

  const sandboxDocument = useMemo(
    () => detail ? buildSandboxDocument(detail.revision.html, bridgeChannel, hostTheme(), hostThemeTokens()) : '',
    [bridgeChannel, detail],
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
      data: runtimeData(detail, viewParams),
    }, '*');
  }, [bridgeChannel, detail, viewParams]);
  // A content-sized Page reports its height; it only applies to the document that sent it,
  // so a new revision that does not opt in never inherits the previous one's size.
  const [frameSize, setFrameSize] = useState<{ doc: string; height: number } | null>(null);
  const frameDocRef = useRef(sandboxDocument);
  useEffect(() => { frameDocRef.current = sandboxDocument; }, [sandboxDocument]);
  const frameHeight = frameSize && frameSize.doc === sandboxDocument ? frameSize.height : null;
  // Where the Page placed its third-party players; like the height, it only
  // applies to the document that reported it.
  const [embedsReport, setEmbedsReport] = useState<{ doc: string; embeds: LivePageEmbedPlacement[] } | null>(null);
  const pageEmbeds = embedsReport && embedsReport.doc === sandboxDocument ? embedsReport.embeds : NO_EMBEDS;
  // Display preferences a Page asks to keep are stored for the Page shown.
  const shownPageIdRef = useRef<string | null>(null);
  useEffect(() => { shownPageIdRef.current = detail?.id ?? null; }, [detail]);
  useEffect(() => {
    const relay = createLivePageOpenLinkRelay(bridgeChannel, {
      pageId: () => shownPageIdRef.current,
      onAction: intent => {
        setActionUnavailable(false);
        handlePageActionIntent(intent);
      },
      onAnchor: movePageActionAnchor,
      onHeight: height => setFrameSize({ doc: frameDocRef.current, height }),
      onEmbeds: embeds => setEmbedsReport({ doc: frameDocRef.current, embeds }),
    });
    linkRelayRef.current = relay;
    return () => {
      if (linkRelayRef.current === relay) linkRelayRef.current = null;
      relay.dispose();
    };
  }, [bridgeChannel, handlePageActionIntent, movePageActionAnchor]);
  // View parameters are part of what the Page was given: a new `?scene=` must
  // reach it without a reload, an identical one must not re-post.
  usePublishPageDataWhenChanged(detail, publishToFrame, paramsKey);
  // Each button shows how its row's last run went, and keeps up while it runs.
  const publishAllToFrame = useActionStatesInFrame(iframeRef, bridgeChannel, pageLaunches, publishToFrame);
  // The Page keeps a collapse of exactly this height open under the row.
  const [actionCardHeight, setActionCardHeight] = useState<number | null>(null);
  useLivePageActionSlot(iframeRef, bridgeChannel, pageActiveAction, actionCardHeight);
  // La Page suit le thème de Kronn, pas celui du système.
  useLivePageTheme(iframeRef, bridgeChannel);

  if (error) {
    return <main className="standalone-live-page-state" role="alert">{t('pages.standaloneLoadError', error)}</main>;
  }
  if (!detail) {
    return <main className="standalone-live-page-state" role="status">{t('pages.standaloneLoading')}</main>;
  }

  return (
    <main className="standalone-live-page" data-testid="standalone-live-page">
      <div className={frameHeight ? "standalone-live-page-frame-shell is-content-sized" : "standalone-live-page-frame-shell"}>
        {actionUnavailable && (
          <p className="standalone-live-page-action-error" role="alert">
            {t('disc.action.unavailablePageAction')}
          </p>
        )}
        <iframe
          ref={iframeRef}
                  style={frameHeight ? { height: frameHeight } : undefined}
          title={detail.title}
          sandbox="allow-scripts"
          srcDoc={sandboxDocument}
          onLoad={publishAllToFrame}
          data-testid="standalone-live-page-frame"
        />
        <LivePageEmbedOverlay frameRef={iframeRef} embeds={pageEmbeds} />
        <LivePageActionOverlay
          active={pageActiveAction}
          action={pageSelectedAction}
          offer={pageSelectedOffer}
          onClose={closePageAction}
          onChanged={handlePageActionChanged}
          onOpenDiscussion={openStandaloneDiscussion}
          onHeightChange={setActionCardHeight}
        />
      </div>
    </main>
  );
}
