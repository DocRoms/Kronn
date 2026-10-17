/**
 * KT-1324 — a launch stops the agents a previous audit run left, and refuses
 * when it cannot tell that none is still running because the process table
 * could not be read. That refusal (and only that one: a process that was seen
 * is stopped by hand) is lifted by the human's word, sent as
 * `accept_unverified_agents`. The backend names the flag in that one message.
 */
export function isUnverifiedAgentsRefusal(message: string | null | undefined): boolean {
  return !!message && message.includes('accept_unverified_agents');
}
