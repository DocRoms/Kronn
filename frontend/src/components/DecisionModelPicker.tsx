import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { AlertTriangle, Check, ChevronDown, Compass, Loader2 } from 'lucide-react';
import { useModelCatalogSnapshot } from '../hooks/useModelCatalogSnapshot';
import { useT } from '../lib/I18nContext';
import {
  decisionModelOptions,
  sameDecisionChoice,
  type DecisionModelChoice,
  type DecisionModelOption,
} from '../lib/decisionModels';
import './AgentSwitchPicker.css';
import './DecisionModelPicker.css';

interface DecisionModelPickerProps {
  /** The step's own choice; `model: null` = the configured Decision model. */
  choice: DecisionModelChoice;
  /** The configured Decision model, what « Par défaut » runs. */
  defaultModel?: string | null;
  /** Saves the choice. The caller owns the optimistic update, the rollback and
   *  the error message; a rejection keeps the menu open. Absent = read-only. */
  onChange?: (choice: DecisionModelChoice) => Promise<void> | void;
  disabled?: boolean;
  compact?: boolean;
  ariaLabel: string;
  /** Shown instead of the picker's own list (callers that hold the catalogue). */
  options?: DecisionModelOption[];
}

/**
 * KT-1697 — the decision model of a Decide step, picked like the Agent step's
 * model: one button `Décision · <model> ▾` with its transport badge, a menu of
 * the decision-capable models (default first). With nothing to choose from it
 * stays visible and read-only. Shared by the workflow overview and the editor,
 * and by the Decide-step shadows later.
 */
export function DecisionModelPicker({
  choice, defaultModel, onChange, disabled = false, compact = false, ariaLabel, options: suppliedOptions,
}: DecisionModelPickerProps) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [position, setPosition] = useState<{ top: number; left: number; maxHeight: number } | null>(null);
  const savingRef = useRef(false);
  const rootRef = useRef<HTMLSpanElement>(null);
  const popoverRef = useRef<HTMLSpanElement>(null);
  // Ollama's catalogue is local: loading it on mount costs one list call.
  const { catalog, catalogError } = useModelCatalogSnapshot(!suppliedOptions, ['agent:ollama']);
  const options = suppliedOptions ?? decisionModelOptions(catalog);
  const loaded = suppliedOptions !== undefined || catalog !== null;

  const configured = defaultModel?.trim() || null;
  const shownModel = choice.model ?? configured;
  const transport = choice.connectionId ? 'http' : 'local';
  const listed = options.some(option => option.model === choice.model && option.connectionId === choice.connectionId);
  // A chosen model the catalogue does not list is flagged, never swapped.
  const missing = loaded && choice.model !== null && !listed;
  const distinct = new Set([configured, ...options.map(option => option.model)].filter(Boolean));
  const canChange = Boolean(onChange) && (distinct.size > 1 || missing);

  const updatePosition = useCallback(() => {
    const rect = rootRef.current?.getBoundingClientRect();
    if (!rect) return;
    const padding = 8;
    const estimated = (options.length + 1) * 30 + 12;
    const below = rect.bottom + 5 + estimated <= window.innerHeight - padding;
    const top = below ? rect.bottom + 5 : Math.max(padding, rect.top - estimated - 5);
    const left = Math.max(padding, Math.min(rect.left, window.innerWidth - 260 - padding));
    setPosition({ top, left, maxHeight: Math.max(0, window.innerHeight - top - padding) });
  }, [options.length]);

  useEffect(() => {
    if (!open) return;
    updatePosition();
    const closeOutside = (event: MouseEvent) => {
      const target = event.target as Node;
      if (!rootRef.current?.contains(target) && !popoverRef.current?.contains(target)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !event.isComposing) setOpen(false);
    };
    document.addEventListener('mousedown', closeOutside);
    document.addEventListener('keydown', closeOnEscape);
    window.addEventListener('resize', updatePosition);
    window.addEventListener('scroll', updatePosition, true);
    return () => {
      document.removeEventListener('mousedown', closeOutside);
      document.removeEventListener('keydown', closeOnEscape);
      window.removeEventListener('resize', updatePosition);
      window.removeEventListener('scroll', updatePosition, true);
    };
  }, [open, updatePosition]);

  const select = async (next: DecisionModelChoice) => {
    if (savingRef.current || !onChange || sameDecisionChoice(next, choice)) return;
    savingRef.current = true;
    setSaving(true);
    try {
      await onChange(next);
      setOpen(false);
    } catch {
      // The caller owns the user-facing error; keep the menu open to pick again.
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  const modelText = shownModel ?? t('decisionPicker.notConfigured');
  const body = (
    <>
      <Compass size={compact ? 9 : 10} aria-hidden="true" />
      <span>{t('decisionPicker.name')}</span>
      <span className="kr-agent-switch-suffix"> · <span data-testid="decision-model-name">{modelText}</span></span>
      <span className="kr-decision-transport" data-transport={transport} data-testid="decision-transport">
        {t(`decisionPicker.transport.${transport}`)}
      </span>
      {missing && (
        <span className="kr-decision-missing" role="img" aria-label={t('decisionPicker.missing')} title={t('decisionPicker.missing')}>
          <AlertTriangle size={10} />
        </span>
      )}
    </>
  );

  if (!canChange) {
    return (
      <span className={compact ? 'wf-pipe-chip-agent kr-decision-static' : 'wf-step-agent-static kr-decision-static'}
        data-testid="decision-model-static">
        {body}
      </span>
    );
  }

  const isDefault = choice.model === null;
  return (
    <span ref={rootRef} className="kr-agent-switch" data-compact={compact} data-open={open}
      onClick={event => event.stopPropagation()}>
      <button
        type="button"
        className="kr-agent-switch-btn kr-decision-btn"
        title={`${t('decisionPicker.title')} · ${modelText}`}
        aria-label={ariaLabel}
        aria-haspopup="dialog"
        aria-expanded={open}
        data-testid="decision-model-picker"
        disabled={disabled || saving}
        onClick={() => { if (!open) updatePosition(); setOpen(value => !value); }}
      >
        {saving ? <Loader2 size={9} className="spin" /> : null}
        {body}
        <ChevronDown size={9} aria-hidden="true" />
      </button>
      {open && position && createPortal(
        <span ref={popoverRef} className="kr-agent-switch-popover" role="dialog" aria-label={ariaLabel} style={position}>
          {catalogError && <span role="status" className="kr-agent-switch-status">{t('modelCatalog.loadError')}</span>}
          <span role="menu" aria-label={ariaLabel} className="kr-agent-switch-results">
            <button
              type="button"
              role="menuitem"
              className="kr-agent-switch-option"
              data-testid="decision-option-default"
              data-current={isDefault}
              disabled={saving || isDefault}
              onClick={() => void select({ model: null, connectionId: null })}
            >
              <span>{t('decisionPicker.default', configured ?? t('decisionPicker.notConfigured'))}</span>
              {isDefault && <Check size={10} aria-hidden="true" />}
            </button>
            {options.map(option => {
              const next = { model: option.model, connectionId: option.connectionId };
              const current = sameDecisionChoice(next, choice);
              return (
                <button
                  key={`${option.connectionId ?? ''}:${option.model}`}
                  type="button"
                  role="menuitem"
                  className="kr-agent-switch-option"
                  data-testid={`decision-option-${option.model}`}
                  data-current={current}
                  disabled={saving || current || option.unavailable}
                  title={option.unavailable ? t('modelCatalog.unavailable') : option.model}
                  onClick={() => void select(next)}
                >
                  <span>{option.label}</span>
                  <span className="kr-decision-transport" data-transport={option.transport}>
                    {t(`decisionPicker.transport.${option.transport}`)}
                  </span>
                  {current && <Check size={10} aria-hidden="true" />}
                </button>
              );
            })}
            {missing && choice.model && (
              <button type="button" role="menuitem" className="kr-agent-switch-option" data-current="true" disabled>
                <span>{choice.model}</span>
                <span className="kr-decision-missing"><AlertTriangle size={10} aria-hidden="true" /> {t('decisionPicker.missing')}</span>
              </button>
            )}
            {!options.length && !missing && (
              <span role="status" className="kr-agent-switch-status">{t('decisionPicker.none')}</span>
            )}
          </span>
        </span>,
        document.body,
      )}
    </span>
  );
}
