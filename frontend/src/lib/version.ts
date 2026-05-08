// Lenient version comparator — TypeScript mirror of `backend/core/versions.rs`.
//
// Kept in sync by the unit tests on both sides (same cases). We need a
// frontend copy so per-agent `update_available` can be computed at render
// time without a per-row backend round-trip. RTK keeps using the backend
// computation (the API surface already exposes it).
//
// Rules:
//   - Strip a leading `v`.
//   - Strip pre-release / build metadata after `-` or `+` (semver convention).
//   - Compare dotted numeric components, zero-padding the shorter list.
//   - Any parse failure → `false` (better silent than wrong).

function parseVersion(v: string): number[] | null {
  const trimmed = v.trim().replace(/^v/, '');
  const core = trimmed.split(/[-+]/)[0];
  if (!core) return null;
  const parts = core.split('.').map(s => Number(s));
  if (parts.some(n => !Number.isFinite(n) || n < 0 || !Number.isInteger(n))) {
    return null;
  }
  return parts;
}

/** Returns `true` iff `installed` is strictly older than `latest` under
 *  lenient semver. Unparsable inputs return `false`. */
export function isUpdateAvailable(installed: string | null | undefined, latest: string | null | undefined): boolean {
  if (!installed || !latest) return false;
  const i = parseVersion(installed);
  const l = parseVersion(latest);
  if (!i || !l) return false;
  const len = Math.max(i.length, l.length);
  for (let k = 0; k < len; k++) {
    const iv = i[k] ?? 0;
    const lv = l[k] ?? 0;
    if (iv < lv) return true;
    if (iv > lv) return false;
  }
  return false;
}
