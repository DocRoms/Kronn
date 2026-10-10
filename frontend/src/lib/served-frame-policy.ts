/** Name of the `<meta>` the host puts on each app document (desktop, native, Docker). */
export const SERVED_FRAME_SRC_META = 'kronn-served-frame-src';
const HOST_SOURCE = /^https?:\/\/[a-z0-9.:-]+$/;

/**
 * The origins a document's `frame-src` was served with, from its marker; `null`
 * when the marker is absent or malformed, so embeds fail closed.
 */
export function parseServedFrameSources(value: string | null | undefined): ReadonlySet<string> | null {
  const tokens = value ? value.split(' ') : [];
  if (tokens[0] !== "'self'" || !tokens.slice(1).every(token => HOST_SOURCE.test(token))) return null;
  return new Set(tokens.slice(1));
}

let served: ReadonlySet<string> | null | undefined;

/** Read once: the policy cannot change for the life of the document. */
export function servedFrameOrigins(): ReadonlySet<string> | null {
  if (served === undefined) {
    const meta = document.querySelector<HTMLMetaElement>(`meta[name="${SERVED_FRAME_SRC_META}"]`);
    served = parseServedFrameSources(meta?.content);
  }
  return served;
}

/** Test-only: read the marker again. */
export function resetServedFrameOriginsForTests(): void {
  served = undefined;
}
