// KT-540 — launch a generation from the discussion that will hold the asset.
//
// The model is deliberately NOT a field: it comes from the connection's image
// or video slot, so the UI cannot bill a model the operator never configured.
// What the form does own is the prompt and the shape of the output.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { Clapperboard, Image as ImageIcon, Loader2, Sparkles } from 'lucide-react';
import { media } from '../lib/api';
import type { ExternalApiConnectionView, MediaModality } from '../lib/api';
import './MediaGenerateForm.css';

type T = (key: string, ...args: (string | number)[]) => string;

const DURATIONS = [3, 5, 8];
const RESOLUTIONS = ['480p', '720p', '1080p'];
const RATIOS = ['16:9', '9:16', '1:1'];

function slotFor(connection: ExternalApiConnectionView, modality: MediaModality): string | null {
  const slot = modality === 'image' ? connection.image_model : connection.video_model;
  return slot && slot.trim() ? slot : null;
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
  const [modality, setModality] = useState<MediaModality>('image');
  const [prompt, setPrompt] = useState('');
  const [connectionId, setConnectionId] = useState('');
  const [durationSecs, setDurationSecs] = useState(5);
  const [resolution, setResolution] = useState('480p');
  const [aspectRatio, setAspectRatio] = useState('16:9');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [launched, setLaunched] = useState<{ model: string } | null>(null);
  const [estimate, setEstimate] = useState<{ usd: number | null; samples: number } | null>(null);

  // Only connections that actually have this slot filled: offering the others
  // would produce a refusal the user cannot act on from here.
  const usable = useMemo(
    () => connections.filter(connection => slotFor(connection, modality)),
    [connections, modality],
  );

  useEffect(() => {
    if (usable.length === 0) {
      setConnectionId('');
      return;
    }
    if (!usable.some(connection => connection.id === connectionId)) {
      setConnectionId(usable[0].id);
    }
  }, [usable, connectionId]);

  const selected = usable.find(connection => connection.id === connectionId) ?? null;
  const model = selected ? slotFor(selected, modality) : null;

  // Price of the click, derived from what this model was actually billed
  // before. Absent on a first run, and said so rather than shown as free.
  useEffect(() => {
    if (!connectionId) {
      setEstimate(null);
      return;
    }
    let cancelled = false;
    media
      .estimate(connectionId, modality, modality === 'video' ? durationSecs : undefined)
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
  }, [connectionId, modality, durationSecs]);

  const submit = useCallback(async () => {
    if (!connectionId || !prompt.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const job = await media.generate({
        connection_id: connectionId,
        modality,
        prompt: prompt.trim(),
        discussion_id: discussionId,
        ...(modality === 'video'
          ? { duration_secs: durationSecs, resolution, aspect_ratio: aspectRatio }
          : { aspect_ratio: aspectRatio }),
      });
      setLaunched({ model: job.model });
      setPrompt('');
      onLaunched?.(job.job_id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [aspectRatio, busy, connectionId, discussionId, durationSecs, modality, onLaunched, prompt, resolution]);

  const modalities: Array<{ id: MediaModality; label: string; icon: typeof ImageIcon }> = [
    { id: 'image', label: t('disc.media.modalityImage'), icon: ImageIcon },
    { id: 'video', label: t('disc.media.modalityVideo'), icon: Clapperboard },
  ];

  return (
    <form
      className="media-generate-form"
      data-testid="media-generate-form"
      onSubmit={event => {
        event.preventDefault();
        void submit();
      }}
    >
      <div className="media-generate-modalities" role="group" aria-label={t('disc.media.modality')}>
        {modalities.map(item => {
          const Icon = item.icon;
          return (
            <button
              key={item.id}
              type="button"
              data-active={modality === item.id}
              onClick={() => setModality(item.id)}
            >
              <Icon size={13} aria-hidden="true" />
              <span>{item.label}</span>
            </button>
          );
        })}
      </div>

      {usable.length === 0 ? (
        <p className="media-generate-empty">{t('disc.media.noSlot')}</p>
      ) : (
        <>
          <label className="media-generate-field">
            <span>{t('disc.media.connection')}</span>
            <select
              value={connectionId}
              onChange={event => setConnectionId(event.target.value)}
            >
              {usable.map(connection => (
                <option key={connection.id} value={connection.id}>
                  {connection.display_name}
                </option>
              ))}
            </select>
          </label>

          {model && <p className="media-generate-model">{t('disc.media.modelUsed', model)}</p>}

          <label className="media-generate-field">
            <span>{t('disc.media.prompt')}</span>
            <textarea
              value={prompt}
              onChange={event => setPrompt(event.target.value)}
              rows={3}
              placeholder={t('disc.media.promptPlaceholder')}
            />
          </label>

          {modality === 'video' && (
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

          <label className="media-generate-field">
            <span>{t('disc.media.aspectRatio')}</span>
            <select value={aspectRatio} onChange={event => setAspectRatio(event.target.value)}>
              {RATIOS.map(value => <option key={value} value={value}>{value}</option>)}
            </select>
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

          <button type="submit" className="btn btn-sm" disabled={busy || !prompt.trim()}>
            {busy
              ? <Loader2 size={13} aria-hidden="true" className="spin" />
              : <Sparkles size={13} aria-hidden="true" />}
            <span>{t('disc.media.generate')}</span>
          </button>
        </>
      )}
    </form>
  );
}
