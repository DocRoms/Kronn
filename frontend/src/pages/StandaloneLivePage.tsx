import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { LivePageDetail } from '../types/generated';
import { pages as pagesApi } from '../lib/api';
import {
  buildSandboxDocument,
  createLivePageOpenLinkRelay,
  runtimeData,
} from '../lib/live-page-sandbox';
import { openStandaloneDiscussion } from '../lib/live-page-navigation';
import { useLivePageActions } from '../hooks/useLivePageActions';
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
    handleIntent: handlePageActionIntent,
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
    () => detail ? buildSandboxDocument(detail.revision.html, bridgeChannel) : '',
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
    });
    linkRelayRef.current = relay;
    return () => {
      if (linkRelayRef.current === relay) linkRelayRef.current = null;
      relay.dispose();
    };
  }, [bridgeChannel, handlePageActionIntent]);
  useEffect(() => { publishToFrame(); }, [publishToFrame]);

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
          onLoad={publishToFrame}
          data-testid="standalone-live-page-frame"
        />
        <LivePageActionOverlay
          active={pageActiveAction}
          action={pageSelectedAction}
          onChanged={handlePageActionChanged}
          onOpenDiscussion={openStandaloneDiscussion}
        />
      </div>
    </main>
  );
}
