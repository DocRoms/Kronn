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

/// Measured against OpenRouter on 02/09: a 8x8 source came back
/// `400 InvalidParameter — expected the width to be at least 300px`. The
/// refusal is synchronous and unbilled, but it costs the operator a launch
/// that could only fail, so the picture is measured here instead.
const MIN_REFERENCE_WIDTH_PX = 300;

/// Known shapes, so a ratio the provider names can still be drawn. An unknown
/// one is listed without a preview rather than dropped: the provider accepts
/// it, so the operator must be able to pick it.
const RATIO_SHAPES = new Set(['16:9', '4:3', '1:1', '9:16', '3:4', '21:9', '9:21', '2:3', '3:2', '4:5', '5:4']);

/// Below this, the whole list fits on screen and a search field is one more
/// control for nothing.
const SEARCHABLE_FROM = 5;

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
  connections,
  images = [],
  t,
  onLaunched,
  onImageAttached,
}: {
  discussionId: string;
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
}) {
  const slots = useMemo(() => slotsOf(connections), [connections]);
  const [selectedKey, setSelectedKey] = useState<string>('');
  const [prompt, setPrompt] = useState('');
  const [durationSecs, setDurationSecs] = useState(5);
  const [resolution, setResolution] = useState('480p');
  const [aspectRatio, setAspectRatio] = useState<string>('16:9');
  // Checked by default because that is what the providers already do when
  // nothing is sent. The box makes an existing behaviour visible and
  // switchable — a soundtrack nobody asked for is what got a generation
  // rejected for copyright, with no clue in the request that it existed.
  const [generateAudio, setGenerateAudio] = useState(true);
  const [capabilities, setCapabilities] = useState<MediaModelCapabilities | null>(null);
  // The pictures this generation starts from, chosen among the room's own
  // assets. Ids, never paths: the browser never learns where a file lives, and
  // the backend re-checks that each one belongs to this discussion. A clip
  // takes exactly one — a frame is one end of it; an illustration takes as
  // many references as its model advertises.
  const [reference, setReference] = useState<{ assetIds: string[]; mode: MediaReferenceMode } | null>(null);
  const [referenceUrl, setReferenceUrl] = useState<string | null>(null);
  /// Width of the chosen picture, or null while unknown. A picture that could
  /// not be measured never blocks: refusing on an unknown would hide a source
  /// the provider would have accepted.
  const [referenceWidth, setReferenceWidth] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /// Narrows the pictures offered. A room with fifty assets makes the right
  /// one unreachable by scrolling alone.
  const [imageQuery, setImageQuery] = useState('');
  const [attaching, setAttaching] = useState(false);
  const [launched, setLaunched] = useState<{ model: string } | null>(null);
  const [estimate, setEstimate] = useState<{ usd: number | null; samples: number } | null>(null);
  // A transport failure is ambiguous: the backend may have committed the job
  // even though its response never reached the browser. Reuse the same key
  // only for an identical retry, so it can never schedule a second charge.
  const pendingLaunchRef = useRef<{ signature: string; key: string } | null>(null);

  useEffect(() => {
    if (slots.length === 0) {
      setSelectedKey('');
      return;
    }
    if (!slots.some(slot => slot.key === selectedKey)) setSelectedKey(slots[0].key);
  }, [slots, selectedKey]);

  const selected = slots.find(slot => slot.key === selectedKey) ?? null;
  const isVideo = selected?.modality === 'video';

  // What this exact model accepts. Absent (an unreachable or catalogue-less
  // provider) keeps the fallback lists: an empty form would be worse than a
  // slightly wrong one, because it offers nothing at all.
  useEffect(() => {
    if (!selected) {
      setCapabilities(null);
      return;
    }
    let cancelled = false;
    media
      .capabilities(selected.connectionId, selected.modality)
      .then(result => { if (!cancelled) setCapabilities(result.capabilities); })
      .catch(() => { if (!cancelled) setCapabilities(null); });
    return () => { cancelled = true; };
  }, [selected]);

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
  const framePositions = isVideo ? (capabilities?.frame_positions ?? []) : [];
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
  const pickedIds = reference?.assetIds ?? [];
  const canPickMore = pickedIds.length < referenceLimit;
  const offeredImages = useMemo(() => {
    const needle = imageQuery.trim().toLocaleLowerCase();
    return images.filter(
      image =>
        !pickedIds.includes(image.id)
        && (!needle || image.filename.toLocaleLowerCase().includes(needle)),
    );
  }, [images, imageQuery, pickedIds]);

  // A source image the newly selected model cannot take must be dropped, not
  // carried into a submission it would fail.
  useEffect(() => {
    setReference(current => {
      if (!current) return null;
      if (!canReference) return null;
      if (isVideo) return framePositions.includes(current.mode as MediaFramePosition) ? current : null;
      if (current.mode !== 'reference') return null;
      // Trimmed rather than dropped: the pictures already chosen stay, and
      // only what the new model cannot take goes.
      return current.assetIds.length > referenceLimit
        ? { ...current, assetIds: current.assetIds.slice(0, referenceLimit) }
        : current;
    });
    // `framePositions` is rebuilt on every render, so it cannot gate this.
  }, [canReference, isVideo, referenceLimit, capabilities]);

  // Thumbnail of the chosen picture: what the operator picked must be visible
  // before paying for a clip built on it.
  useEffect(() => {
    const firstAssetId = reference?.assetIds[0];
    if (!firstAssetId) {
      setReferenceUrl(null);
      setReferenceWidth(null);
      return;
    }
    let objectUrl: string | null = null;
    let cancelled = false;
    discussionsApi.contextFileBlob(discussionId, firstAssetId)
      .then(async (blob: Blob) => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(blob);
        setReferenceUrl(objectUrl);
        // Measured from the bytes already fetched for the thumbnail, so the
        // warning appears with the picture rather than after a failed launch.
        try {
          const bitmap = await createImageBitmap(blob);
          if (!cancelled) setReferenceWidth(bitmap.width);
          bitmap.close();
        } catch {
          if (!cancelled) setReferenceWidth(null);
        }
      })
      .catch(() => { if (!cancelled) { setReferenceUrl(null); setReferenceWidth(null); } });
    return () => {
      cancelled = true;
      setReferenceUrl(null);
      setReferenceWidth(null);
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [discussionId, reference]);

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
      const uploaded = await discussionsApi.uploadContextFile(discussionId, file);
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

  // A choice the provider does not accept must not survive a model change: it
  // would be submitted as-is and refused after billing started.
  useEffect(() => {
    if (!durations.includes(durationSecs)) setDurationSecs(durations[0]);
  }, [durationSecs, durations]);
  useEffect(() => {
    if (!resolutions.includes(resolution)) setResolution(resolutions[0]);
  }, [resolution, resolutions]);
  useEffect(() => {
    if (!ratios.includes(aspectRatio)) setAspectRatio(ratios[0]);
  }, [aspectRatio, ratios]);

  // Price of the click, derived from what this model was actually billed
  // before. Absent on a first run, and said so rather than shown as free.
  useEffect(() => {
    if (!selected) {
      setEstimate(null);
      return;
    }
    let cancelled = false;
    media
      .estimate(
        selected.connectionId,
        selected.modality,
        selected.modality === 'video' ? durationSecs : undefined,
      )
      .then(result => {
        if (!cancelled) {
          setEstimate({ usd: result.estimated_usd ?? null, samples: result.samples });
        }
      })
      .catch(() => {
        if (!cancelled) setEstimate(null);
      });
    return () => {
      cancelled = true;
    };
  }, [selected, durationSecs]);

  const referenceTooNarrow = referenceWidth !== null && referenceWidth < MIN_REFERENCE_WIDTH_PX;

  const submit = useCallback(async () => {
    if (!selected || !prompt.trim() || busy || referenceTooNarrow) return;
    const signature = JSON.stringify({
      connectionId: selected.connectionId,
      modality: selected.modality,
      prompt: prompt.trim(),
      discussionId,
      aspectRatio,
      durationSecs: selected.modality === 'video' ? durationSecs : null,
      resolution: selected.modality === 'video' ? resolution : null,
      generateAudio: selected.modality === 'video' ? generateAudio : null,
      reference: reference ? `${reference.assetIds.join(',')}:${reference.mode}` : null,
    });
    if (pendingLaunchRef.current?.signature !== signature) {
      pendingLaunchRef.current = { signature, key: crypto.randomUUID() };
    }
    setBusy(true);
    setError(null);
    try {
      const job = await media.generate({
        idempotency_key: pendingLaunchRef.current.key,
        connection_id: selected.connectionId,
        modality: selected.modality,
        prompt: prompt.trim(),
        discussion_id: discussionId,
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
      setBusy(false);
    }
  }, [aspectRatio, busy, discussionId, durationSecs, generateAudio, onLaunched, prompt, reference, referenceTooNarrow, resolution, selected]);

  if (slots.length === 0) {
    return (
      <p className="media-generate-empty" data-testid="media-generate-empty">
        {t('disc.media.noSlot')}
      </p>
    );
  }

  return (
    <form
      className="media-generate-form"
      data-testid="media-generate-form"
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
              aria-checked={slot.key === selectedKey}
              data-active={slot.key === selectedKey}
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
              onChange={event => setDurationSecs(Number(event.target.value))}
            >
              {durations.map(value => (
                <option key={value} value={value}>{t('disc.media.seconds', value)}</option>
              ))}
            </select>
          </label>
          <label className="media-generate-field">
            <span>{t('disc.media.resolution')}</span>
            <select value={resolution} onChange={event => setResolution(event.target.value)}>
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

      {canReference && (
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
            <>
              <div className="media-generate-reference-tools">
                {images.length > SEARCHABLE_FROM && (
                  <input
                    type="search"
                    value={imageQuery}
                    onChange={event => setImageQuery(event.target.value)}
                    placeholder={t('disc.media.searchSourceImage')}
                    aria-label={t('disc.media.searchSourceImage')}
                    data-testid="media-reference-search"
                  />
                )}
                {onImageAttached && (
                  <label className="btn btn-sm btn-ghost" data-testid="media-reference-attach">
                    <Paperclip size={13} aria-hidden="true" />
                    <span>{attaching ? t('disc.media.attaching') : t('disc.media.attachImage')}</span>
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
              <div className="media-generate-reference-choices">
                {offeredImages.map(image => (
                  <button
                    key={image.id}
                    type="button"
                    className="media-generate-reference-choice"
                    onClick={() => pickImage(image.id)}
                    data-testid={`media-reference-pick-${image.id}`}
                    title={image.filename}
                  >
                    {image.filename}
                  </button>
                ))}
                {offeredImages.length === 0 && imageQuery.trim() && (
                  <span className="set-hint" data-testid="media-reference-no-match">
                    {t('disc.media.noSourceImageMatch')}
                  </span>
                )}
              </div>
            </>
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
      <fieldset className="media-generate-ratios">
        <legend>{t('disc.media.aspectRatio')}</legend>
        <div className="media-generate-ratio-choices">
          {ratios.map(ratio => (
            <button
              key={ratio}
              type="button"
              role="radio"
              aria-checked={ratio === aspectRatio}
              aria-label={ratio}
              data-active={ratio === aspectRatio}
              className="media-generate-ratio"
              onClick={() => setAspectRatio(ratio)}
              data-testid={`media-ratio-${ratio}`}
            >
              {RATIO_SHAPES.has(ratio) && (
                <i className="media-generate-ratio-shape" data-ratio={ratio} aria-hidden="true" />
              )}
              <span>{ratio}</span>
            </button>
          ))}
        </div>
      </fieldset>

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

      <button type="submit" className="btn btn-sm" disabled={busy || !prompt.trim() || referenceTooNarrow}>
        {busy
          ? <Loader2 size={13} aria-hidden="true" className="spin" />
          : <Sparkles size={13} aria-hidden="true" />}
        <span>{t('disc.media.generate')}</span>
      </button>
    </form>
  );
}
