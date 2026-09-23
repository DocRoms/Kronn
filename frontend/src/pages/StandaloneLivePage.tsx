import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { LivePageDetail } from '../types/generated';
import { pages as pagesApi } from '../lib/api';
import {
  buildSandboxDocument,
  hostTheme,
  createLivePageOpenLinkRelay,
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
import { LivePageActionOverlay } from '../components/LivePageActionOverlay';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import './StandaloneLivePage.css';

function channelId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `standalone-page-${Date.now()}-${Math.random()}`;
}

export function StandaloneLivePage({ pageId }: { pageId: string }) {
  const { t } = useT();
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

  useEffect(() => {
    if (!detail) return undefined;
    const previousTitle = document.title;
    document.title = `${detail.title} · Kronn`;
    return () => { document.title = previousTitle; };
  }, [detail]);

  const sandboxDocument = useMemo(
    () => detail ? buildSandboxDocument(detail.revision.html, bridgeChannel, hostTheme()) : '',
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
      data: runtimeData(detail),
    }, '*');
  }, [bridgeChannel, detail]);
  useEffect(() => {
    const relay = createLivePageOpenLinkRelay(bridgeChannel, undefined, intent => {
      setActionUnavailable(false);
      handlePageActionIntent(intent);
    }, movePageActionAnchor);
    linkRelayRef.current = relay;
    return () => {
      if (linkRelayRef.current === relay) linkRelayRef.current = null;
      relay.dispose();
    };
  }, [bridgeChannel, handlePageActionIntent, movePageActionAnchor]);
  usePublishPageDataWhenChanged(detail, publishToFrame);
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
      <div className="standalone-live-page-frame-shell">
        {actionUnavailable && (
          <p className="standalone-live-page-action-error" role="alert">
            {t('disc.action.unavailablePageAction')}
          </p>
        )}
        <iframe
          ref={iframeRef}
          title={detail.title}
          sandbox="allow-scripts"
          srcDoc={sandboxDocument}
          onLoad={publishAllToFrame}
          data-testid="standalone-live-page-frame"
        />
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
