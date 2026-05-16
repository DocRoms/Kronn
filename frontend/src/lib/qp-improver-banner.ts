// 0.8.4 follow-up — persist "QP deployed at v<N>" per discussion so the
// banner can render a disabled "✅ déployé en v<N>" state instead of an
// active CTA when the user navigates back to the same discussion (or
// reloads the page) AFTER a successful deploy. Without this, the banner
// re-derives purely from the agent message content — which still
// contains the `KRONN:QP_IMPROVED` signal + the JSON block, so the CTA
// stayed active forever. Cf. user-reported UX bug, 0.8.4 dogfooding.
//
// Persistence is per-device (localStorage) keyed by discussion id. The
// QP version index is the source-of-truth value to display.

const STORAGE_KEY_PREFIX = 'kronn:qpDisc:';
const STORAGE_KEY_SUFFIX = ':deployedVersion';

export function deployedVersionKey(discId: string): string {
  return `${STORAGE_KEY_PREFIX}${discId}${STORAGE_KEY_SUFFIX}`;
}

export function getDeployedVersion(discId: string): number | null {
  try {
    const raw = localStorage.getItem(deployedVersionKey(discId));
    if (raw == null) return null;
    const n = parseInt(raw, 10);
    return Number.isFinite(n) && n > 0 ? n : null;
  } catch {
    // Safari private mode / quota / storage disabled — caller will fall
    // back to the active-CTA path; that's strictly less wrong than
    // throwing and hiding the banner entirely.
    return null;
  }
}

export function setDeployedVersion(discId: string, version: number): void {
  try {
    localStorage.setItem(deployedVersionKey(discId), String(version));
  } catch {
    // Same fallback rationale as the read path — losing the marker
    // means the user will see the active CTA again on the next render,
    // which is annoying but not destructive (the PUT is idempotent
    // from the user's perspective: redeploying the same JSON yields
    // an identical version snapshot).
  }
}

export function clearDeployedVersion(discId: string): void {
  try {
    localStorage.removeItem(deployedVersionKey(discId));
  } catch {
    // ignore
  }
}
