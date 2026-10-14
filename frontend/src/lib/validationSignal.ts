/**
 * KT-1122 — the validation terminal signal, parsed ONE way.
 *
 * Mirror of the backend's `ends_with_terminal_signal`
 * (`backend/src/api/discussions/mod.rs`): the signal is the LAST line of the
 * message, emphasis (`**`, backticks, `_`) tolerated, case-insensitive. A
 * message that merely mentions the marker mid-sentence does not count.
 *
 * Whether a validation is *finished* is the server's verdict
 * (`ValidationState.finished`, durable, never undone by later messages); this
 * helper only serves per-message affordances, where there is no server round
 * trip, and must stay on the same rule.
 */
export const VALIDATION_SIGNAL = 'KRONN:VALIDATION_COMPLETE';

const EMPHASIS = /^[*`_]+|[*`_]+$/g;

export function endsWithValidationSignal(content: string): boolean {
  const lines = content.trimEnd().split('\n');
  const last = lines[lines.length - 1] ?? '';
  return last.trim().replace(EMPHASIS, '').toUpperCase() === VALIDATION_SIGNAL;
}
