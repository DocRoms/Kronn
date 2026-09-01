import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, CheckCircle2, ExternalLink, Loader2, Play, RotateCcw, X } from 'lucide-react';
import { discussionActions as discussionActionsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import type { DiscussionAction, DiscussionActionValue, PromptVariable } from '../types/generated';
import { PromptVariableInput } from './workflows/PromptVariableInput';
import { RunStatusCard } from './RunStatusCard';
import './DiscussionActionCard.css';

interface Props {
  action: DiscussionAction;
  onChanged: (action: DiscussionAction) => void;
  onOpenDiscussion: (discussionId: string) => void;
}

const ACTIVE_STATES = new Set<DiscussionAction['state']>(['launching', 'running']);
const TERMINAL_STATES = new Set<DiscussionAction['state']>([
  'succeeded', 'failed', 'cancelled', 'preflight_failed',
]);

/** A `project_env`/`kronn_context` value is only ever editable when its own
 * declaration opted into an audited manual override; it then stays optional
 * — Kronn still resolves it when the launcher leaves the field blank. */
function isEditableValue(value: DiscussionActionValue): boolean {
  return value.provenance === 'user_input' || value.provenance === 'agent_suggestion'
    || value.allow_manual_override;
}

/** Only a true human input can ever be *required*; an optional override on a
 * Kronn-resolved value must never block launch when left blank. */
function isEffectivelyRequired(value: DiscussionActionValue): boolean {
  return (value.provenance === 'user_input' || value.provenance === 'agent_suggestion') && value.required;
}

function promptVariable(value: DiscussionActionValue): PromptVariable {
  const isEnvironmentOverride = value.provenance === 'project_env' || value.provenance === 'kronn_context';
  return {
    name: value.name,
    label: value.label,
    placeholder: value.placeholder,
    description: value.description,
    required: isEnvironmentOverride ? false : value.required,
    source: 'user_input',
    allow_manual_override: value.allow_manual_override,
    control: value.control,
  };
}

function initialValues(action: DiscussionAction): Record<string, string> {
  return Object.fromEntries(action.values
    .filter(isEditableValue)
    .map(value => [value.name, value.value ?? value.suggested_value ?? '']));
}

function provenanceLabel(value: DiscussionActionValue, t: (key: string, ...args: (string | number)[]) => string) {
  if (value.provenance === 'agent_suggestion') {
    return t('disc.action.suggestedBy', value.suggested_by ?? t('disc.action.agent'));
  }
  const reference = value.source_ref ?? value.name;
  if (value.provenance === 'project_env') {
    return value.allow_manual_override
      ? t('disc.action.projectEnvOverridable', reference)
      : t('disc.action.projectEnv', reference);
  }
  if (value.provenance === 'kronn_context') {
    return value.allow_manual_override
      ? t('disc.action.kronnContextOverridable', reference)
      : t('disc.action.kronnContext', reference);
  }
  return null;
}

export function DiscussionActionCard({ action, onChanged, onOpenDiscussion }: Props) {
  const { t } = useT();
  const [current, setCurrent] = useState(action);
  const [values, setValues] = useState<Record<string, string>>(() => initialValues(action));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlightRef = useRef(false);

  useEffect(() => {
    setCurrent(action);
    if (action.state === 'proposed') setValues(initialValues(action));
  }, [action]);

  useEffect(() => {
    if (!ACTIVE_STATES.has(current.state)) return;
    let cancelled = false;
    const poll = window.setInterval(() => {
      void discussionActionsApi.get(current.id)
        .then(next => {
          if (cancelled) return;
          setCurrent(next);
          onChanged(next);
        })
        .catch(() => { /* RunStatusCard and the next poll keep recovery bounded. */ });
    }, 1_500);
    return () => {
      cancelled = true;
      window.clearInterval(poll);
    };
  }, [current.id, current.state, onChanged]);

  const missingRequired = useMemo(() => current.values.some(value =>
    isEffectivelyRequired(value)
      && !(values[value.name] ?? '').trim(),
  ), [current.values, values]);

  const update = (next: DiscussionAction) => {
    setCurrent(next);
    onChanged(next);
  };

  const runOnce = async (operation: () => Promise<DiscussionAction>) => {
    if (inFlightRef.current) return;
    inFlightRef.current = true;
    setBusy(true);
    setError(null);
    try {
      update(await operation());
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      inFlightRef.current = false;
      setBusy(false);
    }
  };

  const terminal = TERMINAL_STATES.has(current.state);
  const kindLabel = t(`disc.action.kind.${current.kind}`);
  const resultDiscussionId = current.result_discussion_id;

  return (
    <section className="discussion-action-card" data-state={current.state} data-testid={`discussion-action-${current.id}`}>
      <header className="discussion-action-card__header">
        <span className="discussion-action-card__eyebrow">{t('disc.action.proposal')}</span>
        <div className="discussion-action-card__title-row">
          <h4>{kindLabel} · {current.target_name}</h4>
          <span className="discussion-action-card__state" data-state={current.state}>
            {ACTIVE_STATES.has(current.state) && <Loader2 size={13} className="spin" aria-hidden />}
            {current.state === 'succeeded' && <CheckCircle2 size={13} aria-hidden />}
            {(current.state === 'failed' || current.state === 'preflight_failed') && <AlertTriangle size={13} aria-hidden />}
            {current.state === 'cancelled' && <X size={13} aria-hidden />}
            {t(`disc.action.state.${current.state}`)}
          </span>
        </div>
      </header>

      {current.state === 'proposed' && current.values.length > 0 && (
        <div className="discussion-action-card__fields">
          {current.values.map(value => {
            const editable = isEditableValue(value);
            const provenance = provenanceLabel(value, t);
            return (
              <div key={value.name} className="discussion-action-card__field">
                <span className="discussion-action-card__label">
                  {value.label || value.name}{isEffectivelyRequired(value) ? ' *' : ''}
                </span>
                {editable ? (
                  <PromptVariableInput
                    variable={promptVariable(value)}
                    value={values[value.name] ?? ''}
                    onChange={next => setValues(currentValues => ({ ...currentValues, [value.name]: next }))}
                    disabled={busy}
                  />
                ) : (
                  <input
                    className="wf-input discussion-action-card__resolved"
                    value={t('disc.action.resolvedAtLaunch')}
                    disabled
                    aria-label={value.label || value.name}
                  />
                )}
                {(value.description || provenance) && (
                  <small>{value.description}{value.description && provenance ? ' · ' : ''}{provenance}</small>
                )}
                {value.provenance === 'agent_suggestion' && value.suggested_value !== undefined
                  && values[value.name] !== value.suggested_value && (
                  <button
                    type="button"
                    className="discussion-action-card__reset"
                    onClick={() => setValues(currentValues => ({
                      ...currentValues,
                      [value.name]: value.suggested_value ?? '',
                    }))}
                  >
                    <RotateCcw size={11} aria-hidden /> {t('disc.action.restoreSuggestion')}
                  </button>
                )}
              </div>
            );
          })}
        </div>
      )}

      {current.shared_run_id && (
        <RunStatusCard runId={current.shared_run_id} compact={current.state === 'running'} />
      )}

      {current.diagnostic && (
        <p className="discussion-action-card__diagnostic" role="alert">
          <AlertTriangle size={13} aria-hidden /> {current.diagnostic}
        </p>
      )}
      {error && <p className="discussion-action-card__diagnostic" role="alert">{error}</p>}

      <footer className="discussion-action-card__actions">
        {current.state === 'proposed' && (
          <>
            <button
              type="button"
              className="discussion-action-card__cancel"
              disabled={busy}
              onClick={() => void runOnce(() => discussionActionsApi.cancel(current.id))}
            >
              {t('common.cancel')}
            </button>
            <button
              type="button"
              className="discussion-action-card__launch"
              disabled={busy || missingRequired}
              onClick={() => void runOnce(() => discussionActionsApi.launch(current.id, { variables: values }))}
            >
              {busy ? <Loader2 size={13} className="spin" aria-hidden /> : <Play size={13} aria-hidden />}
              {t('disc.action.launch')}
            </button>
          </>
        )}
        {terminal && resultDiscussionId && (
          <button
            type="button"
            className="discussion-action-card__open"
            onClick={() => onOpenDiscussion(resultDiscussionId)}
          >
            <ExternalLink size={13} aria-hidden /> {t('disc.action.openDiscussion')}
          </button>
        )}
      </footer>
    </section>
  );
}
