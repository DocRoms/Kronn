// KT-540 — launch a generation from the discussion that will hold the asset.
//
// There is no modality switch: the operator picks a CONFIGURED model, exactly
// as the connection card offers its Economy/Standard/Advanced slots, and the
// modality follows from that choice. Asking for a modality first meant asking a
// question the configuration already answers — and offering a modality nobody
// had configured.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { Clapperboard, Image as ImageIcon, Loader2, Sparkles } from 'lucide-react';
import { media } from '../lib/api';
import type { ExternalApiConnectionView, MediaModality } from '../lib/api';
import './MediaGenerateForm.css';

type T = (key: string, ...args: (string | number)[]) => string;

const DURATIONS = [3, 5, 8];
const RESOLUTIONS = ['480p', '720p', '1080p'];
/// Ratios shown with a proportional preview, like the live-page mosaic layouts:
/// `4:3` means nothing to most people until they see the shape.
const RATIOS = ['16:9', '4:3', '1:1', '9:16'] as const;

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
  t,
  onLaunched,
}: {
  discussionId: string;
  connections: ExternalApiConnectionView[];
  t: T;
  onLaunched?: (jobId: string) => void;
}) {
  const slots = useMemo(() => slotsOf(connections), [connections]);
  const [selectedKey, setSelectedKey] = useState<string>('');
  const [prompt, setPrompt] = useState('');
  const [durationSecs, setDurationSecs] = useState(5);
  const [resolution, setResolution] = useState('480p');
  const [aspectRatio, setAspectRatio] = useState<string>('16:9');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [launched, setLaunched] = useState<{ model: string } | null>(null);
  const [estimate, setEstimate] = useState<{ usd: number | null; samples: number } | null>(null);

  useEffect(() => {
    if (slots.length === 0) {
      setSelectedKey('');
      return;
    }
    if (!slots.some(slot => slot.key === selectedKey)) setSelectedKey(slots[0].key);
  }, [slots, selectedKey]);

  const selected = slots.find(slot => slot.key === selectedKey) ?? null;
  const isVideo = selected?.modality === 'video';

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

  const submit = useCallback(async () => {
    if (!selected || !prompt.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const job = await media.generate({
        connection_id: selected.connectionId,
        modality: selected.modality,
        prompt: prompt.trim(),
        discussion_id: discussionId,
        aspect_ratio: aspectRatio,
        ...(selected.modality === 'video'
          ? { duration_secs: durationSecs, resolution }
          : {}),
      });
      setLaunched({ model: job.model });
      setPrompt('');
      onLaunched?.(job.job_id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [aspectRatio, busy, discussionId, durationSecs, onLaunched, prompt, resolution, selected]);

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
              {DURATIONS.map(value => (
                <option key={value} value={value}>{t('disc.media.seconds', value)}</option>
              ))}
            </select>
          </label>
          <label className="media-generate-field">
            <span>{t('disc.media.resolution')}</span>
            <select value={resolution} onChange={event => setResolution(event.target.value)}>
              {RESOLUTIONS.map(value => <option key={value} value={value}>{value}</option>)}
            </select>
          </label>
        </div>
      )}

      {/* Each ratio carries a box in its own proportions — the same trick the
          live-page mosaic uses for layouts. */}
      <fieldset className="media-generate-ratios">
        <legend>{t('disc.media.aspectRatio')}</legend>
        <div className="media-generate-ratio-choices">
          {RATIOS.map(ratio => (
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
              <i className="media-generate-ratio-shape" data-ratio={ratio} aria-hidden="true" />
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

      <button type="submit" className="btn btn-sm" disabled={busy || !prompt.trim()}>
        {busy
          ? <Loader2 size={13} aria-hidden="true" className="spin" />
          : <Sparkles size={13} aria-hidden="true" />}
        <span>{t('disc.media.generate')}</span>
      </button>
    </form>
  );
}
