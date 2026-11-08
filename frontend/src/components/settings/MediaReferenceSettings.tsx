import { useEffect, useId, useRef, useState } from 'react';
import { media } from '../../lib/api';
import type { MediaModality, MediaModelCapabilities } from '../../types/generated';

interface Props {
  connectionId: string;
  model: string;
  modality: MediaModality;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function MediaReferenceSettings({ connectionId, model, modality, t }: Props) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const [revision, setRevision] = useState(0);
  const requestKey = JSON.stringify([connectionId, modality, model, revision]);
  const [loaded, setLoaded] = useState<{ key: string; caps: MediaModelCapabilities | null; error: string | null } | null>(null);
  const current = loaded?.key === requestKey ? loaded : null;
  const caps = current?.caps;
  const loading = open && current === null;
  const ready = current !== null && current.error === null;
  const [value, setValue] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void media.capabilities(connectionId, modality).then(result => {
      if (cancelled) return;
      if (result.model !== model) throw new Error(t('media.references.modelChanged'));
      setLoaded({ key: requestKey, caps: result.capabilities, error: null });
      const configured = result.capabilities?.reference_support?.configured_max;
      setValue(configured == null ? '' : String(configured));
    }).catch(cause => {
      if (!cancelled) setLoaded({ key: requestKey, caps: null, error: String(cause instanceof Error ? cause.message : cause) });
    });
    return () => { cancelled = true; };
  }, [open, connectionId, modality, model, requestKey, t]);

  const maximum = value.trim() === '' ? null : Number(value);
  const invalid = maximum !== null && (!Number.isInteger(maximum) || maximum < 0 || maximum > 64);
  const save = async () => {
    if (invalid || loading || !ready || inFlight.current) return;
    inFlight.current = true;
    setSaving(true);
    setError(null);
    try {
      const result = await media.setReferenceLimit({ connection_id: connectionId, model, modality, max_input_references: maximum });
      setLoaded({ key: requestKey, caps: result.capabilities, error: null });
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      inFlight.current = false;
      setSaving(false);
    }
  };

  return (
    <details className="set-ollama-context-section" onToggle={event => setOpen(event.currentTarget.open)}>
      <summary className="set-ollama-context-summary text-xs text-muted">{t('media.references.title', modality === 'image' ? t('comfyui.slot.image') : t('comfyui.slot.video'))}</summary>
      {open && <div className="set-ollama-context-card">
        {loading ? <p role="status">{t('media.references.loading')}</p> : <>
          <p>{caps?.max_input_references == null ? t('media.references.unknown') : t('media.references.effective', caps.max_input_references)}</p>
          {caps?.reference_support && <p className="set-hint">{t(`media.references.source.${caps.reference_support.source}`)}</p>}
          {caps?.reference_support?.blocker && <p role="status" className="set-hint">{caps.reference_support.blocker}</p>}
          <div className="set-ollama-context-editor">
            <label htmlFor={id}>{t('media.references.limit')}</label>
            <input id={id} type="number" min={0} max={64} step={1} value={value}
              placeholder={t('media.references.automatic')} disabled={saving}
              onChange={event => setValue(event.target.value)} />
          </div>
          <p className="set-hint">{t('media.references.hint')}</p>
          <div className="flex-row gap-2">
            <button type="button" className="btn btn-sm" disabled={saving || invalid || !ready} onClick={() => void save()}>{t('media.references.save')}</button>
            <button type="button" className="btn btn-sm btn-ghost" disabled={saving} onClick={() => { setError(null); setRevision(value => value + 1); }}>{t('media.references.refresh')}</button>
          </div>
        </>}
        {(error || current?.error) && <p role="alert" className="set-ollama-context-error">{error || current?.error}</p>}
        {connectionId === 'local:comfyui' && <>
          <p className="set-hint">{t('media.references.comfyHelp')}</p>
          <a href={modality === 'image' ? 'https://comfyanonymous.github.io/ComfyUI_examples/flux/#redux' : 'https://comfyanonymous.github.io/ComfyUI_examples/wan/#vace-reference-image-to-video'} target="_blank" rel="noreferrer">{t('media.references.guide')}</a>
        </>}
      </div>}
    </details>
  );
}
