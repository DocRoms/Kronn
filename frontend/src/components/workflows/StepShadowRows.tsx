/**
 * KT-1696 — the shadows of an Agent step as sub-rows under its agent, and the
 * merged result they feed. Each row picks the shadow's agent and model and
 * carries two one-click toggles: Active (the shadow runs) and Merge (its answer
 * goes into the step result, the one the next step receives). Used by the
 * workflow overview, where a change saves at once, and by the editor.
 *
 * KT-1707 — a step with shadows reads as one table. Every row has the same
 * layout, `role · agent · model [badge]`: the highlighted « Principal » row
 * first (the step's own picker, always active, the base of the merge), then the
 * « Shadow » rows indented under it. The shadow's free label is a tooltip.
 */
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { CircleHelp, Plus, X } from 'lucide-react';
import { DEFAULT_MERGE_MINUTES, MAX_MERGE_MINUTES } from '../../lib/shadowRuns';
import {
  activeMergedIds, canPauseShadow, freshShadowId, withMergeAgent, withShadowAdded, withShadowChanged, withShadowEnabled,
  withShadowMerged, withShadowRemoved, type ShadowEdit,
} from '../../lib/shadowMerge';
import { MAX_SHADOWS, stepCanCarryShadows } from './shadowEditorModel';
import { agentModelLabel, shadowDifferences, stepLoadLabel, stepMainAgent, type LoadNames } from './shadowRowModel';
import { AgentSwitchPicker, TransportBadge, type AgentSwitchTarget, type ExactModelSelection, type MainAgent } from '../AgentSwitchPicker';
import './StepShadowRows.css';
import type { ModelTiersConfig, StepShadow, WorkflowStep } from '../../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

interface Props {
  step: WorkflowStep;
  /** The step the result goes to, when there is one. */
  nextStepName?: string;
  choices?: readonly AgentSwitchTarget[];
  /** The agent families' configured models: they name the default model of an agent. */
  modelTiers?: ModelTiersConfig | null;
  /** Applies an edit; without it the rows only show. */
  onEdit?: (edit: ShadowEdit) => void;
  /** Quick Prompt and skill names: with them each row says the prompt and skills it runs. */
  names?: LoadNames;
  /** Offer « + Shadow » on a step that has none yet. */
  addWhenEmpty?: boolean;
  /** The step's own agent/model picker: it moves into the Principal row once the step has shadows. */
  mainPicker?: ReactNode;
  t: Translate;
  testId?: string;
}

/** The `?` of the merged result: hover, or tap, opens what the merge does. */
function MergeHelp({ merged, next, timeoutSecs, t, testId }: { merged: string; next: string | undefined; timeoutSecs: number | undefined; t: Translate; testId: string }) {
  const [hover, setHover] = useState(false);
  const [pinned, setPinned] = useState(false);
  const rootRef = useRef<HTMLSpanElement>(null);
  const open = hover || pinned;
  useEffect(() => {
    if (!open) return;
    const close = (event: Event) => {
      if (event instanceof KeyboardEvent ? event.key === 'Escape' : !rootRef.current?.contains(event.target as Node)) {
        setHover(false);
        setPinned(false);
      }
    };
    document.addEventListener('pointerdown', close);
    document.addEventListener('keydown', close);
    return () => {
      document.removeEventListener('pointerdown', close);
      document.removeEventListener('keydown', close);
    };
  }, [open]);
  const minutes = Math.min(MAX_MERGE_MINUTES, Math.max(1, Math.ceil((timeoutSecs ?? DEFAULT_MERGE_MINUTES * 60) / 60)));
  return (
    <span ref={rootRef} className="wf-shadow-help" onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}>
      <button
        type="button"
        className="wf-shadow-help-btn"
        data-testid={`${testId}-help`}
        aria-label={t('wf.shadow.rows.helpLabel')}
        aria-expanded={open}
        onClick={() => setPinned(value => !value)}
      >
        <CircleHelp size={11} aria-hidden />
      </button>
      {open && (
        <span className="wf-shadow-help-pop" role="dialog" aria-label={t('wf.shadow.rows.helpTitle')} data-testid={`${testId}-help-pop`}>
          <strong>{t('wf.shadow.rows.helpTitle')}</strong>
          <span>{t('wf.shadow.rows.helpOne')}</span>
          <code>{t('wf.shadow.rows.helpFlow', merged, next ?? t('wf.shadow.rows.helpResult'))}</code>
          <span>{t('wf.shadow.rows.helpDedupe')}</span>
          <span>{t('wf.shadow.rows.helpCompared')}</span>
          <span>{t('wf.shadow.rows.helpWait', minutes)}</span>
        </span>
      )}
    </span>
  );
}

/** KT-1885 — the shared agent picker in its « exact model » mode: agent + model id, « = principal », main model greyed. */
function ExactModelPicker({ choices, modelTiers, disabled, ariaLabel, ...selection }: ExactModelSelection & {
  choices?: readonly AgentSwitchTarget[];
  modelTiers?: ModelTiersConfig | null;
  disabled?: boolean;
  ariaLabel: string;
}) {
  return (
    <AgentSwitchPicker
      currentAgent={selection.main.agent}
      availableAgents={[]}
      availableTargets={choices?.length ? [...choices] : undefined}
      modelTiers={modelTiers}
      disabled={disabled}
      title={ariaLabel}
      ariaLabel={ariaLabel}
      exactModel={selection}
    />
  );
}

function Toggle({ kind, on, label, title, disabled, onClick, testId }: {
  kind: 'active' | 'merge'; on: boolean; label: string; title: string; disabled?: boolean; onClick: () => void; testId: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      className="wf-shadow-switch"
      data-kind={kind}
      data-on={on}
      aria-checked={on}
      aria-label={label}
      title={title}
      disabled={disabled}
      data-testid={testId}
      onClick={onClick}
    >
      <span className="wf-shadow-switch-knob" aria-hidden />
      <span>{label}</span>
    </button>
  );
}

export function StepShadowRows({ step, nextStepName, choices, modelTiers, names, onEdit, addWhenEmpty = false, mainPicker, t, testId = 'wf-shadow-rows' }: Props) {
  const shadows = step.shadows ?? [];
  const merge = step.shadow_merge;
  if (!stepCanCarryShadows(step) || (shadows.length === 0 && !(addWhenEmpty && onEdit))) return null;

  const main: MainAgent = stepMainAgent(step);
  const merged = activeMergedIds(merge, shadows);
  const targets = choices ?? [];
  const mainLabel = agentModelLabel(undefined, main, targets, t, modelTiers);
  const agentName = (shadow: StepShadow) => agentModelLabel(shadow.agent, main, targets, t, modelTiers);
  const mergedNames = shadows.filter(shadow => merged.includes(shadow.id)).map(shadow => t('wf.shadow.rows.shadowNamed', agentName(shadow)));
  const editable = !!onEdit;
  const load = names ? stepLoadLabel(step, names, t) : '';

  return (
    <div className="wf-shadow-rows" data-testid={testId} data-block={shadows.length > 0} onClick={event => event.stopPropagation()}>
      {shadows.length > 0 && (
        <div className="wf-shadow-subrow wf-shadow-principal" data-role="principal" data-testid={`${testId}-principal`}>
          <span className="wf-shadow-subrow-name">{t('wf.shadow.rows.principal')}</span>
          <span className="wf-shadow-subrow-agent" data-testid={`${testId}-principal-agent`}>
            {mainPicker ?? <span className="wf-shadow-static" title={mainLabel}>{mainLabel}</span>}
          </span>
          <TransportBadge agent={step.agent} connectionId={step.agent_settings?.connection_id} t={t} />
          <span className="wf-shadow-subrow-toggles">
            <span className="wf-shadow-chip" data-testid={`${testId}-principal-active`}>{t('wf.shadow.rows.alwaysActive')}</span>
            {merged.length > 0 && <span className="wf-shadow-base" data-testid={`${testId}-principal-base`}>{t('wf.shadow.rows.mergeBase')}</span>}
          </span>
          {load && <span className="wf-shadow-load" data-testid={`${testId}-principal-load`}>{load}</span>}
        </div>
      )}
      {shadows.map(shadow => {
        const rowId = `${testId}-${shadow.id}`;
        const isMerged = merged.includes(shadow.id);
        const active = shadow.enabled !== false;
        const name = agentName(shadow);
        const tooltip = shadow.label || shadow.id;
        const differences = names ? shadowDifferences(step, shadow, names, t) : [];
        const mergeTip = nextStepName ? t('wf.shadow.rows.mergeTip', name, nextStepName) : t('wf.shadow.rows.mergeTipLast', name);
        return (
          <div key={shadow.id} className="wf-shadow-subrow" data-role="shadow" data-testid={rowId} data-paused={!active} data-merged={isMerged} title={tooltip}>
            <span className="wf-shadow-subrow-name">{t('wf.shadow.rows.roleShadow')}</span>
            <span className="wf-shadow-subrow-agent">
              <ExactModelPicker
                value={shadow.agent}
                choices={choices}
                main={main}
                modelTiers={modelTiers}
                disabled={!editable}
                ariaLabel={t('wf.shadow.rows.pickerLabel', tooltip)}
                testId={`${rowId}-agent`}
                onChange={agent => onEdit?.(withShadowChanged(shadows, merge, shadow.id, { ...shadow, agent }))}
              />
            </span>
            <TransportBadge agent={shadow.agent?.agent ?? step.agent} connectionId={shadow.agent ? shadow.agent.connection_id : step.agent_settings?.connection_id} t={t} />
            <span className="wf-shadow-subrow-toggles">
              <Toggle
                kind="active"
                on={active}
                label={t('wf.shadow.rows.active')}
                title={canPauseShadow(merge, shadows, shadow.id) ? t('wf.shadow.rows.activeTip') : t('wf.shadow.rows.activeLocked')}
                disabled={!editable || (active && !canPauseShadow(merge, shadows, shadow.id))}
                testId={`${rowId}-active`}
                onClick={() => onEdit?.(withShadowEnabled(shadows, merge, shadow.id, !active))}
              />
              <Toggle
                kind="merge"
                on={isMerged}
                label={t('wf.shadow.rows.merge')}
                title={mergeTip}
                disabled={!editable}
                testId={`${rowId}-merge`}
                onClick={() => onEdit?.(withShadowMerged(shadows, merge, shadow.id, !isMerged))}
              />
              {editable && (
                <button
                  type="button"
                  className="wf-shadow-subrow-remove"
                  data-testid={`${rowId}-remove`}
                  aria-label={t('wf.shadow.remove', tooltip)}
                  title={t('wf.shadow.remove', tooltip)}
                  onClick={() => onEdit?.(withShadowRemoved(shadows, merge, shadow.id))}
                >
                  <X size={11} aria-hidden />
                </button>
              )}
            </span>
            {names && (
              <span className="wf-shadow-load" data-testid={`${rowId}-load`} data-same={differences.length === 0}>
                {differences.length === 0
                  ? <span className="wf-shadow-load-same">{t('wf.shadow.rows.diff.same')}</span>
                  : differences.map(text => <span key={text} className="wf-shadow-diff">{text}</span>)}
              </span>
            )}
          </div>
        );
      })}
      {editable && shadows.length < MAX_SHADOWS && (
        <ExactModelPicker
          value={undefined}
          choices={choices}
          main={main}
          modelTiers={modelTiers}
          ariaLabel={t('wf.shadow.rows.addLabel')}
          testId={`${testId}-add`}
          className="wf-shadow-add"
          onChange={agent => onEdit?.(withShadowAdded(shadows, merge, { id: freshShadowId(shadows), label: '', enabled: true, ...(agent ? { agent } : {}) }))}
        >
          <Plus size={10} aria-hidden /> <span>{t('wf.shadow.rows.add')}</span>
        </ExactModelPicker>
      )}
      {merge && merged.length > 0 && (
        <div className="wf-shadow-merged" data-testid={`${testId}-merged`}>
          <span className="wf-shadow-merged-text">
            {nextStepName
              ? t('wf.shadow.rows.summary', mergedNames.join(' + '), nextStepName)
              : t('wf.shadow.rows.summaryLast', mergedNames.join(' + '))}
          </span>
          <MergeHelp
            merged={mergedNames.join(' + ')}
            next={nextStepName}
            timeoutSecs={merge.timeout_secs}
            t={t}
            testId={testId}
          />
          <span className="wf-shadow-merged-agent" data-testid={`${testId}-merged-agent`}>
            <span className="wf-shadow-merged-by">{t('wf.shadow.rows.mergedBy')}</span>
            <ExactModelPicker
              value={merge.agent}
              choices={choices}
              main={main}
              modelTiers={modelTiers}
              disabled={!editable}
              ariaLabel={t('wf.shadow.rows.condenseLabel')}
              testId={`${testId}-condense`}
              onChange={agent => onEdit?.(withMergeAgent(shadows, merge, agent))}
            />
          </span>
        </div>
      )}
    </div>
  );
}
