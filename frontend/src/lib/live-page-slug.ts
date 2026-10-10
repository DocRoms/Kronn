const UUID_LIKE = /^[0-9a-f]{8}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{12}$/;

/** Mirrors the backend rule so a bad slug is caught before the round trip. */
export function isValidPageSlug(slug: string): boolean {
  return slug.length > 0
    && slug.length <= 100
    && /^[a-z0-9]+(-[a-z0-9]+)*$/.test(slug)
    && !UUID_LIKE.test(slug);
}
