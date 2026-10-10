/**
 * Third-party content a Live Page asks the host to draw.
 *
 * A Page marks a placeholder with the full URL of what it wants to show
 * (`<div data-kronn-embed="https://player.example.com/embed/abc">`). The host
 * draws it only when the URL's origin (scheme, host and port, compared exactly)
 * is one of this Kronn's allowed sites (Configuration → Artifacts → External
 * content). Anything else gets a warning drawn by Kronn, never the content.
 * Adding a service is a configuration change, never a code change.
 */
import type { LivePageEmbedPlacement } from './live-page-sandbox';

/** The most players one Page may show at once, counted after the check. */
export const MAX_LIVE_PAGE_EMBEDS = 8;
/** The most placeholders the bridge reports, allowed or not. A separate,
 * larger bound so refused placeholders never crowd out the allowed ones. */
export const MAX_LIVE_PAGE_EMBED_REPORTS = 64;
export const MAX_LIVE_PAGE_EMBED_URL_CHARS = 2048;

function parseWebUrl(raw: unknown): URL | null {
  if (typeof raw !== 'string') return null;
  const value = raw.trim();
  if (!value || value.length > MAX_LIVE_PAGE_EMBED_URL_CHARS) return null;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  if ((url.protocol !== 'https:' && url.protocol !== 'http:') || !url.hostname) return null;
  if (url.username || url.password) return null;
  return url;
}

/** `scheme://host[:port]` of an embed URL, or `null` when Kronn would never draw it. */
export function embedUrlOrigin(raw: unknown): string | null {
  return parseWebUrl(raw)?.origin ?? null;
}

/**
 * The origin a user typed, normalized exactly as the backend stores it, or
 * `null` when it is not a bare origin (a trailing `/` is fine; a path, query,
 * fragment or credentials are not).
 */
export function normalizeEmbedOrigin(raw: string): string | null {
  const url = parseWebUrl(raw);
  if (!url || url.pathname !== '/' || url.search || url.hash || /[?#]/.test(raw.trim())) return null;
  return url.origin;
}

export interface LivePageEmbedPlayer {
  placement: LivePageEmbedPlacement;
  url: string;
  origin: string;
  /** Why a reload notice replaces it: the site was allowed after load, or a site the document's policy admits was revoked. */
  reason?: 'added' | 'revoked' | 'pending' | 'unknown' | 'httpOnly';
}

export interface LivePageEmbedPlan {
  /** Allowed content, at most `MAX_LIVE_PAGE_EMBEDS`. */
  players: LivePageEmbedPlayer[];
  /** Valid URLs from sites this Kronn has not allowed, as many again. */
  blocked: LivePageEmbedPlayer[];
  /** Allowed after this document loaded: its frame policy still refuses them. */
  reload: LivePageEmbedPlayer[];
}

const NO_PLAN: LivePageEmbedPlan = { players: [], blocked: [], reload: [] };

/**
 * Decide, on the host, what each placeholder gets. Nothing is drawn before the
 * allowed sites are known (`null`). Malformed URLs get nothing; refused ones
 * do not count against the players' quota. A site allowed after the document
 * loaded (absent from `framableOrigins`) gets a reload notice, since the
 * browser would refuse to frame it. Once a site the document's policy admits
 * is revoked, every player gets one: any of them may have navigated, or may
 * still navigate, to that site, which the document's policy keeps admitting.
 * While a change is announced but not read yet (`suspended`), or when the
 * document's policy is unknown (`framableOrigins` is `null`), every player
 * gets one too.
 */
export function planLivePageEmbeds(
  placements: readonly LivePageEmbedPlacement[],
  allowedOrigins: ReadonlySet<string> | null,
  framableOrigins: ReadonlySet<string> | null = allowedOrigins,
  suspended = false,
): LivePageEmbedPlan {
  if (!allowedOrigins) return NO_PLAN;
  const players: LivePageEmbedPlayer[] = [];
  const blocked: LivePageEmbedPlayer[] = [];
  const reload: LivePageEmbedPlayer[] = [];
  const policyRevoked = !!framableOrigins && [...framableOrigins].some(origin => !allowedOrigins.has(origin));
  const seen = new Set<string>();
  for (const placement of placements) {
    if (seen.has(placement.key)) continue;
    seen.add(placement.key);
    const url = parseWebUrl(placement.url);
    if (!url) continue;
    const entry = { placement, url: url.href, origin: url.origin };
    if (allowedOrigins.has(url.origin)) {
      // CSP lets an http source match its https upgrade too, so the host never
      // frames an http site whose https address is not allowed: a reload cannot help.
      const httpOnly = url.protocol === 'http:' && !allowedOrigins.has(`https://${url.host}`);
      if (!framableOrigins || suspended || policyRevoked || httpOnly || !framableOrigins.has(url.origin)) {
        const reason = !framableOrigins ? 'unknown' : suspended ? 'pending' : policyRevoked ? 'revoked' : httpOnly ? 'httpOnly' : 'added';
        if (reload.length < MAX_LIVE_PAGE_EMBEDS) reload.push({ ...entry, reason });
      } else if (players.length < MAX_LIVE_PAGE_EMBEDS) {
        players.push(entry);
      }
    } else if (blocked.length < MAX_LIVE_PAGE_EMBEDS) {
      blocked.push(entry);
    }
  }
  return { players, blocked, reload };
}

/** Whether any of the placeholder can be seen, its clip included. */
export function isEmbedPlacementShown({ visible, rect, clip }: LivePageEmbedPlacement): boolean {
  return visible && rect.width > 0 && rect.height > 0 && (!clip || (clip.width > 0 && clip.height > 0));
}

export interface LivePageEmbedStyle {
  left: number;
  top: number;
  width: number;
  height: number;
  borderRadius?: string;
  visibility: 'visible' | 'hidden';
  clipPath?: string;
}

/**
 * Where the host draws a placeholder's content, cut to what the Page's own
 * scrolling containers leave visible. `clip-path` also clips hit-testing: a
 * click outside the visible part goes through to the Page's controls below.
 */
export function embedPlacementStyle(placement: LivePageEmbedPlacement, shown: boolean): LivePageEmbedStyle {
  const { rect, clip } = placement;
  const style: LivePageEmbedStyle = {
    left: rect.left,
    top: rect.top,
    width: rect.width,
    height: rect.height,
    borderRadius: placement.radius,
    visibility: shown ? 'visible' : 'hidden',
  };
  if (clip) {
    const top = Math.max(0, clip.top - rect.top);
    const left = Math.max(0, clip.left - rect.left);
    const right = Math.max(0, rect.left + rect.width - (clip.left + clip.width));
    const bottom = Math.max(0, rect.top + rect.height - (clip.top + clip.height));
    if (top || left || right || bottom) style.clipPath = `inset(${top}px ${right}px ${bottom}px ${left}px)`;
  }
  return style;
}
