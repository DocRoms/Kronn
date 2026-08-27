import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { LivePageDetail } from '../types/generated';
import { pages as pagesApi } from '../lib/api';
import { buildSandboxDocument, runtimeData } from '../lib/live-page-sandbox';
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
  const [bridgeChannel] = useState(channelId);
  const iframeRef = useRef<HTMLIFrameElement>(null);

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
    iframeRef.current?.contentWindow?.postMessage({
      type: 'kronn:page-data',
      version: 1,
      channel_id: bridgeChannel,
      data: runtimeData(detail),
    }, '*');
  }, [bridgeChannel, detail]);
  useEffect(() => { publishToFrame(); }, [publishToFrame]);

  if (error) {
    return <main className="standalone-live-page-state" role="alert">{t('pages.standaloneLoadError', error)}</main>;
  }
  if (!detail) {
    return <main className="standalone-live-page-state" role="status">{t('pages.standaloneLoading')}</main>;
  }

  return (
    <main className="standalone-live-page" data-testid="standalone-live-page">
      <iframe
        ref={iframeRef}
        title={detail.title}
        sandbox="allow-scripts"
        srcDoc={sandboxDocument}
        onLoad={publishToFrame}
        data-testid="standalone-live-page-frame"
      />
    </main>
  );
}
