import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import {
  ArrowDown, ArrowUp, CircleMinus, CirclePlus, Film, GripVertical, Loader2, Play, RotateCcw,
  SkipBack, SkipForward, X,
} from 'lucide-react';
import type { ContextFile } from '../types/generated';
import { discussions as discussionsApi } from '../lib/api';
import {
  clipCostUsd, clipDurationMs, filmTotals, formatUsd, gapAt, moveClip, orderVideos, slotForGap,
  type Gap, type Slot, type Zone,
} from '../lib/videoSequence';

type T = (key: string, ...args: (string | number)[]) => string;

function formatDuration(ms: number, t: T): string {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return t('disc.assets.durationSeconds', seconds);
  return t('disc.assets.durationMinutes', Math.floor(seconds / 60), String(seconds % 60).padStart(2, '0'));
}

interface ClipSources {
  urls: Record<string, string>;
  failed: Set<string>;
  load: (clip: ContextFile | undefined) => void;
}

/**
 * The clips' bytes, fetched once each and shared by the thumbnails and the
 * player, then released with the editor.
 */
function useClipSources(discussionId: string): ClipSources {
  const [urls, setUrls] = useState<Record<string, string>>({});
  const [failed, setFailed] = useState<Set<string>>(new Set());
  const loaded = useRef(new Map<string, string>());
  const loading = useRef(new Set<string>());
  const mounted = useRef(true);

  const load = useCallback((clip: ContextFile | undefined) => {
    if (!clip || loaded.current.has(clip.id) || loading.current.has(clip.id)) return;
    loading.current.add(clip.id);
    discussionsApi.contextFileBlob(discussionId, clip.id)
      .then(blob => {
        if (!mounted.current) return;
        const url = URL.createObjectURL(blob);
        loaded.current.set(clip.id, url);
        setUrls(previous => ({ ...previous, [clip.id]: url }));
      })
      .catch(() => {
        if (mounted.current) setFailed(previous => new Set(previous).add(clip.id));
      })
      .finally(() => { loading.current.delete(clip.id); });
  }, [discussionId]);

  useEffect(() => {
    mounted.current = true;
    const urlsToRelease = loaded.current;
    return () => {
      mounted.current = false;
      urlsToRelease.forEach(url => URL.revokeObjectURL(url));
      urlsToRelease.clear();
    };
  }, []);

  return { urls, failed, load };
}

/**
 * The clip's first frame, loaded once the row is on screen. Its metadata also
 * gives the length of a clip Kronn did not measure itself, such as an upload.
 */
function ClipThumb({
  clip,
  sources,
  onDuration,
}: {
  clip: ContextFile;
  sources: ClipSources;
  onDuration: (clipId: string, ms: number) => void;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const url = sources.urls[clip.id];
  const { load } = sources;

  useEffect(() => {
    if (url) return;
    const node = ref.current;
    if (!node || typeof IntersectionObserver === 'undefined') {
      load(clip);
      return;
    }
    const observer = new IntersectionObserver(entries => {
      if (!entries.some(entry => entry.isIntersecting)) return;
      observer.disconnect();
      load(clip);
    }, { rootMargin: '120px' });
    observer.observe(node);
    return () => observer.disconnect();
  }, [clip, load, url]);

  return (
    <span ref={ref} className="disc-sequence-thumb" aria-hidden="true" data-testid="video-sequence-thumb">
      {url ? (
        <video
          src={url}
          preload="metadata"
          muted
          playsInline
          onLoadedMetadata={event => {
            const video = event.currentTarget;
            if (Number.isFinite(video.duration) && video.duration > 0) {
              video.currentTime = Math.min(0.1, video.duration / 10);
              onDuration(clip.id, Math.round(video.duration * 1000));
            }
          }}
        />
      ) : (
        <Film size={12} />
      )}
    </span>
  );
}

/**
 * Assets > Editor: which of a discussion's clips make the film, in what
 * order, and the whole film played through. The order is the discussion's,
 * kept server-side, so it survives a reload and follows the human elsewhere.
 */
export function VideoSequenceEditor({
  discussionId,
  videos,
  t,
}: {
  discussionId: string;
  videos: ContextFile[];
  t: T;
}) {
  // `null` until the stored order has been read: the list is not drawn in
  // one order and then jumped into another.
  const [arranged, setArranged] = useState<{ included: string[]; excluded: string[] } | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  // The clip being dragged, and the gap it would be dropped in: drawn as a
  // line between two rows, so the drop lands where the eye expects.
  const [dragging, setDragging] = useState<Slot | null>(null);
  const [gap, setGap] = useState<Gap | null>(null);
  // Lengths read by the browser, for clips Kronn did not measure.
  const [measured, setMeasured] = useState<Record<string, number>>({});
  const sources = useClipSources(discussionId);
  const recordDuration = useCallback((clipId: string, ms: number) => {
    setMeasured(previous => (previous[clipId] === ms ? previous : { ...previous, [clipId]: ms }));
  }, []);

  useEffect(() => {
    let current = true;
    discussionsApi.videoSequence(discussionId)
      .then(sequence => {
        if (current) setArranged({ included: sequence.file_ids, excluded: sequence.excluded_ids });
      })
      // Unreadable, every clip still plays, in the order it was made.
      .catch(() => { if (current) setArranged({ included: [], excluded: [] }); });
    return () => { current = false; };
  }, [discussionId]);

  const zones = useMemo(
    () => orderVideos(videos, arranged?.included ?? [], arranged?.excluded ?? []),
    [videos, arranged],
  );

  const move = useCallback((from: Slot, to: Slot) => {
    const next = moveClip(zones, from, to);
    if (next === zones) return;
    const ids = {
      included: next.included.map(video => video.id),
      excluded: next.excluded.map(video => video.id),
    };
    setArranged(ids);
    setSaveError(null);
    discussionsApi.setVideoSequence(discussionId, ids.included, ids.excluded)
      .catch(error => setSaveError(error instanceof Error ? error.message : String(error)));
  }, [discussionId, zones]);

  // No line where the clip would not move: on either side of itself.
  const showGap = (next: Gap) => {
    const shown = dragging && slotForGap(dragging, next) ? next : null;
    if (shown?.zone !== gap?.zone || shown?.before !== gap?.before) setGap(shown);
  };

  const endDrag = () => {
    setDragging(null);
    setGap(null);
  };

  const commitDrop = () => {
    const to = dragging && gap ? slotForGap(dragging, gap) : null;
    if (dragging && to) move(dragging, to);
    endDrag();
  };

  // Between two rows the pointer is over the list itself: the line stays
  // where it was instead of jumping to the end of the zone.
  const listEvents = {
    onDragOver: (event: DragEvent) => {
      event.preventDefault();
      event.stopPropagation();
    },
    onDrop: (event: DragEvent) => {
      event.preventDefault();
      event.stopPropagation();
      commitDrop();
    },
  };

  if (arranged === null) {
    return (
      <div className="disc-sequence-loading" role="status">
        <Loader2 size={14} className="spin" aria-hidden="true" /> {t('disc.assets.editorLoading')}
      </div>
    );
  }

  const dropLine = (zone: Zone, index: number): 'before' | 'after' | undefined => {
    if (gap?.zone !== zone) return undefined;
    if (gap.before === index) return 'before';
    return gap.before === index + 1 && index === zones[zone].length - 1 ? 'after' : undefined;
  };

  const totals = filmTotals(zones.included, measured);

  const clipMeta = (video: ContextFile) => {
    const duration = clipDurationMs(video, measured);
    const cost = clipCostUsd(video);
    const parts = [
      duration === null ? null : formatDuration(duration, t),
      video.ai_generation?.is_byok ? t('run.media.byok') : cost === null ? null : formatUsd(cost),
    ].filter((part): part is string => part !== null);
    return parts.length > 0
      ? <span className="disc-sequence-meta" data-testid="video-sequence-meta">{parts.join(' · ')}</span>
      : null;
  };

  const renderClip = (video: ContextFile, index: number, zone: Zone) => (
    <li
      key={video.id}
      className="disc-sequence-item"
      draggable
      data-testid={zone === 'included' ? 'video-sequence-item' : 'video-sequence-aside-item'}
      data-dragging={dragging?.zone === zone && dragging.index === index ? 'true' : undefined}
      data-drop={dropLine(zone, index)}
      onDragStart={event => {
        setDragging({ zone, index });
        event.dataTransfer.effectAllowed = 'move';
      }}
      onDragOver={event => {
        event.preventDefault();
        // The zone would take it too, and point at its end.
        event.stopPropagation();
        event.dataTransfer.dropEffect = 'move';
        showGap(gapAt(zone, index, event.clientY, event.currentTarget.getBoundingClientRect()));
      }}
      onDrop={event => {
        event.preventDefault();
        event.stopPropagation();
        commitDrop();
      }}
      onDragEnd={endDrag}
    >
      <GripVertical size={14} className="disc-sequence-grip" aria-hidden="true" />
      <ClipThumb clip={video} sources={sources} onDuration={recordDuration} />
      {zone === 'included' && <span className="disc-sequence-index">{index + 1}</span>}
      <span className="disc-sequence-name" title={video.filename}>{video.filename}</span>
      {clipMeta(video)}
      <span className="disc-sequence-moves">
        {zone === 'included' ? (
          <>
            <button
              type="button"
              className="disc-icon-btn"
              onClick={() => move({ zone, index }, { zone, index: index - 1 })}
              disabled={index === 0}
              aria-label={t('disc.assets.moveUp', video.filename)}
              title={t('disc.assets.moveUp', video.filename)}
            >
              <ArrowUp size={13} aria-hidden="true" />
            </button>
            <button
              type="button"
              className="disc-icon-btn"
              onClick={() => move({ zone, index }, { zone, index: index + 1 })}
              disabled={index === zones.included.length - 1}
              aria-label={t('disc.assets.moveDown', video.filename)}
              title={t('disc.assets.moveDown', video.filename)}
            >
              <ArrowDown size={13} aria-hidden="true" />
            </button>
            <button
              type="button"
              className="disc-icon-btn"
              onClick={() => move({ zone, index }, { zone: 'excluded', index: zones.excluded.length })}
              aria-label={t('disc.assets.excludeClip', video.filename)}
              title={t('disc.assets.excludeClip', video.filename)}
            >
              <CircleMinus size={13} aria-hidden="true" />
            </button>
          </>
        ) : (
          <button
            type="button"
            className="disc-icon-btn"
            onClick={() => move({ zone, index }, { zone: 'included', index: zones.included.length })}
            aria-label={t('disc.assets.includeClip', video.filename)}
            title={t('disc.assets.includeClip', video.filename)}
          >
            <CirclePlus size={13} aria-hidden="true" />
          </button>
        )}
      </span>
    </li>
  );

  return (
    <section className="disc-sequence" data-testid="video-sequence-editor" aria-label={t('disc.assets.tabEditor')}>
      <div className="disc-sequence-head">
        <p>{t('disc.assets.editorHint', zones.included.length)}</p>
        {/* What the final cut adds up to: only the clips in it. */}
        <dl className="disc-sequence-summary" data-testid="video-sequence-summary">
          <div>
            <dt>{t('disc.assets.totalDuration')}</dt>
            <dd data-testid="video-sequence-total-duration">
              {zones.included.length - totals.unknownDurations > 0 ? formatDuration(totals.durationMs, t) : '—'}
              {totals.unknownDurations > 0 && (
                <small>{t('disc.assets.unknownDurations', totals.unknownDurations)}</small>
              )}
            </dd>
          </div>
          <div>
            <dt>{t('disc.assets.totalCost')}</dt>
            <dd data-testid="video-sequence-total-cost">
              {zones.included.length - totals.uncountedCosts > 0 ? formatUsd(totals.costUsd) : '—'}
              {totals.uncountedCosts > 0 && (
                <small title={t('disc.assets.uncountedCostsHint')}>
                  {t('disc.assets.uncountedCosts', totals.uncountedCosts)}
                </small>
              )}
            </dd>
          </div>
        </dl>
        <button
          type="button"
          className="btn btn-sm btn-primary"
          onClick={() => setPlaying(true)}
          disabled={zones.included.length === 0}
          data-testid="video-sequence-play"
        >
          <Play size={13} aria-hidden="true" /> {t('disc.assets.playFinal')}
        </button>
      </div>
      {saveError && <p className="disc-sequence-error" role="alert">{t('disc.assets.orderNotSaved', saveError)}</p>}

      <ClipZone
        className="disc-sequence-zone"
        testId="video-sequence-final"
        title={<><Film size={13} aria-hidden="true" /> {t('disc.assets.finalCut')}</>}
        count={zones.included.length}
        empty={t('disc.assets.finalEmpty')}
        dropTarget={gap?.zone === 'included' && dragging?.zone !== 'included'}
        onDragOver={() => showGap({ zone: 'included', before: zones.included.length })}
        onDragLeave={() => setGap(current => (current?.zone === 'included' ? null : current))}
        onDrop={commitDrop}
      >
        <ol className="disc-sequence-list" {...listEvents}>
          {zones.included.map((video, index) => renderClip(video, index, 'included'))}
        </ol>
      </ClipZone>

      <ClipZone
        className="disc-sequence-zone disc-sequence-zone--aside"
        testId="video-sequence-aside"
        title={<><CircleMinus size={13} aria-hidden="true" /> {t('disc.assets.excludedZone')}</>}
        count={zones.excluded.length}
        empty={t('disc.assets.excludedEmpty')}
        dropTarget={gap?.zone === 'excluded' && dragging?.zone !== 'excluded'}
        onDragOver={() => showGap({ zone: 'excluded', before: zones.excluded.length })}
        onDragLeave={() => setGap(current => (current?.zone === 'excluded' ? null : current))}
        onDrop={commitDrop}
      >
        <ul className="disc-sequence-list" {...listEvents}>
          {zones.excluded.map((video, index) => renderClip(video, index, 'excluded'))}
        </ul>
      </ClipZone>

      {playing && (
        <SequencePlayer
          clips={zones.included}
          sources={sources}
          t={t}
          onClose={() => setPlaying(false)}
        />
      )}
    </section>
  );
}

/**
 * One drop zone of the editor. Pointed at outside its rows (its title, its
 * margins, its empty message), it points at its own end.
 */
function ClipZone({
  className,
  testId,
  title,
  count,
  empty,
  dropTarget,
  onDragOver,
  onDragLeave,
  onDrop,
  children,
}: {
  className: string;
  testId: string;
  title: ReactNode;
  count: number;
  empty: string;
  dropTarget: boolean;
  onDragOver: () => void;
  onDragLeave: () => void;
  onDrop: () => void;
  children: ReactNode;
}) {
  return (
    <div
      className={className}
      data-drop-target={dropTarget}
      data-testid={testId}
      onDragOver={(event: DragEvent) => {
        event.preventDefault();
        event.dataTransfer.dropEffect = 'move';
        onDragOver();
      }}
      onDragLeave={(event: DragEvent) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) onDragLeave();
      }}
      onDrop={(event: DragEvent) => {
        event.preventDefault();
        onDrop();
      }}
    >
      <h3 className="disc-sequence-zone-title">
        {title}
        <span className="disc-assets-filter-count">{count}</span>
      </h3>
      {count > 0 ? children : <p className="disc-sequence-empty">{empty}</p>}
    </div>
  );
}

/**
 * The film: every clip in order, each starting as the previous one ends. The
 * next clip is fetched while the current one plays, so the cut is not a wait.
 */
function SequencePlayer({
  clips,
  sources,
  t,
  onClose,
}: {
  clips: ContextFile[];
  sources: ClipSources;
  t: T;
  onClose: () => void;
}) {
  const [index, setIndex] = useState(0);
  const { urls, failed, load } = sources;

  useEffect(() => {
    load(clips[index]);
    load(clips[index + 1]);
  }, [clips, index, load]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  const finished = index >= clips.length;
  const clip = clips[index];

  // Portaled like the carousel: the panel clips its overflow, and the film
  // wants the whole screen.
  return createPortal(
    <div className="disc-sequence-player" role="dialog" aria-modal="true" aria-label={t('disc.assets.playFinal')} data-testid="video-sequence-player">
      <div className="disc-sequence-player-bar">
        <span data-testid="video-sequence-position">
          {finished
            ? t('disc.assets.finalEnded', clips.length)
            : t('disc.assets.clipPosition', index + 1, clips.length, clip.filename)}
        </span>
        <span className="disc-sequence-player-controls">
          <button type="button" className="disc-icon-btn" onClick={() => setIndex(i => Math.max(0, i - 1))} disabled={index === 0} aria-label={t('disc.assets.previousClip')} title={t('disc.assets.previousClip')}>
            <SkipBack size={14} aria-hidden="true" />
          </button>
          <button type="button" className="disc-icon-btn" onClick={() => setIndex(i => Math.min(clips.length, i + 1))} disabled={finished} aria-label={t('disc.assets.nextClip')} title={t('disc.assets.nextClip')}>
            <SkipForward size={14} aria-hidden="true" />
          </button>
          <button type="button" className="disc-icon-btn" onClick={onClose} aria-label={t('common.close')} title={t('common.close')} data-testid="video-sequence-close">
            <X size={14} aria-hidden="true" />
          </button>
        </span>
      </div>
      <div className="disc-sequence-screen">
        {finished ? (
          <button type="button" className="btn btn-sm" onClick={() => setIndex(0)} data-testid="video-sequence-replay">
            <RotateCcw size={13} aria-hidden="true" /> {t('disc.assets.replayFinal')}
          </button>
        ) : failed.has(clip.id) ? (
          <p className="disc-sequence-error" role="alert">
            {t('disc.assets.clipUnavailable', clip.filename)}
            <button type="button" className="btn btn-sm" onClick={() => setIndex(i => i + 1)}>{t('disc.assets.nextClip')}</button>
          </p>
        ) : urls[clip.id] ? (
          <video
            key={clip.id}
            src={urls[clip.id]}
            autoPlay
            controls
            playsInline
            data-testid="video-sequence-video"
            onEnded={() => setIndex(i => i + 1)}
          />
        ) : (
          <Loader2 size={20} className="spin" aria-label={t('disc.assets.editorLoading')} />
        )}
      </div>
    </div>,
    document.body,
  );
}
