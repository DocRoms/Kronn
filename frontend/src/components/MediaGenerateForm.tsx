// KT-540 — launch a generation from the discussion that will hold the asset.
//
// There is no modality switch: the operator picks a CONFIGURED model, exactly
// as the connection card offers its Economy/Standard/Advanced slots, and the
// modality follows from that choice. Asking for a modality first meant asking a
// question the configuration already answers — and offering a modality nobody
// had configured.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Clapperboard, Image as ImageIcon, Loader2, Paperclip, Sparkles, X } from 'lucide-react';
import { media, discussions as discussionsApi } from '../lib/api';
import { Dropdown, type DropdownOption } from './Dropdown';
import { SearchableSelect, type SearchableSelectOption } from './SearchableSelect';
import type {
  ExternalApiConnectionView,
  MediaModality,
  MediaFramePosition,
  MediaModelCapabilities,
  MediaReferenceMode,
} from '../lib/api';
import type { ContextFile } from '../types/generated';
import './MediaGenerateForm.css';

type T = (key: string, ...args: (string | number)[]) => string;

// Fallbacks, used ONLY when the provider advertises nothing — NVIDIA serves no
// media catalogue at all. Against a provider that does answer, these lists were
// actively wrong: `seedance-2.0-mini` refuses 3 s and 1080p, and accepts seven
// ratios of which this list showed four. Offering them meant offering a
// billable click that could only fail.
const FALLBACK_DURATIONS = [3, 5, 8];
const FALLBACK_RESOLUTIONS = ['480p', '720p', '1080p'];
/// Ratios shown with a proportional preview, like the live-page mosaic layouts:
/// `4:3` means nothing to most people until they see the shape.
const FALLBACK_RATIOS = ['16:9', '4:3', '1:1', '9:16'];
/// Preferred whenever the model accepts it.
const DEFAULT_RATIO = '16:9';

/// Width in pixels, or null when the bytes cannot be decoded here. Unknown must
/// never block: refusing on it would hide a source the provider accepts.
async function measureWidth(blob: Blob): Promise<number | null> {
  try {
    const bitmap = await createImageBitmap(blob);
    const { width } = bitmap;
    bitmap.close();
    return width;
  } catch {
    return null;
  }
}
// Preferred when the model offers them. Falling back to whatever the catalogue
// happens to list first gave a 3-second clip and a portrait frame nobody asked
// for.
const DEFAULT_DURATION = 5;
const DEFAULT_RESOLUTION = '480p';

/// Measured against OpenRouter on 02/09: a 8x8 source came back
/// `400 InvalidParameter — expected the width to be at least 300px`. The
/// refusal is synchronous and unbilled, but it costs the operator a launch
/// that could only fail, so the picture is measured here instead.
const MIN_REFERENCE_WIDTH_PX = 300;

/// Known shapes, so a ratio the provider names can still be drawn. An unknown
/// one is listed without a preview rather than dropped: the provider accepts
/// it, so the operator must be able to pick it.
const RATIO_SHAPES = new Set(['16:9', '4:3', '1:1', '9:16', '3:4', '21:9', '9:21', '2:3', '3:2', '4:5', '5:4']);

/** One configured media model: what the operator actually chooses. */
type Slot = {
  key: string;
  connectionId: string;
  connectionName: string;
  modality: MediaModality;
  model: string;
};

function slotsOf(connections: ExternalApiConnectionView[]): Slot[] {
  const slots: Slot[] = [];
  for (const connection of connections) {
    for (const modality of ['image', 'video'] as const) {
      const model = modality === 'image' ? connection.image_model : connection.video_model;
      if (!model || !model.trim()) continue;
      slots.push({
        key: `${connection.id}:${modality}`,
        connectionId: connection.id,
        connectionName: connection.display_name,
        modality,
        model: model.trim(),
      });
    }
  }
  return slots;
}

export function MediaGenerateForm({
  discussionId,
  onCreateDiscussion,
  onDiscussionCreated,
  connections,
  images = [],
  t,
  onLaunched,
  onImageAttached,
  initialSlotKey,
  initialReference,
}: {
  /** Existing room, or a creator used by the New discussion media entry. */
  discussionId?: string;
  /** Creates the room only after an explicit Generate click. */
  onCreateDiscussion?: () => Promise<string>;
  onDiscussionCreated?: (discussionId: string) => void;
  connections: ExternalApiConnectionView[];
  /** Images of THIS discussion, the only ones a generation may start from. */
  images?: ContextFile[];
  t: T;
  /** Fired once the backend accepted the job — after this, the form is
   *  already reset and reusable, so the caller can rely on it purely as a
   *  signal to reveal the new anchor message (not as a "busy" gate). */
  onLaunched?: (jobId: string, messageId: string) => void;
  /// Fired once the server accepted a picture attached from here, so the
  /// discussion adds it to its own inventory. Absent, the launcher offers no
  /// attachment: a surface that cannot show the new file must not create it.
  onImageAttached?: (file: ContextFile) => void;
  /** A capability-checked viewer action can choose the matching configured
   * slot and keep its current image attached while this form loads again. */
  initialSlotKey?: string;
  initialReference?: { assetId: string; mode: MediaReferenceMode } | null;
}) {
  const slots = useMemo(() => slotsOf(connections), [connections]);
  const [selectedKey, setSelectedKey] = useState<string>('');
  const [prompt, setPrompt] = useState('');
  // What the operator picked, which the model in front of them may not accept.
  // `null` means "never chose", so the model's own default applies.
  const [storedDuration, setStoredDuration] = useState<number | null>(null);
  const [storedResolution, setStoredResolution] = useState<string | null>(null);
  const [storedRatio, setStoredRatio] = useState<string | null>(null);
  // Checked by default because that is what the providers already do when
  // nothing is sent. The box makes an existing behaviour visible and
  // switchable — a soundtrack nobody asked for is what got a generation
  // rejected for copyright, with no clue in the request that it existed.
  const [generateAudio, setGenerateAudio] = useState(true);
  // A viewer handoff is an explicit request to generate FROM a picture.  Keep
  // the capability request's lifecycle separately from its value: `null` can
  // mean either a provider that advertised no envelope or a request that has
  // not answered yet, and those two states must never have the same billing
  // behaviour.
  const [capabilityResult, setCapabilityResult] = useState<{
    key: string;
    status: 'loading' | 'resolved' | 'error';
    capabilities: MediaModelCapabilities | null;
  }>({ key: '', status: 'loading', capabilities: null });
  // The pictures this generation starts from, chosen among the room's own
  // assets. Ids, never paths: the browser never learns where a file lives, and
  // the backend re-checks that each one belongs to this discussion. A clip
  // takes exactly one — a frame is one end of it; an illustration takes as
  // many references as its model advertises.
  // What the operator attached. The model in front of them may take fewer
  // pictures, or none — `reference` below is the part that actually applies.
  const [storedReference, setReference] =
    useState<{ assetIds: string[]; mode: MediaReferenceMode } | null>(() => (
      initialReference ? { assetIds: [initialReference.assetId], mode: initialReference.mode } : null
    ));
  /// Keyed by the picture it belongs to: showing the previous thumbnail while
  /// the new blob loads would be a lie about what is attached. The width is
  /// null while unknown — a picture that could not be measured never blocks,
  /// since refusing on an unknown would hide a source the provider accepts.
  const [thumbnail, setThumbnail] =
    useState<{ assetId: string; url: string | null; width: number | null } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attaching, setAttaching] = useState(false);
  const [launched, setLaunched] = useState<{ model: string } | null>(null);
  // Keyed by the model and duration it prices: a figure carried over from the
  // previous model would be a wrong number on a paid click.
  const [fetchedEstimate, setFetchedEstimate] =
    useState<{ key: string; usd: number | null; samples: number } | null>(null);
  // A transport failure is ambiguous: the backend may have committed the job
  // even though its response never reached the browser. Reuse the same key
  // only for an identical retry, so it can never schedule a second charge.
  const pendingLaunchRef = useRef<{ signature: string; key: string } | null>(null);
  // A generation error is not a reason to create another room. Keep the
  // successful creation independently from the billable request's receipt.
  const createdDiscussionIdRef = useRef<string | null>(null);
  // React state does not change inside the same event turn. This ref closes
  // the gap where two synchronous submit clicks would otherwise bill twice.
  const submissionInFlightRef = useRef(false);
  const formRef = useRef<HTMLFormElement | null>(null);

  // KT-587 — the stored key can name a slot that no longer exists: the
  // catalogue reloads, a connection goes away. Resolving it at render falls
  // back to the first slot in the same pass, where an effect repaired it one
  // render late and made the form flicker through an invalid state.
  const selected = slots.find(slot => slot.key === selectedKey)
    ?? slots.find(slot => slot.key === initialSlotKey)
    ?? slots[0]
    ?? null;
  const activeKey = selected?.key ?? '';
  // The endpoint is addressed by connection and modality, but its answer
  // describes the configured model. Include that model in the identity so a
  // response for the previous configured model cannot authorise a new one.
  const capabilityKey = selected
    ? `${selected.connectionId}:${selected.modality}:${selected.model}`
    : '';
  const isVideo = selected?.modality === 'video';

  // What this exact model accepts. Absent (an unreachable or catalogue-less
  // provider) keeps the fallback lists: an empty form would be worse than a
  // slightly wrong one, because it offers nothing at all.
  useEffect(() => {
    if (!selected) return;
    let cancelled = false;
    const requestKey = `${selected.connectionId}:${selected.modality}:${selected.model}`;
    media
      .capabilities(selected.connectionId, selected.modality)
      .then(result => {
        if (cancelled) return;
        setCapabilityResult({ key: requestKey, status: 'resolved', capabilities: result.capabilities });
      })
      .catch(() => {
        if (cancelled) return;
        setCapabilityResult({ key: requestKey, status: 'error', capabilities: null });
      });
    return () => { cancelled = true; };
  }, [selected]);

  // KT-587 — derived rather than cleared inside the effect. Deliberately NOT
  // keyed to the current slot: the previous model's limits stand until the new
  // ones arrive, because treating the gap as "no limits known" makes the trim
  // below drop every attached picture for one render, and that is not
  // recoverable.
  const capabilitiesResolvedForSelected = capabilityResult.key === capabilityKey
    && capabilityResult.status === 'resolved';
  const capabilities = capabilitiesResolvedForSelected ? capabilityResult.capabilities : null;

  const durations = capabilities?.durations_secs?.length
    ? capabilities.durations_secs
    : FALLBACK_DURATIONS;
  const resolutions = capabilities?.resolutions?.length
    ? capabilities.resolutions
    : FALLBACK_RESOLUTIONS;
  const ratios = capabilities?.aspect_ratios?.length
    ? capabilities.aspect_ratios
    : FALLBACK_RATIOS;
  // A soundtrack switch only exists for a model that names one. Absent
  // capabilities keep the box for a video, which is the pre-catalogue
  // behaviour and stays truthful: the provider does generate audio.
  const offersAudio = isVideo && capabilities?.generate_audio !== false;

  // Only what this model advertises. 15 of the 28 video models take both a
  // first and a last frame, 9 only a first, and 4 none at all — so an empty
  // list means the picker does not appear, not that it defaults to something.
  // Memoised so the effects below can depend on it honestly. Rebuilt inline it
  // was a new array every render, which is why one of them had to leave it out
  // of its dependencies and say so in a comment.
  const framePositions = useMemo(
    () => (isVideo ? (capabilities?.frame_positions ?? []) : []),
    [isVideo, capabilities],
  );
  // How many pictures an IMAGE model takes as references. The catalogue states
  // it per model and it ranges from 1 to 16, so a fixed number would be wrong
  // for nearly every provider. Absent means the model advertises none.
  const maxReferences = isVideo ? 0 : (capabilities?.max_input_references ?? 0);
  const referenceLimit = isVideo ? 1 : maxReferences;
  // An empty room still shows the picker when a picture can be attached from
  // here: hiding it made attaching the FIRST one impossible, which is exactly
  // the case a fresh discussion is in.
  const canReference =
    (isVideo ? framePositions.length > 0 : maxReferences > 0)
    && (images.length > 0 || !!onImageAttached);
  // KT-587 — normalised at render, where an effect used to rewrite the state
  // one render late. That render is the one a fast click submits, and it would
  // have carried a picture the model refuses — after billing started.
  const reference = useMemo(() => {
    if (!storedReference || !canReference) return null;
    if (isVideo) {
      return framePositions.includes(storedReference.mode as MediaFramePosition)
        ? storedReference
        : null;
    }
    if (storedReference.mode !== 'reference') return null;
    // Trimmed rather than dropped: the pictures already chosen stay, and only
    // what the new model cannot take goes.
    return storedReference.assetIds.length > referenceLimit
      ? { ...storedReference, assetIds: storedReference.assetIds.slice(0, referenceLimit) }
      : storedReference;
  }, [storedReference, canReference, isVideo, framePositions, referenceLimit]);
  // Do not silently downgrade a viewer handoff to text-to-media.  The stored
  // source remains available for an explicit removal, but a click cannot
  // reach the paid endpoint until this selected model has accepted it.
  const referenceIntentUnresolved = !!storedReference
    && (!capabilitiesResolvedForSelected || !reference);
  const pickedIds = useMemo(() => reference?.assetIds ?? [], [reference]);
  const canPickMore = pickedIds.length < referenceLimit;
  const offeredImages = useMemo(
    () => images.filter(image => !pickedIds.includes(image.id)),
    [images, pickedIds],
  );

  // Thumbnail of the chosen picture: what the operator picked must be visible
  // before paying for a clip built on it.
  const firstAssetId = reference?.assetIds[0] ?? null;
  useEffect(() => {
    if (!firstAssetId) return;
    let objectUrl: string | null = null;
    let cancelled = false;
    discussionsApi.contextFileBlob(discussionId ?? '', firstAssetId)
      .then(async (blob: Blob) => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(blob);
        // Measured from the bytes already fetched for the thumbnail, so the
        // warning appears with the picture rather than after a failed launch.
        const width = await measureWidth(blob);
        if (!cancelled) setThumbnail({ assetId: firstAssetId, url: objectUrl, width });
      })
      .catch(() => {
        if (!cancelled) setThumbnail({ assetId: firstAssetId, url: null, width: null });
      });
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [discussionId, firstAssetId]);

  // Thumbnails of the room's pictures, so the list shows what each name means.
  // Loaded once per set of images and revoked together: a name alone makes the
  // reader open every entry to find the one they meant.
  const [thumbnails, setThumbnails] = useState<Record<string, string>>({});
  const imageKey = images.map(image => image.id).join(',');
  useEffect(() => {
    if (!canReference || images.length === 0) return;
    let cancelled = false;
    const created: string[] = [];
    Promise.all(
      images.map(image =>
        discussionsApi.contextFileBlob(discussionId ?? '', image.id)
          .then((blob: Blob) => {
            if (cancelled) return null;
            const url = URL.createObjectURL(blob);
            created.push(url);
            return [image.id, url] as const;
          })
          // A thumbnail that failed to load costs a picture in the list, never
          // the entry itself: the name still selects it.
          .catch(() => null),
      ),
    ).then(entries => {
      if (cancelled) return;
      setThumbnails(Object.fromEntries(entries.filter(Boolean) as (readonly [string, string])[]));
    });
    return () => {
      cancelled = true;
      setThumbnails({});
      created.forEach(url => URL.revokeObjectURL(url));
    };
    // Keyed on the ids, never on the array: the parent rebuilds
    // `files.filter(isImage)` on every render, so depending on the reference
    // re-ran this effect in a loop — revoking and recreating every object URL,
    // which is what made the open list flicker every few hundred milliseconds.
    // `imageKey` IS the content of `images`; adding the array back would
    // restore the loop, so the exception is declared rather than suffered.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canReference, discussionId, imageKey]);

  const imageOptions: SearchableSelectOption[] = useMemo(
    () => offeredImages.map(image => ({
      value: image.id,
      label: image.filename,
      visual: thumbnails[image.id]
        ? <img src={thumbnails[image.id]} alt="" />
        : undefined,
    })),
    [offeredImages, thumbnails],
  );

  const ratioOptions: DropdownOption[] = useMemo(
    () => ratios.map(ratio => ({
      value: ratio,
      label: ratio,
      visual: RATIO_SHAPES.has(ratio)
        ? <i className="media-generate-ratio-shape" data-ratio={ratio} />
        : undefined,
    })),
    [ratios],
  );

  const pickImage = useCallback((assetId: string) => {
    setReference(current => {
      const mode: MediaReferenceMode = isVideo ? framePositions[0] : 'reference';
      if (!current) return { assetIds: [assetId], mode };
      if (current.assetIds.includes(assetId)) return current;
      if (current.assetIds.length >= referenceLimit) return current;
      return { ...current, assetIds: [...current.assetIds, assetId] };
    });
  }, [framePositions, isVideo, referenceLimit]);

  const attachImage = useCallback(async (file: File) => {
    setAttaching(true);
    setError(null);
    try {
      const uploaded = await discussionsApi.uploadContextFile(discussionId ?? '', file);
      onImageAttached?.(uploaded.file);
      // Picked right away: attaching one here is asking to use it, and making
      // the operator find it again in the list would be the extra step this
      // control exists to remove.
      pickImage(uploaded.file.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setAttaching(false);
    }
  }, [discussionId, onImageAttached, pickImage]);

  const dropImage = useCallback((assetId: string) => {
    setReference(current => {
      if (!current) return null;
      const assetIds = current.assetIds.filter(id => id !== assetId);
      return assetIds.length ? { ...current, assetIds } : null;
    });
  }, []);

  // KT-587 — a choice the provider does not accept must not survive a model
  // change: it would be submitted as-is and refused after billing started.
  // Resolved at render rather than repaired by an effect, so the invalid value
  // never exists — an effect fixed it one render late, and that render is the
  // one a fast click submits.
  const pick = <T,>(stored: T | null, offered: T[], preferred: T): T =>
    (stored !== null && offered.includes(stored))
      ? stored
      : (offered.includes(preferred) ? preferred : offered[0]);
  const durationSecs = pick(storedDuration, durations, DEFAULT_DURATION);
  const resolution = pick(storedResolution, resolutions, DEFAULT_RESOLUTION);
  const aspectRatio = pick(storedRatio, ratios, DEFAULT_RATIO);

  // Price of the click, derived from what this model was actually billed
  // before. Absent on a first run, and said so rather than shown as free.
  const estimateKey = selected ? `${selected.key}:${durationSecs}` : '';
  useEffect(() => {
    if (!selected) return;
    let cancelled = false;
    media
      .estimate(
        selected.connectionId,
        selected.modality,
        selected.modality === 'video' ? durationSecs : undefined,
      )
      .then(result => {
        if (!cancelled) {
          setFetchedEstimate({
            key: estimateKey,
            usd: result.estimated_usd ?? null,
            samples: result.samples,
          });
        }
      })
      .catch(() => {
        if (!cancelled) setFetchedEstimate({ key: estimateKey, usd: null, samples: 0 });
      });
    return () => {
      cancelled = true;
    };
  }, [selected, durationSecs, estimateKey]);

  // Only the figure priced for what is on screen. Anything else reads as
  // unknown, which the form already says rather than showing a zero.
  const estimate = fetchedEstimate?.key === estimateKey ? fetchedEstimate : null;

  const referenceUrl = thumbnail?.assetId === firstAssetId ? thumbnail.url : null;
  const referenceWidth = thumbnail?.assetId === firstAssetId ? thumbnail.width : null;
  const referenceTooNarrow = referenceWidth !== null && referenceWidth < MIN_REFERENCE_WIDTH_PX;

  const initialReferenceKey = initialReference
    ? `${initialReference.assetId}:${initialReference.mode}`
    : '';
  useEffect(() => {
    if (initialReferenceKey) formRef.current?.focus();
  }, [initialReferenceKey]);

  const submit = useCallback(async () => {
    if (!selected || !prompt.trim() || busy || referenceTooNarrow || referenceIntentUnresolved || submissionInFlightRef.current) return;
    submissionInFlightRef.current = true;
    setBusy(true);
    setError(null);
    let targetDiscussionId = discussionId ?? createdDiscussionIdRef.current;
    try {
      if (!targetDiscussionId) {
        if (!onCreateDiscussion) throw new Error('A discussion is required to generate media.');
        targetDiscussionId = await onCreateDiscussion();
        createdDiscussionIdRef.current = targetDiscussionId;
        onDiscussionCreated?.(targetDiscussionId);
      }
    const signature = JSON.stringify({
      connectionId: selected.connectionId,
      modality: selected.modality,
      prompt: prompt.trim(),
      discussionId: targetDiscussionId,
      aspectRatio,
      durationSecs: selected.modality === 'video' ? durationSecs : null,
      resolution: selected.modality === 'video' ? resolution : null,
      generateAudio: selected.modality === 'video' ? generateAudio : null,
      reference: reference ? `${reference.assetIds.join(',')}:${reference.mode}` : null,
    });
    if (pendingLaunchRef.current?.signature !== signature) {
      pendingLaunchRef.current = { signature, key: crypto.randomUUID() };
    }
      const job = await media.generate({
        idempotency_key: pendingLaunchRef.current.key,
        connection_id: selected.connectionId,
        modality: selected.modality,
        prompt: prompt.trim(),
        discussion_id: targetDiscussionId,
        aspect_ratio: aspectRatio,
        ...(selected.modality === 'video'
          // Sent explicitly, including the default: an absent field leaves the
          // provider's own default in charge, which is exactly the silence
          // this box exists to remove.
          ? { duration_secs: durationSecs, resolution, generate_audio: generateAudio }
          : {}),
        ...(reference
          ? { reference_asset_ids: reference.assetIds, reference_mode: reference.mode }
          : {}),
      });
      setLaunched({ model: job.model });
      setPrompt('');
      setReference(null);
      pendingLaunchRef.current = null;
      onLaunched?.(job.job_id, job.message_id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      submissionInFlightRef.current = false;
      setBusy(false);
    }
  }, [aspectRatio, busy, discussionId, durationSecs, generateAudio, onCreateDiscussion, onDiscussionCreated, onLaunched, prompt, reference, referenceIntentUnresolved, referenceTooNarrow, resolution, selected]);

  if (slots.length === 0) {
    return (
      <p className="media-generate-empty" data-testid="media-generate-empty">
        {t('disc.media.noSlot')}
      </p>
    );
  }

  return (
    <form
      ref={formRef}
      className="media-generate-form"
      data-testid="media-generate-form"
      tabIndex={-1}
      onSubmit={event => {
        event.preventDefault();
        void submit();
      }}
    >
      {/* Configured models, presented like the connection card's AI-mode
          slots: the choice IS the modality. */}
      <div className="media-generate-slots" role="radiogroup" aria-label={t('disc.media.model')}>
        {slots.map(slot => {
          const Icon = slot.modality === 'image' ? ImageIcon : Clapperboard;
          return (
            <button
              key={slot.key}
              type="button"
              role="radio"
              aria-checked={slot.key === activeKey}
              data-active={slot.key === activeKey}
              className="media-generate-slot"
              onClick={() => setSelectedKey(slot.key)}
              data-testid={`media-slot-${slot.key}`}
            >
              <span className="media-generate-slot-head">
                <Icon size={12} aria-hidden="true" />
                <span>{t(`disc.media.modality.${slot.modality}`)}</span>
              </span>
              <span className="media-generate-slot-model" title={slot.model}>{slot.model}</span>
              <span className="media-generate-slot-conn">{slot.connectionName}</span>
            </button>
          );
        })}
      </div>

      <label className="media-generate-field">
        <span>{t('disc.media.prompt')}</span>
        <textarea
          value={prompt}
          onChange={event => setPrompt(event.target.value)}
          rows={3}
          placeholder={t('disc.media.promptPlaceholder')}
        />
      </label>

      {isVideo && (
        <div className="media-generate-row">
          <label className="media-generate-field">
            <span>{t('disc.media.duration')}</span>
            <select
              value={durationSecs}
              onChange={event => setStoredDuration(Number(event.target.value))}
            >
              {durations.map(value => (
                <option key={value} value={value}>{t('disc.media.seconds', value)}</option>
              ))}
            </select>
          </label>
          <label className="media-generate-field">
            <span>{t('disc.media.resolution')}</span>
            <select value={resolution} onChange={event => setStoredResolution(event.target.value)}>
              {resolutions.map(value => <option key={value} value={value}>{value}</option>)}
            </select>
          </label>
        </div>
      )}

      {offersAudio && (
        <label className="media-generate-check">
          <input
            type="checkbox"
            checked={generateAudio}
            onChange={event => setGenerateAudio(event.target.checked)}
            data-testid="media-generate-audio"
          />
          <span>{t('disc.media.generateAudio')}</span>
        </label>
      )}

      {referenceIntentUnresolved && (
        <fieldset className="media-generate-reference" data-testid="media-reference-pending">
          <legend>{isVideo ? t('disc.media.sourceImage') : t('disc.media.referenceImages')}</legend>
          <p className="media-generate-reference-warning" role="alert">
            {t('disc.media.referenceCompatibilityPending')}
          </p>
          <p>{storedReference?.assetIds.map(assetId => images.find(image => image.id === assetId)?.filename ?? assetId).join(', ')}</p>
          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={() => setReference(null)}
            aria-label={t('disc.media.clearSourceImage')}
            data-testid="media-reference-clear"
          >
            <X size={13} aria-hidden="true" />
          </button>
        </fieldset>
      )}

      {canReference && !referenceIntentUnresolved && (
        <fieldset className="media-generate-reference" data-testid="media-reference-picker">
          <legend>{isVideo ? t('disc.media.sourceImage') : t('disc.media.referenceImages')}</legend>
          {reference && (
            <div className="media-generate-reference-picked">
              {referenceUrl && <img src={referenceUrl} alt="" />}
              {isVideo && (
                <div className="media-generate-reference-modes" role="radiogroup">
                  {framePositions.map(position => (
                    <button
                      key={position}
                      type="button"
                      role="radio"
                      aria-checked={reference.mode === position}
                      data-active={reference.mode === position}
                      className="media-generate-reference-mode"
                      onClick={() => setReference({ ...reference, mode: position })}
                      data-testid={`media-reference-mode-${position}`}
                    >
                      {t(`disc.media.frame.${position}`)}
                    </button>
                  ))}
                </div>
              )}
              {!isVideo && (
                // Named one by one: which pictures were chosen, and in which
                // order — providers weigh references by position.
                <ul className="media-generate-reference-list">
                  {reference.assetIds.map((assetId, index) => (
                    <li key={assetId}>
                      <span>{index + 1}. {images.find(image => image.id === assetId)?.filename ?? assetId}</span>
                      <button
                        type="button"
                        className="btn btn-sm btn-ghost"
                        onClick={() => dropImage(assetId)}
                        aria-label={t('disc.media.clearSourceImage')}
                        data-testid={`media-reference-drop-${assetId}`}
                      >
                        <X size={12} aria-hidden="true" />
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              <button
                type="button"
                className="btn btn-sm btn-ghost"
                onClick={() => setReference(null)}
                aria-label={t('disc.media.clearSourceImage')}
                data-testid="media-reference-clear"
              >
                <X size={13} aria-hidden="true" />
              </button>
              {referenceTooNarrow && (
                <p className="media-generate-reference-warning" role="alert" data-testid="media-reference-too-narrow">
                  {t('disc.media.referenceTooNarrow', referenceWidth ?? 0, MIN_REFERENCE_WIDTH_PX)}
                </p>
              )}
            </div>
          )}
          {canPickMore ? (
            <div className="media-generate-reference-tools">
              {/* One row instead of a carpet of buttons: the list shows each
                  picture before its name, so the right one is recognised
                  rather than guessed from a filename. */}
              <SearchableSelect
                value=""
                options={imageOptions}
                onChange={pickImage}
                label={t('disc.media.sourceImage')}
                placeholder={t('disc.media.pickSourceImage')}
                emptyLabel={t('disc.media.noSourceImageMatch')}
                clearable={false}
                className="media-generate-reference-select"
                testId="media-reference-select"
              />
              {onImageAttached && (
                <label
                  className="btn btn-sm btn-ghost media-generate-attach"
                  data-testid="media-reference-attach"
                  title={attaching ? t('disc.media.attaching') : t('disc.media.attachImage')}
                  aria-label={attaching ? t('disc.media.attaching') : t('disc.media.attachImage')}
                >
                  {attaching
                    ? <Loader2 size={14} aria-hidden="true" className="spin" />
                    : <Paperclip size={14} aria-hidden="true" />}
                  <input
                    type="file"
                    accept="image/*"
                    hidden
                    disabled={attaching}
                    onChange={event => {
                      const file = event.target.files?.[0];
                      // Cleared before the upload: the same picture must be
                      // attachable twice in a row.
                      event.target.value = '';
                      if (file) void attachImage(file);
                    }}
                  />
                </label>
              )}
            </div>
          ) : (
            // The ceiling is the model's own, said out loud: a picker that
            // simply stopped responding would read as broken.
            !isVideo && (
              <p className="set-hint" data-testid="media-reference-limit">
                {t('disc.media.referenceLimitReached', referenceLimit)}
              </p>
            )
          )}
        </fieldset>
      )}

      {/* Each ratio carries a box in its own proportions — the same trick the
          live-page mosaic uses for layouts. */}
      <label className="media-generate-field">
        <span>{t('disc.media.aspectRatio')}</span>
        {/* Each entry carries its own proportions, so a shape is recognised
            rather than decoded from `4:3`. */}
        <Dropdown
          value={aspectRatio}
          options={ratioOptions}
          onChange={setStoredRatio}
          ariaLabel={t('disc.media.aspectRatio')}
          className="media-generate-ratio-select"
          testId="media-ratio-select"
        />
      </label>

      <p className="media-generate-estimate">
        {estimate && estimate.usd !== null && estimate.samples > 0
          ? t('disc.media.estimate', estimate.usd.toFixed(4), estimate.samples)
          : t('disc.media.estimateUnknown')}
      </p>

      {error && <p className="media-generate-error" role="alert">{error}</p>}
      {launched && (
        <p className="media-generate-launched" role="status">
          {t('disc.media.launched', launched.model)}
        </p>
      )}

      <button type="submit" className="btn btn-sm" disabled={busy || !prompt.trim() || referenceTooNarrow || referenceIntentUnresolved}>
        {busy
          ? <Loader2 size={13} aria-hidden="true" className="spin" />
          : <Sparkles size={13} aria-hidden="true" />}
        <span>{t('disc.media.generate')}</span>
      </button>
    </form>
  );
}
