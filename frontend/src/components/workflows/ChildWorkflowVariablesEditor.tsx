import { useEffect, useState } from 'react';
import { useT } from '../../lib/I18nContext';
import { workflows as workflowsApi } from '../../lib/api';
import type { PromptVariable } from '../../types/generated';

interface Props {
  /** The SubWorkflow / TriggerWorkflow target (`sub_workflow_id`). */
  targetId: string | null | undefined;
  value: Record<string, string>;
  onChange: (next: Record<string, string>) => void;
}

function takesInput(variable: PromptVariable): boolean {
  return (variable.source ?? 'user_input') === 'user_input' || variable.allow_manual_override;
}

/** Maps the target workflow's launch variables to templates rendered in the
 *  parent run (`sub_workflow_variables`). */
export function ChildWorkflowVariablesEditor({ targetId, value, onChange }: Props) {
  const { t } = useT();
  const [fetched, setFetched] = useState<{ id: string; variables: PromptVariable[] | null } | null>(null);
  // A `@bundle:` child does not exist yet: nothing to read its variables from.
  const readable = !!targetId && !targetId.startsWith('@bundle:');

  useEffect(() => {
    if (!readable || !targetId) return;
    let alive = true;
    workflowsApi.get(targetId)
      .then(workflow => { if (alive) setFetched({ id: targetId, variables: workflow.variables ?? [] }); })
      .catch(() => { if (alive) setFetched({ id: targetId, variables: null }); });
    return () => { alive = false; };
  }, [readable, targetId]);
  // Only the answer about the current target counts, never a previous one's.
  const declared = readable && fetched?.id === targetId ? fetched.variables : null;

  const set = (name: string, template: string) => {
    const next = { ...value };
    if (template === '') delete next[name];
    else next[name] = template;
    onChange(next);
  };
  const declaredNames = new Set((declared ?? []).map(variable => variable.name));
  const extra = Object.keys(value).filter(name => !declaredNames.has(name)).sort();

  if (!targetId) return null;
  return (
    <div className="wf-child-variables mt-2">
      <label className="text-xs text-muted mb-1">{t('wiz.childVariables')}</label>
      {declared?.length === 0 && extra.length === 0 && (
        <p className="text-2xs text-ghost">{t('wiz.childVariablesNone')}</p>
      )}
      {declared?.map(variable => (
        <div key={variable.name} className="flex-row gap-3 mb-2">
          <code className="text-xs" style={{ minWidth: 120 }}>
            {variable.name}
            {variable.required && (variable.source ?? 'user_input') === 'user_input' && (
              <span className="wf-required"> *</span>
            )}
          </code>
          {takesInput(variable) ? (
            <input
              className="wf-input text-sm"
              value={value[variable.name] ?? ''}
              placeholder={`{{${variable.name}}}`}
              onChange={e => set(variable.name, e.target.value)}
              aria-label={t('wiz.childVariableFor', variable.name)}
            />
          ) : (
            <span className="text-2xs text-ghost">{t('wiz.childVariableResolvedByChild')}</span>
          )}
        </div>
      ))}
      {extra.map(name => (
        <div key={name} className="flex-row gap-3 mb-2">
          <code className="text-xs" style={{ minWidth: 120 }} data-invalid={declared !== null}>{name}</code>
          <input
            className="wf-input text-sm"
            value={value[name]}
            onChange={e => set(name, e.target.value)}
            aria-label={t('wiz.childVariableFor', name)}
          />
          <button
            type="button"
            className="wf-icon-btn text-xs"
            onClick={() => set(name, '')}
            aria-label={t('wiz.childVariableRemove', name)}
          >
            ×
          </button>
        </div>
      ))}
      {declared !== null && extra.length > 0 && (
        <p className="text-2xs text-error">{t('wiz.childVariablesUndeclared')}</p>
      )}
      <p className="text-2xs text-ghost">{t('wiz.childVariablesHint')}</p>
    </div>
  );
}
