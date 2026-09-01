import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, CheckCircle2, ChevronDown, ExternalLink, Loader2, Play, RotateCcw, X } from 'lucide-react';
import { discussionActions as discussionActionsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import type { DiscussionAction, DiscussionActionValue, LivePageAction, PromptVariable } from '../types/generated';
import { PromptVariableInput } from './workflows/PromptVariableInput';
import { RunStatusCard } from './RunStatusCard';
import './DiscussionActionCard.css';

interface Props {
  action: DiscussionAction;
  onChanged: (action: DiscussionAction) => void;
  onOpenDiscussion: (discussionId: string) => void;
}

type KronnAction = DiscussionAction | LivePageAction;

export interface KronnActionOperations<T extends KronnAction> {
  get: (actionId: string) => Promise<T>;
  cancel: (actionId: string) => Promise<T>;
  launch: (actionId: string, request: {
    variables: Record<string, string>;
    bindings?: Record<string, string>;
  }) => Promise<T>;
}

interface KronnActionCardProps<T extends KronnAction> {
  action: T;
  operations: KronnActionOperations<T>;
  onChanged: (action: T) => void;
  onOpenDiscussion: (discussionId: string) => void;
  bindings?: Record<string, string>;
  initiallyExpanded?: boolean;
  testIdPrefix?: string;
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

function initialValues(action: KronnAction): Record<string, string> {
  return Object.fromEntries(action.values
    .filter(isEditableValue)
    .map(value => [value.name, value.value ?? value.suggested_value ?? '']));
}

function provenanceLabel(
  value: DiscussionActionValue,
  t: (key: string, ...args: (string | number)[]) => string,
  binding?: string,
) {
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
  if (value.provenance === 'dynamic_binding') {
    return t('disc.action.dynamicBinding', binding ? `${reference} → ${binding}` : reference);
  }
  return null;
}

/** Shared native renderer for discussion- and Live-Page-authored actions. The
 * origin supplies only its durable API adapter; state, preflight form and run
 * rendering deliberately stay in this one component. */
export function KronnActionCard<T extends KronnAction>({
  action,
  operations,
  onChanged,
  onOpenDiscussion,
  bindings,
  initiallyExpanded = false,
  testIdPrefix = 'discussion-action',
}: KronnActionCardProps<T>) {
  const { t } = useT();
  const current = action;
  const [values, setValues] = useState<Record<string, string>>(() => initialValues(action));
  const [expanded, setExpanded] = useState(initiallyExpanded);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlightRef = useRef(false);

  useEffect(() => {
    if (!ACTIVE_STATES.has(current.state)) return;
    let cancelled = false;
    const poll = window.setInterval(() => {
      void operations.get(current.id)
        .then(next => {
          if (cancelled) return;
          onChanged(next);
        })
        .catch(() => { /* RunStatusCard and the next poll keep recovery bounded. */ });
    }, 1_500);
    return () => {
      cancelled = true;
      window.clearInterval(poll);
    };
  }, [current.id, current.state, onChanged, operations]);

  const missingRequired = useMemo(() => current.values.some(value =>
    isEffectivelyRequired(value)
      && !(values[value.name] ?? '').trim(),
  ), [current.values, values]);

  const update = (next: T) => {
    onChanged(next);
  };

  const runOnce = async (operation: () => Promise<T>) => {
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
  const stalePageSource = 'stale_source' in current && current.stale_source;

  return (
    <section className="discussion-action-card" data-state={current.state} data-expanded={expanded} data-testid={`${testIdPrefix}-${current.id}`}>
      <button
        type="button"
        className="discussion-action-card__toggle"
        aria-expanded={expanded}
        onClick={() => {
          if (!expanded && current.state === 'proposed') setValues(initialValues(current));
          setExpanded(!expanded);
        }}
      >
        <span className="discussion-action-card__header">
          <span className="discussion-action-card__heading">
            <span className="discussion-action-card__eyebrow">{t('disc.action.proposal')}</span>
            <span className="discussion-action-card__title-row">
              <strong>{kindLabel} · {current.target_name}</strong>
            </span>
          </span>
          <span className="discussion-action-card__state" data-state={current.state}>
            {ACTIVE_STATES.has(current.state) && <Loader2 size={13} className="spin" aria-hidden />}
            {current.state === 'succeeded' && <CheckCircle2 size={13} aria-hidden />}
            {(current.state === 'failed' || current.state === 'preflight_failed') && <AlertTriangle size={13} aria-hidden />}
            {current.state === 'cancelled' && <X size={13} aria-hidden />}
            {t(`disc.action.state.${current.state}`)}
          </span>
          <ChevronDown size={14} className="discussion-action-card__chevron" aria-hidden />
        </span>
      </button>

      {expanded && current.state === 'proposed' && current.values.length > 0 && (
        <div className="discussion-action-card__fields">
          {current.values.map(value => {
            const editable = isEditableValue(value);
            const provenance = provenanceLabel(value, t, bindings?.[value.name]);
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

      {expanded && current.shared_run_id && (
        <RunStatusCard runId={current.shared_run_id} compact={current.state === 'running'} />
      )}

      {expanded && stalePageSource && (
        <p className="discussion-action-card__diagnostic" role="status">
          <AlertTriangle size={13} aria-hidden /> {t(
            current.state === 'proposed'
              ? 'disc.action.unavailablePageAction'
              : 'disc.action.stalePageSource',
          )}
        </p>
      )}
      {expanded && current.diagnostic && (
        <p className="discussion-action-card__diagnostic" role="alert">
          <AlertTriangle size={13} aria-hidden /> {current.diagnostic}
        </p>
      )}
      {expanded && error && <p className="discussion-action-card__diagnostic" role="alert">{error}</p>}

      {expanded && <footer className="discussion-action-card__actions">
        {current.state === 'proposed' && (
          <>
            <button
              type="button"
              className="discussion-action-card__cancel"
              disabled={busy}
              onClick={() => void runOnce(() => operations.cancel(current.id))}
            >
              {t('common.cancel')}
            </button>
            <button
              type="button"
              className="discussion-action-card__launch"
              disabled={busy || missingRequired || stalePageSource}
              onClick={() => void runOnce(() => operations.launch(current.id, { variables: values, bindings }))}
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
      </footer>}
    </section>
  );
}

const discussionActionOperations: KronnActionOperations<DiscussionAction> = {
  get: actionId => discussionActionsApi.get(actionId),
  cancel: actionId => discussionActionsApi.cancel(actionId),
  launch: (actionId, request) => discussionActionsApi.launch(actionId, { variables: request.variables }),
};

export function DiscussionActionCard({ action, onChanged, onOpenDiscussion }: Props) {
  return (
    <KronnActionCard
      action={action}
      operations={discussionActionOperations}
      onChanged={onChanged}
      onOpenDiscussion={onOpenDiscussion}
    />
  );
}
