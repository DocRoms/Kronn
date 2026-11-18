/**
 * KT-1446 — what an import does with each custom skill its file carries:
 * reuse the identical local skill, create a missing one, or, for a skill of
 * the same name with other content, the importer's choice (a copy by default).
 * KT-1843 — and with each custom directive and profile: the local one that
 * says the same thing is reused, another is created; nothing to choose.
 */
import type { BindingImportDecision, SkillConflictResolution, SkillImportChoice, SkillImportDecision } from '../types/generated';

export function SkillImportDecisions({ decisions, bindings = [], choices, onChoose, missing = [], disabled, t }: {
  decisions: SkillImportDecision[];
  /** Bundled directives and profiles; absent from an older preview. */
  bindings?: BindingImportDecision[];
  choices: SkillImportChoice[];
  onChoose: (next: SkillImportChoice[]) => void;
  /** Skills the steps name that neither this Kronn nor the file has. */
  missing?: string[];
  disabled?: boolean;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  if (decisions.length === 0 && missing.length === 0 && bindings.length === 0) return null;
  const choose = (sourceId: string, resolution: SkillConflictResolution) => onChoose([
    ...choices.filter(choice => choice.source_id !== sourceId),
    { source_id: sourceId, resolution },
  ]);
  return (
    <div className="wf-import-skills mt-4" data-testid="import-skills">
      {decisions.length > 0 && <>
        <strong className="text-sm">{t('imp.skills.title')}</strong>
        <ul className="text-sm">
          {decisions.map(decision => {
            const chosen = choices.find(choice => choice.source_id === decision.source_id)?.resolution
              ?? decision.resolution ?? 'create_copy';
            return (
              <li key={decision.source_id} data-skill={decision.source_id}>
                <strong>{decision.name}</strong>{' '}
                <span className="text-muted">{t(`imp.skills.action.${decision.action}`)}</span>
                {decision.action === 'conflict' && (
                  <select
                    className="wf-select text-sm"
                    aria-label={t('imp.skills.choice', decision.name)}
                    value={chosen}
                    disabled={disabled}
                    onChange={event => choose(decision.source_id, event.target.value as SkillConflictResolution)}
                  >
                    <option value="create_copy">{t('imp.skills.createCopy')}</option>
                    <option value="use_existing">{t('imp.skills.useExisting')}</option>
                  </select>
                )}
              </li>
            );
          })}
        </ul>
      </>}
      {bindings.length > 0 && <>
        <strong className="text-sm">{t('imp.bindings.title')}</strong>
        <ul className="text-sm" data-testid="import-bindings">
          {bindings.map(binding => (
            <li key={`${binding.kind}:${binding.source_id}`} data-binding={binding.source_id}>
              <span className="text-muted">{t(`imp.bindings.kind.${binding.kind}`)}</span>{' '}
              <strong>{binding.name}</strong>{' '}
              <span className="text-muted">{t(`imp.bindings.action.${binding.action === 'reuse' ? 'reuse' : 'create'}`)}</span>
            </li>
          ))}
        </ul>
      </>}
      {missing.length > 0 && (
        <p className="text-sm text-muted" role="note" data-testid="import-missing-skills">
          {t('imp.skills.missing', missing.join(', '))}
        </p>
      )}
    </div>
  );
}
