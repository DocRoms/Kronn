import { EMBED_SETTINGS_PATH, PAGE_PATHS, STANDALONE_PATHS } from './routes';

/**
 * Before pages had addresses, deep links lived in the hash. Those links are
 * still out there: in Live Pages stored in the database, in messages, in
 * bookmarks and in the CLI. Each one keeps working, by redirecting to the
 * address that replaced it. This mapping is permanent, not transitional.
 *
 * An in-page anchor (`#settings-server`) is not one of them: it returns `null`.
 */
const PREFIXES: [legacyPrefix: string, canonicalPrefix: string, fallback: string][] = [
  // `#discussion-<id>?message=<id>`: the query survives as the address's own.
  // The fallback is where an id that cannot be decoded lands: the page, bare.
  ['#discussion-', `${PAGE_PATHS.discussions}/`, PAGE_PATHS.discussions],
  ['#project-', `${PAGE_PATHS.projects}/`, PAGE_PATHS.projects],
  ['#page/', `${STANDALONE_PATHS.page}/`, PAGE_PATHS.pages],
  ['#pages/mosaic?', `${STANDALONE_PATHS.pagesMosaic}?`, PAGE_PATHS.pages],
  ['#discussions/mosaic?', `${STANDALONE_PATHS.discussionsMosaic}?`, PAGE_PATHS.discussions],
];

/** Whether a path segment is valid percent-encoding: the router decodes it once. */
function decodable(segment: string): boolean {
  try {
    decodeURIComponent(segment);
    return true;
  } catch {
    return false;
  }
}

// Exact match or followed by its query, like `#config`: it is a whole section.
const LEGACY_EMBED_SETTINGS = '#settings/artifacts';

export function legacyHashToPath(hash: string): string | null {
  if (hash === '#config') return PAGE_PATHS.settings;
  // `#settings/artifacts?origin=<site>`: the site to type in survives as the query.
  if (hash === LEGACY_EMBED_SETTINGS || hash.startsWith(`${LEGACY_EMBED_SETTINGS}?`)) {
    return `${EMBED_SETTINGS_PATH}${hash.slice(LEGACY_EMBED_SETTINGS.length)}`;
  }
  for (const [legacyPrefix, canonicalPrefix, fallback] of PREFIXES) {
    if (!hash.startsWith(legacyPrefix)) continue;
    const rest = hash.slice(legacyPrefix.length);
    if (!rest) return null;
    // A malformed id (`%E0%A4%A`) names nothing: its page, rather than an
    // address the router cannot read.
    const id = rest.split('?', 1)[0];
    return decodable(id) ? `${canonicalPrefix}${rest}` : fallback;
  }
  return null;
}
