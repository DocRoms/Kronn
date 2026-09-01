import type { PromptVariable } from '../types/generated';

export function promptVariableEffectiveValue(variable: PromptVariable, value?: string): string {
  if (value !== undefined) return value;
  return variable.control?.type === 'select' ? variable.control.default_value ?? '' : '';
}
