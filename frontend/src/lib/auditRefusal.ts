/**
 * KT-1324 — a launch stops the agents a previous audit run left, and refuses
 * when it cannot tell that none is still running because the process table
 * could not be read. That refusal (and only that one: a process that was seen
 * is stopped by hand, `agents_still_running`) is lifted by the human's word,
 * sent as `accept_unverified_agents`. The backend marks it with the typed
 * `error_code` of its SSE `error` event; the text is for the human only.
 */
export const AGENTS_UNVERIFIED_CODE = 'agents_unverified';

export function isUnverifiedAgentsRefusal(errorCode: string | null | undefined): boolean {
  return errorCode === AGENTS_UNVERIFIED_CODE;
}
