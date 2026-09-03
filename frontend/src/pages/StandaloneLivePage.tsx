import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { pages as pagesApi } from '../lib/api';
import {
  buildSandboxDocument,
  createLivePageActionRelay,
  createLivePageOpenLinkRelay,
} from '../lib/live-page-sandbox';
import { useLivePageMirror } from '../lib/useLivePageMirror';
import { useT } from '../lib/I18nContext';
import './StandaloneLivePage.css';

function channelId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `standalone-page-${Date.now()}-${Math.random()}`;
}

export function StandaloneLivePage({ pageId }: { pageId: string }) {
  const { t } = useT();
  const [bridgeChannel] = useState(channelId);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const linkRelayRef = useRef<ReturnType<typeof createLivePageOpenLinkRelay> | null>(null);
  const actionRelayRef = useRef<ReturnType<typeof createLivePageActionRelay> | null>(null);

  // The published view owns its page detail: the shared mirror hook fetches it
  // and the bound run together on an adaptive cadence, pausing on a hidden tab.
  const { detail, error, publishToFrame } = useLivePageMirror({
    pageId,
    bridgeChannel,
    iframeRef,
    ownsDetail: true,
  });

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
  useEffect(() => {
    const relay = createLivePageOpenLinkRelay(bridgeChannel);
    linkRelayRef.current = relay;
    return () => {
      if (linkRelayRef.current === relay) linkRelayRef.current = null;
      relay.dispose();
    };
  }, [bridgeChannel]);

  // Broker page-authored actions (gate decisions, workflow triggers) to the
  // authenticated parent. The backend re-checks the page's binding on every call.
  const handleAction = useCallback(async (action: string, payload: unknown): Promise<unknown> => {
    if (action === 'gate.decide') {
      const request = (payload ?? {}) as {
        dataset?: string; run_id?: string; decision?: string; comment?: string | null;
      };
      if (!request.dataset || !request.run_id || !request.decision) {
        throw new Error('Missing dataset, run_id or decision');
      }
      return pagesApi.decideGate(pageId, {
        dataset: request.dataset,
        run_id: request.run_id,
        decision: request.decision,
        comment: request.comment ?? null,
      });
    }
    if (action === 'workflow.trigger') {
      const request = (payload ?? {}) as { dataset?: string; variables?: Record<string, string> };
      if (!request.dataset) throw new Error('Missing dataset');
      return pagesApi.triggerWorkflow(pageId, {
        dataset: request.dataset,
        variables: request.variables ?? {},
      });
    }
    throw new Error(`Unknown action: ${action}`);
  }, [pageId]);
  useEffect(() => {
    const relay = createLivePageActionRelay(bridgeChannel, handleAction);
    actionRelayRef.current = relay;
    return () => {
      if (actionRelayRef.current === relay) actionRelayRef.current = null;
      relay.dispose();
    };
  }, [bridgeChannel, handleAction]);

  // Connect the action relay once per loaded document (not on every data push),
  // so a decision in flight isn't dropped by a 4s refresh re-transferring the port.
  const handleFrameLoad = useCallback(() => {
    // A fresh document must receive the relays + current data regardless of the
    // signature guard, so force this publish. Connecting both relays once per
    // loaded document (not on every push) keeps their private ports stable so an
    // in-flight decision isn't dropped by a refresh.
    linkRelayRef.current?.connect(iframeRef.current?.contentWindow ?? null);
    actionRelayRef.current?.connect(iframeRef.current?.contentWindow ?? null);
    publishToFrame(true);
  }, [publishToFrame]);
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
        onLoad={handleFrameLoad}
        data-testid="standalone-live-page-frame"
      />
    </main>
  );
}
