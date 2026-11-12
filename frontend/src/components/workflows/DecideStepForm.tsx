import { Compass, Plus, Trash2 } from 'lucide-react';
import type { DecideConfig, DecideKind, DecideQuestion } from '../../types/generated';
import { DECIDE_KINDS, blankDecideQuestion, decideSignals } from '../../lib/decideStep';

interface DecideStepFormProps {
  config: DecideConfig;
  /// Names of the steps before this one, for the state shortcuts.
  previousSteps: string[];
  onChange: (config: DecideConfig) => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

/** KT-1367 — the Decide step's form: what the model reads and what it answers. */
export function DecideStepForm({ config, previousSteps, onChange, t }: DecideStepFormProps) {
  const setQuestion = (index: number, patch: Partial<DecideQuestion>) =>
    onChange({ ...config, questions: config.questions.map((question, i) => i === index ? { ...question, ...patch } : question) });
  const setKind = (index: number, kind: DecideKind) => {
    const patch: Partial<DecideQuestion> = { kind, options: undefined, levels: undefined };
    if (kind === 'choice') patch.options = { a: '', b: '' };
    if (kind === 'score') patch.levels = ['', '', ''];
    setQuestion(index, patch);
  };
  const setOption = (index: number, key: string, next: { key?: string; text?: string }) => {
    const entries = Object.entries(config.questions[index].options ?? {})
      .map(([k, text]) => k === key ? [next.key ?? k, next.text ?? text] : [k, text]);
    setQuestion(index, { options: Object.fromEntries(entries) });
  };
  const signals = config.questions.flatMap(question => [...decideSignals(question), `${question.id}_unsure`]);

  return (
    <div className="wf-gate-form" data-testid="decide-form">
      <div className="wf-batch-intro">
        <Compass size={14} />
        <div>
          <strong>{t('wiz.decideTitle')}</strong>
          <p className="text-xs text-muted">{t('wiz.decideHint')}</p>
        </div>
      </div>

      <label className="text-xs text-muted mb-1" htmlFor="decide-state">{t('wiz.decideState')}</label>
      <textarea
        id="decide-state"
        className="wf-textarea text-sm mb-2"
        rows={3}
        value={config.state}
        onChange={e => onChange({ ...config, state: e.target.value })}
        placeholder="{{previous_step.output}}"
      />
      {previousSteps.length > 0 && (
        <div className="mb-2 text-xs text-ghost flex-wrap flex-row gap-1">
          {previousSteps.map(name => (
            <button key={name} type="button" className="wf-var-hint-code" onClick={() => onChange({ ...config, state: `{{steps.${name}.output}}` })}>
              {`{{steps.${name}.output}}`}
            </button>
          ))}
        </div>
      )}

      {config.questions.map((question, index) => (
        <div key={index} className="wf-condition-row mb-2" data-testid={`decide-question-${index}`}>
          <div className="flex-row gap-2 mb-1">
            <input
              className="wf-input text-sm"
              value={question.id}
              onChange={e => setQuestion(index, { id: e.target.value.toLowerCase() })}
              aria-label={t('wiz.decideQuestionId')}
              placeholder="ship"
            />
            <select
              className="wf-select text-sm"
              value={question.kind}
              onChange={e => setKind(index, e.target.value as DecideKind)}
              aria-label={t('wiz.decideKind')}
            >
              {DECIDE_KINDS.map(kind => <option key={kind} value={kind}>{t(`wiz.decideKind.${kind}`)}</option>)}
            </select>
            <button
              type="button"
              className="wf-icon-btn"
              disabled={config.questions.length === 1}
              onClick={() => onChange({ ...config, questions: config.questions.filter((_, i) => i !== index) })}
              aria-label={t('wiz.decideRemoveQuestion')}
            >
              <Trash2 size={12} />
            </button>
          </div>
          <input
            className="wf-input text-sm mb-1"
            value={question.instructions}
            onChange={e => setQuestion(index, { instructions: e.target.value })}
            aria-label={t('wiz.decideInstructions')}
            placeholder={t('wiz.decideInstructionsPlaceholder')}
          />
          {question.kind === 'choice' && Object.entries(question.options ?? {}).map(([key, text]) => (
            <div key={key} className="flex-row gap-2 mb-1">
              <input
                className="wf-input text-sm"
                value={key}
                onChange={e => setOption(index, key, { key: e.target.value.toLowerCase() })}
                aria-label={t('wiz.decideOptionId')}
              />
              <input
                className="wf-input text-sm flex-1"
                value={text}
                onChange={e => setOption(index, key, { text: e.target.value })}
                aria-label={t('wiz.decideOptionMeaning')}
                placeholder={t('wiz.decideOptionMeaning')}
              />
            </div>
          ))}
          {question.kind === 'choice' && (
            <button
              type="button"
              className="wf-add-step-btn wf-add-step-btn-inline text-xs"
              onClick={() => setQuestion(index, { options: { ...(question.options ?? {}), [`o${Object.keys(question.options ?? {}).length + 1}`]: '' } })}
            >
              <Plus size={11} /> {t('wiz.decideAddOption')}
            </button>
          )}
          {question.kind === 'score' && (question.levels ?? []).map((level, levelIndex) => (
            <input
              key={levelIndex}
              className="wf-input text-sm mb-1"
              value={level}
              onChange={e => setQuestion(index, { levels: (question.levels ?? []).map((l, j) => j === levelIndex ? e.target.value : l) })}
              aria-label={t('wiz.decideLevel', levelIndex + 1)}
              placeholder={t('wiz.decideLevel', levelIndex + 1)}
            />
          ))}
          {question.kind === 'score' && (question.levels ?? []).length < 10 && (
            <button
              type="button"
              className="wf-add-step-btn wf-add-step-btn-inline text-xs"
              onClick={() => setQuestion(index, { levels: [...(question.levels ?? []), ''] })}
            >
              <Plus size={11} /> {t('wiz.decideAddLevel')}
            </button>
          )}
        </div>
      ))}
      {config.questions.length < 64 && (
        <button
          type="button"
          className="wf-add-step-btn wf-add-step-btn-inline text-xs mb-2"
          onClick={() => onChange({ ...config, questions: [...config.questions, blankDecideQuestion(config.questions.length)] })}
        >
          <Plus size={11} /> {t('wiz.decideAddQuestion')}
        </button>
      )}

      <label className="text-xs text-muted mb-1" htmlFor="decide-min-confidence">{t('wiz.decideMinConfidence')}</label>
      <input
        id="decide-min-confidence"
        className="wf-input text-sm"
        type="number"
        min={0}
        max={1}
        step={0.05}
        value={config.min_confidence ?? ''}
        placeholder={t('wiz.decideMinConfidenceDefault')}
        onChange={e => onChange({ ...config, min_confidence: e.target.value === '' ? undefined : Number(e.target.value) })}
      />
      <p className="text-2xs text-ghost mt-1">{t('wiz.decideSignalsHint')}</p>
      <div className="flex-wrap flex-row gap-1" data-testid="decide-signals">
        {[...signals, 'low_confidence'].map(signal => <code key={signal} className="wf-var-hint-code">{signal}</code>)}
      </div>
    </div>
  );
}
