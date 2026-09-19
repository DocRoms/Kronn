import type { ContextFile } from '../types/generated';

/** Where a clip sits in the editor: in the film, or set aside from it. */
export type Zone = 'included' | 'excluded';

export type Zones<T> = Record<Zone, T[]>;

/** One position in a zone. `index` equal to the zone's length means its end. */
export interface Slot {
  zone: Zone;
  index: number;
}

/**
 * A discussion's clips in playing order, and the ones set aside: the clips a
 * person arranged, as arranged, then any clip generated since, oldest first:
 * clips are made one after the other, so the order they were made in is the
 * film's default. A new clip is played until someone sets it aside, and a
 * clip that is gone simply drops out.
 */
export function orderVideos(
  videos: ContextFile[],
  arranged: string[],
  excluded: string[],
): Zones<ContextFile> {
  const byId = new Map(videos.map(video => [video.id, video]));
  const pick = (ids: string[]) => ids
    .map(id => byId.get(id))
    .filter((video): video is ContextFile => video !== undefined);
  const setAside = pick(excluded);
  const placed = pick(arranged.filter(id => !excluded.includes(id)));
  const known = new Set([...placed, ...setAside].map(video => video.id));
  const newer = videos
    .filter(video => !known.has(video.id))
    .sort((left, right) => left.created_at.localeCompare(right.created_at));
  return { included: [...placed, ...newer], excluded: setAside };
}

/** A copy of `items` with the one at `from` moved to `to`. */
export function moveItem<T>(items: T[], from: number, to: number): T[] {
  if (from === to || from < 0 || to < 0 || from >= items.length || to >= items.length) return items;
  const next = [...items];
  const [moved] = next.splice(from, 1);
  next.splice(to, 0, moved);
  return next;
}

/**
 * A copy of `zones` with one clip moved, within its zone or into the other.
 * Returns the same object when nothing moves, so no save is sent.
 */
export function moveClip<T>(zones: Zones<T>, from: Slot, to: Slot): Zones<T> {
  const source = zones[from.zone];
  if (from.index < 0 || from.index >= source.length) return zones;
  if (from.zone === to.zone) {
    // Dropped past the last clip of its own zone: it becomes the last.
    const moved = moveItem(source, from.index, Math.min(to.index, source.length - 1));
    return moved === source ? zones : { ...zones, [from.zone]: moved };
  }
  const target = zones[to.zone];
  const at = Math.max(0, Math.min(to.index, target.length));
  const clip = source[from.index];
  return {
    ...zones,
    [from.zone]: source.filter((_, index) => index !== from.index),
    [to.zone]: [...target.slice(0, at), clip, ...target.slice(at)],
  };
}

/**
 * Where a dragged clip would be inserted: before the clip at `before` in
 * `zone`, as the list stands while it is dragged. `before` equal to the
 * zone's length means its end.
 */
export interface Gap {
  zone: Zone;
  before: number;
}

/** The gap a pointer over the row at `index` points at: its upper half is before it. */
export function gapAt(zone: Zone, index: number, pointerY: number, row: { top: number; height: number }): Gap {
  return { zone, before: pointerY < row.top + row.height / 2 ? index : index + 1 };
}

/**
 * Where the clip dragged from `from` lands when dropped in `gap`, or null when
 * that gap is on either side of the clip itself and it would not move.
 */
export function slotForGap(from: Slot, gap: Gap): Slot | null {
  if (from.zone !== gap.zone) return { zone: gap.zone, index: gap.before };
  if (gap.before === from.index || gap.before === from.index + 1) return null;
  // Taken out first, the clip leaves one place free above the gap.
  return { zone: gap.zone, index: gap.before > from.index ? gap.before - 1 : gap.before };
}

/**
 * A clip's length: measured from the file for a generated clip, otherwise
 * read by the browser from the clip itself. Null until one of them is known.
 */
export function clipDurationMs(clip: ContextFile, measured: Record<string, number>): number | null {
  return clip.ai_generation?.duration_ms ?? measured[clip.id] ?? null;
}

/** The price declared for a clip, or null: an upload, no price declared, or billed on the user's own key. */
export function clipCostUsd(clip: ContextFile): number | null {
  const generation = clip.ai_generation;
  if (!generation || generation.is_byok) return null;
  return generation.cost_usd ?? null;
}

export interface FilmTotals {
  durationMs: number;
  /** Clips whose length is not known yet, so not in `durationMs`. */
  unknownDurations: number;
  costUsd: number;
  /** Clips with no price to add, so not in `costUsd`. Never counted as free. */
  uncountedCosts: number;
}

export function filmTotals(clips: ContextFile[], measured: Record<string, number>): FilmTotals {
  const totals: FilmTotals = { durationMs: 0, unknownDurations: 0, costUsd: 0, uncountedCosts: 0 };
  for (const clip of clips) {
    const duration = clipDurationMs(clip, measured);
    if (duration === null) totals.unknownDurations += 1;
    else totals.durationMs += duration;
    const cost = clipCostUsd(clip);
    if (cost === null) totals.uncountedCosts += 1;
    else totals.costUsd += cost;
  }
  return totals;
}

/** `$0.07`, and four decimals below a cent so a cheap clip does not read as free. */
export function formatUsd(usd: number): string {
  return `$${usd > 0 && usd < 0.01 ? usd.toFixed(4) : usd.toFixed(2)}`;
}
