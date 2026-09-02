import { useCallback, useEffect, useRef, useState } from 'react';
import type { RefObject } from 'react';
import type { LivePageDetail } from '../types/generated';
import { pages as pagesApi } from './api';
import { runtimeData } from './live-page-sandbox';
import { type Pipeline } from './live-page-pipeline';
import { resolveBindingPipelines, type MirrorResult } from './live-page-mirror';
import { userError } from './userError';

// Live-refresh cadence shared by both surfaces that embed a Live Page (the
// published `StandaloneLivePage` and the in-app Pages editor preview). Fast
// while a workflow is actively publishing (the page's data_revision keeps
// moving) or a bound run is non-terminal, then backs off to an idle heartbeat
// once things go quiet. The page HTML re-renders on the `kronn:page-data`
// event, so refreshing the mirror is all it takes to see steps validate one by
// one without a manual reload.
const ACTIVE_REFRESH_MS = 4_000;
const IDLE_REFRESH_MS = 30_000;
const QUIET_POLLS_BEFORE_IDLE = 3;

function isDocumentHidden(): boolean {
  return typeof document !== 'undefined' && document.visibilityState === 'hidden';
}

export interface UseLivePageMirrorParams {
  /** Page to mirror; a falsy value stops the loop (nothing selected yet). */
  pageId: string | null | undefined;
  bridgeChannel: string;
  iframeRef: RefObject<HTMLIFrameElement | null>;
  /**
   * `true` — the hook owns the page detail and fetches it inside the poll loop
   * (published standalone view). `false` — the caller supplies `externalDetail`
   * (the editor already loads it) and the loop only mirrors the bound run.
   */
  ownsDetail: boolean;
  /** Caller-owned detail, used when `ownsDetail` is false. */
  externalDetail?: LivePageDetail | null;
}

export interface UseLivePageMirrorResult {
  detail: LivePageDetail | null;
  pipelines: Record<string, Pipeline>;
  /** First-load error; only ever set when `ownsDetail` is true. */
  error: string | null;
  /**
   * Push the current detail + mirrored pipelines into the iframe. Overlays each
   * pipeline as a snapshot dataset so the Page reads it exactly like a published
   * dataset (`KronnPageData.datasets.<name>.current`). Skips the re-push when
   * nothing changed (signature guard) so an idle poll never rebuilds the frame's
   * DOM and wipes transient UI; `force` (on a fresh document load) always posts.
   */
  publishToFrame: (force?: boolean) => void;
}

export function useLivePageMirror({
  pageId,
  bridgeChannel,
  iframeRef,
  ownsDetail,
  externalDetail = null,
}: UseLivePageMirrorParams): UseLivePageMirrorResult {
  const [ownedDetail, setOwnedDetail] = useState<LivePageDetail | null>(null);
  const [pipelines, setPipelines] = useState<Record<string, Pipeline>>({});
  const [error, setError] = useState<string | null>(null);
  // Signature of the last data posted into the iframe. The loop re-fetches on a
  // timer and setDetail() always yields a new object, so without this guard we'd
  // re-post identical data every poll — forcing every Page to rebuild its DOM
  // and wiping transient UI (an open form, a typed comment, an expanded row).
  const lastPublishedSig = useRef<string | null>(null);

  const detail = ownsDetail ? ownedDetail : externalDetail;

  // Keep the caller's detail reachable from the poll loop without restarting it:
  // in external mode the loop reads its data_revision to notice movement even
  // though a same-id revision bump doesn't re-run the effect.
  const externalDetailRef = useRef(externalDetail);
  externalDetailRef.current = externalDetail;

  useEffect(() => {
    // Reset per-page so one page's mirror never briefly overlays the next.
    setPipelines({});
    lastPublishedSig.current = null;
    if (ownsDetail) {
      setOwnedDetail(null);
      setError(null);
    }
    if (!pageId) return undefined;
    const id = pageId;
    let active = true;
    // In-flight guard: a tick suspended on an await must not let a second chain
    // start (a visibility flip or the scheduled timer), which would double the
    // poll load and race setPipelines out of order.
    let ticking = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let quietPolls = 0;
    let lastRevision: number | null = null;
    let firstLoad = true;

    function schedule(ms: number): void {
      if (!active) return;
      timer = setTimeout(() => { void tick(); }, ms);
    }

    async function tick(): Promise<void> {
      if (!active || ticking) return;
      // Don't poll a backgrounded tab; a real return fires an immediate refresh.
      if (isDocumentHidden()) { schedule(IDLE_REFRESH_MS); return; }
      ticking = true;
      try {
        let mirror: MirrorResult | null;
        let revision: number | null;
        if (ownsDetail) {
          // Independent requests run concurrently. A mirror failure must NOT
          // fail the page fetch, so its rejection is isolated to keep the last
          // good mirror rather than blanking the pipeline.
          const [page, mirrorResult] = await Promise.all([
            pagesApi.get(id),
            resolveBindingPipelines(id).catch(() => null),
          ]);
          if (!active) return;
          mirror = mirrorResult;
          revision = page.data_revision;
          setOwnedDetail(page);
          setError(null);
        } else {
          mirror = await resolveBindingPipelines(id);
          if (!active) return;
          revision = externalDetailRef.current?.data_revision ?? null;
        }
        // Stay on the fast cadence while the page data OR any mirrored run moves.
        const revisionMoved = lastRevision === null || revision !== lastRevision;
        const changed = revisionMoved || (mirror?.active ?? false);
        lastRevision = revision;
        quietPolls = changed ? 0 : quietPolls + 1;
        if (mirror) setPipelines(mirror.pipelines);
      } catch (cause) {
        if (!active) return;
        // Owned mode surfaces only the initial failure; a transient poll error
        // keeps the last good render on screen. External mode keeps the last
        // good mirror and never surfaces here (the caller owns detail errors).
        if (ownsDetail && firstLoad) setError(userError(cause));
      } finally {
        firstLoad = false;
        ticking = false;
      }
      if (!active) return;
      schedule(quietPolls >= QUIET_POLLS_BEFORE_IDLE ? IDLE_REFRESH_MS : ACTIVE_REFRESH_MS);
    }

    function onVisibilityChange(): void {
      // A running tick will schedule the next poll itself; only kick a fresh one
      // when idle, and drop the still-scheduled timer so it can't double up.
      if (!active || isDocumentHidden() || ticking) return;
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
    // `ownsDetail` is a constant per call site; the loop restarts only per page.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pageId, ownsDetail]);

  const publishToFrame = useCallback((force = false) => {
    if (!detail) return;
    const target = iframeRef.current?.contentWindow ?? null;
    if (!target) return;
    const data = runtimeData(detail);
    for (const [name, pipeline] of Object.entries(pipelines)) {
      data.datasets[name] = { kind: 'snapshot', current: pipeline, points: [] };
    }
    const signature = `${detail.data_revision}|${JSON.stringify(pipelines)}`;
    if (!force && signature === lastPublishedSig.current) return;
    lastPublishedSig.current = signature;
    target.postMessage({
      type: 'kronn:page-data',
      version: 1,
      channel_id: bridgeChannel,
      data,
    }, '*');
  }, [bridgeChannel, detail, iframeRef, pipelines]);

  return { detail, pipelines, error, publishToFrame };
}
