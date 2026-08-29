import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { LivePageDetail } from '../types/generated';
import { pages as pagesApi } from '../lib/api';
import {
  buildSandboxDocument,
  createLivePageOpenLinkRelay,
  runtimeData,
} from '../lib/live-page-sandbox';
import type { LivePageMosaicLayout } from '../lib/live-page-navigation';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import './StandaloneLivePageMosaic.css';

function channelId(pageId: string): string {
  return globalThis.crypto?.randomUUID?.() ?? `mosaic-page-${pageId}-${Date.now()}-${Math.random()}`;
}

function MosaicLivePageFrame({ pageId }: { pageId: string }) {
  const { t } = useT();
  const [detail, setDetail] = useState<LivePageDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [bridgeChannel] = useState(() => channelId(pageId));
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const linkRelayRef = useRef<ReturnType<typeof createLivePageOpenLinkRelay> | null>(null);

  useEffect(() => {
    let active = true;
    pagesApi.get(pageId).then(page => {
      if (!active) return;
      setDetail(page);
      setError(null);
    }).catch(cause => {
      if (!active) return;
      setError(userError(cause));
    });
    return () => { active = false; };
  }, [pageId]);

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
    const relay = createLivePageOpenLinkRelay(bridgeChannel);
    linkRelayRef.current = relay;
    return () => {
      if (linkRelayRef.current === relay) linkRelayRef.current = null;
      relay.dispose();
    };
  }, [bridgeChannel]);
  useEffect(() => { publishToFrame(); }, [publishToFrame]);

  if (error) {
    return <section className="standalone-live-page-mosaic-state" role="alert">{t('pages.standaloneLoadError', error)}</section>;
  }
  if (!detail) {
    return <section className="standalone-live-page-mosaic-state" role="status">{t('pages.standaloneLoading')}</section>;
  }

  return (
    <iframe
      ref={iframeRef}
      title={detail.title}
      sandbox="allow-scripts"
      srcDoc={sandboxDocument}
      onLoad={publishToFrame}
      data-testid="standalone-live-page-mosaic-frame"
    />
  );
}

export function StandaloneLivePageMosaic({
  pageIds,
  layout,
}: {
  pageIds: string[];
  layout: LivePageMosaicLayout;
}) {
  const { t } = useT();

  useEffect(() => {
    const previousTitle = document.title;
    document.title = `${t('pages.mosaic.documentTitle')} · Kronn`;
    return () => { document.title = previousTitle; };
  }, [t]);

  return (
    <main
      className="standalone-live-page-mosaic"
      data-layout={layout}
      data-count={pageIds.length}
      data-testid="standalone-live-page-mosaic"
    >
      {pageIds.map(pageId => (
        <section className="standalone-live-page-mosaic-tile" key={pageId}>
          <MosaicLivePageFrame pageId={pageId} />
        </section>
      ))}
    </main>
  );
}
