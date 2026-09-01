// Reads the media fields a run carries, defensively.
//
// The projection is versioned by the backend (`schema_version: 1`), and every
// optional field is ABSENT until measured — a running generation has no cost
// and no dimensions because nothing has been billed or produced, not because
// they are zero. A malformed or missing result must therefore degrade to "no
// media details", never to zeros.

/** The only projection version this reader understands. A newer backend must
 * not be interpreted through older assumptions: fields could have changed
 * meaning, and rendering them anyway would misreport a real generation. */
export const MEDIA_RUN_SCHEMA_VERSION = 1;

export type MediaRunModality = 'image' | 'video';

export type MediaRunDetails = {
  modality: MediaRunModality;
  costUsd?: number;
  isByok?: boolean;
  width?: number;
  height?: number;
  durationMs?: number;
  assetId?: string;
};

function positive(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : undefined;
}

/** `null` when the run carries no usable media projection. */
export function mediaRunDetails(result: unknown): MediaRunDetails | null {
  if (!result || typeof result !== 'object') return null;
  const raw = result as Record<string, unknown>;
  // The version is checked, not merely advertised: an absent or unknown one
  // means this reader cannot vouch for the field meanings, so it declines
  // rather than displaying values it may be misinterpreting.
  if (raw.schema_version !== MEDIA_RUN_SCHEMA_VERSION) return null;
  const modality = raw.modality;
  if (modality !== 'image' && modality !== 'video') return null;
  return {
    modality,
    // 0 USD is legitimate under BYOK, so only the type is checked here.
    costUsd: typeof raw.cost_usd === 'number' && Number.isFinite(raw.cost_usd) ? raw.cost_usd : undefined,
    isByok: typeof raw.is_byok === 'boolean' ? raw.is_byok : undefined,
    width: positive(raw.width),
    height: positive(raw.height),
    durationMs: positive(raw.media_duration_ms),
    assetId: typeof raw.asset_id === 'string' && raw.asset_id ? raw.asset_id : undefined,
  };
}
