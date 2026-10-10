import { useLayoutEffect, useState } from 'react';
import { RotateCw, ShieldAlert } from 'lucide-react';
import type { LivePageEmbedPlacement } from '../lib/live-page-sandbox';
import { embedPlacementStyle, isEmbedPlacementShown, planLivePageEmbeds } from '../lib/live-page-embeds';
import { openEmbedSettings } from '../lib/live-page-navigation';
import { embedOriginsFramableByThisDocument, useEmbedAllowedOrigins, useEmbedOriginsSuspended } from '../hooks/useEmbedAllowedOrigins';
import { useT } from '../lib/I18nContext';
import './LivePageEmbedOverlay.css';

export interface LivePageEmbedOverlayProps {
  /** The Page iframe the content is drawn over. Its content box is the clip. */
  frameRef: { readonly current: HTMLIFrameElement | null };
  embeds: LivePageEmbedPlacement[];
  /** Opens the allowed-sites settings for a blocked origin. */
  onConfigureOrigin?: (origin: string) => void;
}

interface FrameBox { left: number; top: number; width: number; height: number }

function sameBox(a: FrameBox | null, b: FrameBox): boolean {
  return a !== null && a.left === b.left && a.top === b.top && a.width === b.width && a.height === b.height;
}

const RELOAD_DETAIL = {
  added: 'pages.embed.reloadNeededDetail',
  revoked: 'pages.embed.reloadRevokedDetail',
  pending: 'pages.embed.reloadPendingDetail',
  unknown: 'pages.embed.reloadUnknownDetail',
  httpOnly: 'pages.embed.httpOnlyDetail',
} as const;

/**
 * Draw the third-party content a Page placed, in the host, over the Page iframe.
 *
 * Content nested inside the Page would inherit its opaque-origin sandbox and
 * never play, and relaxing that sandbox would let the Page escape it. So the
 * Page only marks a placeholder (`data-kronn-embed="<url>"`), the bridge reports
 * where it is, and this layer draws the content there when the URL's site is
 * allowed on this Kronn, or a warning when it is not. The Page's CSP is
 * untouched: allowing a site lets Kronn frame it, never lets the Page's own
 * script reach it.
 *
 * The layer is sized to the iframe's content box and clips to it, so content
 * can never cover host UI outside the Page; each item is also cut to what the
 * Page's own scrolling containers leave visible. The bridge reports rectangles
 * in the iframe's viewport, which is exactly this layer's coordinate space.
 * Players are keyed and kept in first-seen order: moving an iframe in the DOM
 * reloads it, so a scroll or a reorder only changes its position.
 */
export function LivePageEmbedOverlay({ frameRef, embeds, onConfigureOrigin = openEmbedSettings }: LivePageEmbedOverlayProps) {
  const { t } = useT();
  const allowedOrigins = useEmbedAllowedOrigins();
  const suspended = useEmbedOriginsSuspended();
  const [box, setBox] = useState<FrameBox | null>(null);
  const [order, setOrder] = useState<string[]>([]);

  useLayoutEffect(() => {
    const frame = frameRef.current;
    if (!frame) return undefined;
    const measure = () => {
      const next = {
        left: frame.offsetLeft + frame.clientLeft,
        top: frame.offsetTop + frame.clientTop,
        width: frame.clientWidth,
        height: frame.clientHeight,
      };
      setBox(previous => (sameBox(previous, next) ? previous : next));
    };
    measure();
    const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(measure) : null;
    observer?.observe(frame);
    window.addEventListener('resize', measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener('resize', measure);
    };
  }, [frameRef]);

  const { players, blocked, reload } = planLivePageEmbeds(embeds, allowedOrigins, embedOriginsFramableByThisDocument(), suspended);
  const byKey = new Map(players.map(player => [player.placement.key, player]));
  // First-seen order, kept as derived state: a player that changes rank in the
  // Page must not move in the DOM, where moving it would reload it.
  const nextOrder = [
    ...order.filter(key => byKey.has(key)),
    ...[...byKey.keys()].filter(key => !order.includes(key)),
  ];
  if (nextOrder.length !== order.length || nextOrder.some((key, index) => key !== order[index])) {
    setOrder(nextOrder);
  }
  const ordered = nextOrder.flatMap(key => {
    const player = byKey.get(key);
    return player ? [player] : [];
  });
  if (ordered.length === 0 && blocked.length === 0 && reload.length === 0) return null;

  return (
    <div
      className="live-page-embed-overlay"
      data-testid="live-page-embed-overlay"
      style={box ? { left: box.left, top: box.top, width: box.width, height: box.height } : undefined}
    >
      {ordered.map(({ placement, url, origin }) => {
        const shown = isEmbedPlacementShown(placement);
        return (
          <iframe
            key={placement.key}
            className="live-page-embed-overlay__player"
            data-embed-key={placement.key}
            src={url}
            title={t('pages.embed.title', new URL(origin).host)}
            loading="lazy"
            allow="autoplay; encrypted-media; fullscreen; picture-in-picture"
            referrerPolicy="strict-origin-when-cross-origin"
            sandbox="allow-scripts allow-same-origin allow-popups allow-presentation"
            aria-hidden={shown ? undefined : true}
            tabIndex={shown ? undefined : -1}
            style={embedPlacementStyle(placement, shown)}
          />
        );
      })}
      {blocked.map(({ placement, origin }) => {
        const shown = isEmbedPlacementShown(placement);
        return (
          <div
            key={placement.key}
            className="live-page-embed-overlay__blocked"
            data-embed-key={placement.key}
            data-embed-origin={origin}
            role="note"
            aria-hidden={shown ? undefined : true}
            style={embedPlacementStyle(placement, shown)}
          >
            <ShieldAlert size={16} aria-hidden="true" />
            <div className="live-page-embed-overlay__blocked-text">
              <strong>{t('pages.embed.blocked')}</strong>
              <span>{t('pages.embed.blockedDetail', origin)}</span>
            </div>
            <button
              type="button"
              className="live-page-embed-overlay__configure"
              tabIndex={shown ? undefined : -1}
              onClick={() => onConfigureOrigin(origin)}
            >
              {t('pages.embed.configure')}
            </button>
          </div>
        );
      })}
      {reload.map(({ placement, origin, reason }) => {
        const shown = isEmbedPlacementShown(placement);
        if (reason === 'httpOnly') {
          const secure = origin.replace(/^http:/, 'https:');
          return (
            <div
              key={placement.key}
              className="live-page-embed-overlay__blocked"
              data-embed-key={placement.key}
              data-embed-origin={origin}
              data-embed-reason="http-only"
              role="note"
              aria-hidden={shown ? undefined : true}
              style={embedPlacementStyle(placement, shown)}
            >
              <ShieldAlert size={16} aria-hidden="true" />
              <div className="live-page-embed-overlay__blocked-text">
                <strong>{t('pages.embed.httpOnly')}</strong>
                <span>{t('pages.embed.httpOnlyDetail', origin, secure)}</span>
              </div>
              <button
                type="button"
                className="live-page-embed-overlay__configure"
                tabIndex={shown ? undefined : -1}
                onClick={() => onConfigureOrigin(secure)}
              >
                {t('pages.embed.configure')}
              </button>
            </div>
          );
        }
        return (
          <div
            key={placement.key}
            className="live-page-embed-overlay__blocked live-page-embed-overlay__blocked--reload"
            data-embed-key={placement.key}
            data-embed-origin={origin}
            role="note"
            aria-hidden={shown ? undefined : true}
            style={embedPlacementStyle(placement, shown)}
          >
            <RotateCw size={16} aria-hidden="true" />
            <div className="live-page-embed-overlay__blocked-text">
              <strong>{t('pages.embed.reloadNeeded')}</strong>
              <span>{t(RELOAD_DETAIL[reason ?? 'added'], origin)}</span>
            </div>
            <button
              type="button"
              className="live-page-embed-overlay__configure"
              tabIndex={shown ? undefined : -1}
              onClick={() => window.location.reload()}
            >
              {t('pages.embed.reload')}
            </button>
          </div>
        );
      })}
    </div>
  );
}
