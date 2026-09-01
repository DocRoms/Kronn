import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { LivePageDetail } from '../types/generated';
import { pages as pagesApi } from '../lib/api';
import {
  buildSandboxDocument,
  createLivePageActionRelay,
  createLivePageOpenLinkRelay,
  runtimeData,
} from '../lib/live-page-sandbox';
import { type Pipeline } from '../lib/live-page-pipeline';
import { resolveBindingPipelines, type MirrorResult } from '../lib/live-page-mirror';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import './StandaloneLivePage.css';

function channelId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `standalone-page-${Date.now()}-${Math.random()}`;
}

// Live-refresh cadence for the published (standalone) view. Fast while a
// workflow is actively publishing (data_revision keeps moving), then backs off
// to an idle heartbeat once things go quiet. The page HTML already re-renders
// on the `kronn:page-data` event, so refreshing the detail is all it takes to
// see steps validate one by one without a manual reload.
const ACTIVE_REFRESH_MS = 4_000;
const IDLE_REFRESH_MS = 30_000;
const QUIET_POLLS_BEFORE_IDLE = 3;

function isDocumentHidden(): boolean {
  return typeof document !== 'undefined' && document.visibilityState === 'hidden';
}

export function StandaloneLivePage({ pageId }: { pageId: string }) {
  const { t } = useT();
  const [detail, setDetail] = useState<LivePageDetail | null>(null);
  const [pipelines, setPipelines] = useState<Record<string, Pipeline>>({});
  const [error, setError] = useState<string | null>(null);
  const [bridgeChannel] = useState(channelId);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const linkRelayRef = useRef<ReturnType<typeof createLivePageOpenLinkRelay> | null>(null);
  const actionRelayRef = useRef<ReturnType<typeof createLivePageActionRelay> | null>(null);
  // Signature of the last data we posted into the iframe. The refresh loop
  // re-fetches on a timer and setDetail() always yields a new object, so without
  // this guard we'd re-post identical data every poll — forcing every Page to
  // rebuild its DOM and wiping transient UI (an open form, a typed comment, an
  // expanded row). We only push when this signature actually changes.
  const lastPublishedSig = useRef<string | null>(null);

  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let quietPolls = 0;
    let lastRevision: number | null = null;
    let firstLoad = true;

    function schedule(ms: number): void {
      if (!active) return;
      timer = setTimeout(() => { void tick(); }, ms);
    }

    async function tick(): Promise<void> {
      if (!active) return;
      // Don't poll a backgrounded tab; a real return fires an immediate refresh.
      if (isDocumentHidden()) { schedule(IDLE_REFRESH_MS); return; }
      try {
        const page = await pagesApi.get(pageId);
        if (!active) return;
        // Mirror bound workflow runs client-side (best-effort; a failure keeps
        // the last good mirror rather than blanking the pipeline).
        let mirror: MirrorResult | null = null;
        try {
          mirror = await resolveBindingPipelines(pageId);
        } catch {
          mirror = null;
        }
        if (!active) return;
        // Stay on the fast cadence while the page data OR any mirrored run moves.
        const revisionMoved = lastRevision === null || page.data_revision !== lastRevision;
        const changed = revisionMoved || (mirror?.active ?? false);
        lastRevision = page.data_revision;
        quietPolls = changed ? 0 : quietPolls + 1;
        setDetail(page);
        if (mirror) setPipelines(mirror.pipelines);
        setError(null);
      } catch (cause) {
        if (!active) return;
        // Only the initial load surfaces an error; a transient poll failure
        // keeps the last good render on screen instead of blanking it.
        if (firstLoad) setError(userError(cause));
      } finally {
        firstLoad = false;
      }
      if (!active) return;
      schedule(quietPolls >= QUIET_POLLS_BEFORE_IDLE ? IDLE_REFRESH_MS : ACTIVE_REFRESH_MS);
    }

    function onVisibilityChange(): void {
      if (!active || isDocumentHidden()) return;
      if (timer) clearTimeout(timer);
      void tick();
    }

    void tick();
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', onVisibilityChange);
    }

    return () => {
      active = false;
      if (timer) clearTimeout(timer);
      if (typeof document !== 'undefined') {
        document.removeEventListener('visibilitychange', onVisibilityChange);
      }
    };
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
  const publishToFrame = useCallback((force = false) => {
    if (!detail) return;
    const target = iframeRef.current?.contentWindow ?? null;
    if (!target) return;
    const data = runtimeData(detail);
    // Overlay each mirrored pipeline as a snapshot dataset so the Page reads it
    // exactly like a published dataset (`KronnPageData.datasets.<name>.current`).
    for (const [name, pipeline] of Object.entries(pipelines)) {
      data.datasets[name] = { kind: 'snapshot', current: pipeline, points: [] };
    }
    // Skip the re-push when nothing changed (see lastPublishedSig). `force` is set
    // on a fresh document load, which must always receive the current data.
    const signature = `${detail.data_revision}|${JSON.stringify(pipelines)}`;
    if (!force && signature === lastPublishedSig.current) return;
    lastPublishedSig.current = signature;
    target.postMessage({
      type: 'kronn:page-data',
      version: 1,
      channel_id: bridgeChannel,
      data,
    }, '*');
  }, [bridgeChannel, detail, pipelines]);
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
    // signature guard, so reset it and force this publish. Connecting both relays
    // once per loaded document (not on every push) keeps their private ports
    // stable so an in-flight decision isn't dropped by a refresh.
    lastPublishedSig.current = null;
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
