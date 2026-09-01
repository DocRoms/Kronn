import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, Eye, EyeOff, Loader2 } from 'lucide-react';
import { executionVariables } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import type { PromptVariable } from '../../types/generated';

interface Props {
  variables: PromptVariable[];
  projectId?: string | null;
  values: Record<string, string>;
  onValueChange: (name: string, value: string | undefined) => void;
  disabled?: boolean;
}

export function ProvidedVariablesPreview({
  variables,
  projectId,
  values,
  onValueChange,
  disabled = false,
}: Props) {
  const { t } = useT();
  const [previewRunId, setPreviewRunId] = useState<string | null>(null);
  const [resolvedAt, setResolvedAt] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [revealed, setRevealed] = useState<Record<string, string>>({});
  const [revealing, setRevealing] = useState<string | null>(null);
  const [overrides, setOverrides] = useState<Record<string, boolean>>({});
  const revealTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  const definitionKey = useMemo(() => JSON.stringify([
    projectId ?? null,
    variables.map(variable => ({
      name: variable.name,
      source: variable.source,
      source_ref: variable.source_ref,
      required: variable.required,
      allow_manual_override: variable.allow_manual_override,
    })),
  ]), [projectId, variables]);

  useEffect(() => {
    let active = true;
    void Promise.resolve()
      .then(() => {
        if (!active) return Promise.reject(new DOMException('Preview cancelled', 'AbortError'));
        setLoading(true);
        setError(null);
        setPreviewRunId(null);
        setResolvedAt(null);
        setRevealed({});
        setOverrides({});
        return executionVariables.preview(projectId, variables);
      })
      .then(preview => {
        if (!active) return;
        setPreviewRunId(preview.run_id);
        setResolvedAt(preview.metadata.resolved_at);
      })
      .catch(cause => {
        if (active && !(cause instanceof DOMException && cause.name === 'AbortError')) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => { active = false; };
    // definitionKey is the canonical trigger; `variables` may be recreated by
    // a parent render without changing the declaration.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [definitionKey]);

  useEffect(() => {
    const timers = revealTimers.current;
    return () => Object.values(timers).forEach(clearTimeout);
  }, []);

  const remask = (name: string) => {
    if (revealTimers.current[name]) clearTimeout(revealTimers.current[name]);
    delete revealTimers.current[name];
    setRevealed(current => {
      const next = { ...current };
      delete next[name];
      return next;
    });
  };

  const toggleReveal = async (name: string) => {
    if (revealed[name] !== undefined) {
      remask(name);
      return;
    }
    if (!previewRunId || revealing) return;
    setRevealing(name);
    setError(null);
    try {
      const value = await executionVariables.reveal('preview', previewRunId, name);
      setRevealed(current => ({ ...current, [name]: value }));
      revealTimers.current[name] = setTimeout(() => remask(name), 30_000);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setRevealing(null);
    }
  };

  return (
    <section className="provided-variables-preview" aria-label={t('wf.launchProvidedTitle')}>
      <h3 className="text-xs font-medium mt-4">{t('wf.launchProvidedTitle')}</h3>
      {variables.map(variable => {
        const label = variable.label || variable.name;
        const overridden = overrides[variable.name] === true;
        return (
          <div key={variable.name} className="qp-launch-field">
            <label className="qp-launch-label">{label}</label>
            {overridden ? (
              <input
                className="wf-input flex-1"
                value={values[variable.name] ?? ''}
                onChange={event => onValueChange(variable.name, event.target.value)}
                placeholder={t('wf.launchOverridePlaceholder')}
                aria-label={t('wf.launchOverrideAria', label)}
                disabled={disabled}
              />
            ) : (
              <div className="flex-row gap-2">
                <input
                  className="wf-input flex-1"
                  value={revealed[variable.name] ?? '••••••'}
                  readOnly
                  onBlur={() => remask(variable.name)}
                  aria-label={revealed[variable.name] === undefined
                    ? t('wf.launchMaskedAria', label)
                    : t('executionVariables.revealed', label)}
                />
                <button
                  type="button"
                  className="wf-icon-btn"
                  disabled={disabled || loading || !previewRunId || revealing === variable.name}
                  onClick={() => void toggleReveal(variable.name)}
                  aria-label={revealed[variable.name] === undefined
                    ? t('executionVariables.revealTemporarily', label)
                    : t('executionVariables.remask', label)}
                >
                  {revealing === variable.name
                    ? <Loader2 size={12} className="spin" />
                    : revealed[variable.name] === undefined
                      ? <Eye size={12} />
                      : <EyeOff size={12} />}
                </button>
              </div>
            )}
            <p className="text-2xs text-ghost">
              {variable.source_ref} · {resolvedAt
                ? t('wf.launchResolvedPreview', new Date(resolvedAt).toLocaleTimeString())
                : t('wf.launchResolvedAtDispatch')}
            </p>
            {variable.allow_manual_override && (
              <button
                type="button"
                className="wf-small-btn mt-1"
                disabled={disabled}
                onClick={() => {
                  const next = !overridden;
                  setOverrides(current => ({ ...current, [variable.name]: next }));
                  onValueChange(variable.name, next ? '' : undefined);
                  remask(variable.name);
                }}
              >
                {overridden ? t('wf.launchUseProjectValue') : t('wf.launchUseOverride')}
              </button>
            )}
          </div>
        );
      })}
      {error && (
        <div className="wf-variable-preview-error" role="alert">
          <AlertTriangle size={12} />
          <span>{t('wf.launchPreviewUnavailable')}</span>
        </div>
      )}
    </section>
  );
}
