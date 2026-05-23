// Rewrite relative `<img src>` in rendered project docs so repo-local images
// (e.g. a README's `docs/screenshots/foo.png`) actually load in the viewer.
//
// A relative src is resolved against the CURRENT doc's directory — a root
// README's base is '', while a file under `docs/architecture/sequences/`
// resolves `../../diagram.png` correctly — then pointed at
//   GET /api/projects/:id/doc-asset?path=<repo-relative>
// which is same-origin, so `img-src 'self'` covers it without touching the
// CSP. External / data: / blob: URLs are left untouched.

/** Resolve `.`/`..` segments. Returns null if the path escapes the repo root. */
function normalizeRepoPath(parts: string[]): string | null {
  const out: string[] = [];
  for (const part of parts) {
    if (part === '' || part === '.') continue;
    if (part === '..') {
      if (out.length === 0) return null; // escapes the project root
      out.pop();
    } else {
      out.push(part);
    }
  }
  return out.length > 0 ? out.join('/') : null;
}

/**
 * Build the doc-asset URL for a relative image `src`, or return `null` when
 * `src` is external (has a scheme / protocol-relative) or escapes the root —
 * in which case the caller leaves the original `src` as-is.
 *
 * @param baseDir directory of the current doc (e.g. `docs/architecture`), '' for root.
 * @param apiBase API base URL ('' = same origin).
 */
export function resolveDocAssetUrl(
  projectId: string,
  baseDir: string,
  src: string,
  apiBase: string,
): string | null {
  if (!src) return null;
  // URI scheme (http:, https:, data:, blob:, mailto:…) or protocol-relative
  // → external, leave alone.
  if (/^[a-z][a-z0-9+.-]*:/i.test(src) || src.startsWith('//')) return null;

  const raw = src.startsWith('/')
    ? src.slice(1) // repo-root-absolute (`/docs/x.png`)
    : `${baseDir ? `${baseDir}/` : ''}${src}`; // relative to the current doc dir
  const rel = normalizeRepoPath(raw.split('/'));
  if (!rel) return null;

  return `${apiBase}/api/projects/${encodeURIComponent(projectId)}/doc-asset?path=${encodeURIComponent(rel)}`;
}

// Minimal hast node shape — avoids a hard dependency on the `hast` types.
interface HastNode {
  type: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
}

/**
 * rehype plugin that rewrites every relative `<img>` `src` to the project's
 * doc-asset route. Must run AFTER rehype-sanitize (sanitize keeps relative
 * `src`; we then repoint it).
 */
export function rehypeRewriteDocImages(opts: { projectId: string; baseDir: string; apiBase: string }) {
  return (tree: HastNode) => {
    const walk = (node: HastNode) => {
      if (node.tagName === 'img' && node.properties && typeof node.properties.src === 'string') {
        const rewritten = resolveDocAssetUrl(opts.projectId, opts.baseDir, node.properties.src, opts.apiBase);
        if (rewritten) node.properties.src = rewritten;
      }
      node.children?.forEach(walk);
    };
    walk(tree);
  };
}
