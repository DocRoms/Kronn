import type { McpConfigDisplay, StepResult } from '../types/generated';

/** Closes the output of an API step whose failure is a refused credential
 *  (`AUTH_REFUSED_SIGNAL`, backend `workflows/api_auth_failure.rs`, KT-1524). */
export const AUTH_REFUSED_SIGNAL = '[SIGNAL: auth_refused]';

export interface StepAuthRefusal {
  stepName: string;
  /** The plain sentence the backend leads the step output with. */
  sentence: string;
}

/** The connections whose credentials the provider currently refuses. */
export function connectionsNeedingAttention(configs: McpConfigDisplay[]): McpConfigDisplay[] {
  return configs.filter(config => config.auth_failure != null);
}

/** The plain sentence of a step output closed by the auth-refused signal, or
 *  `null` for any other output. */
export function authRefusalSentence(output: string): string | null {
  if (!output.includes(AUTH_REFUSED_SIGNAL)) return null;
  const line = output.split('\n').find(candidate => candidate.startsWith('Connection "'));
  return line?.trim() ?? null;
}

/** Steps of a run that hit a refused credential: failed ones, and a
 *  collection that went on with the sources that still answered. */
export function runAuthRefusals(steps: StepResult[]): StepAuthRefusal[] {
  return steps.flatMap(step => {
    const sentence = authRefusalSentence(step.output);
    return sentence ? [{ stepName: step.step_name, sentence }] : [];
  });
}
