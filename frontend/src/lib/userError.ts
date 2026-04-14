// Transforms raw error objects/strings into user-friendly messages.
//
// Before: toast(String(e), 'error')  →  "TypeError: Cannot read property 'x' of undefined"
// After:  toast(userError(e), 'error')  →  "Une erreur est survenue. Réessayez."
//
// The function tries to extract a meaningful message from the error:
// 1. If it's an ApiResponse error string from the backend (e.g. "DB error: ..."), keep it
// 2. If it's a network error, translate to plain language
// 3. If it's gibberish (stack trace, TypeError), fall back to generic message

/** Known backend error prefixes that are already human-readable. */
const BACKEND_PREFIXES = [
  'Project path',
  'Directory does not exist',
  'A project with this path',
  'Invalid',
  'Workflow',
  'Quick prompt',
  'Recovery failed',
  'Discussion',
  'Unsupported',
  'Message too long',
  'partial_pending',
];

/** Extract a user-friendly error message from any thrown value. */
export function userError(e: unknown, fallback?: string): string {
  const raw = extractMessage(e);
  if (!raw) return fallback ?? genericError();

  // Backend ApiResponse errors are already readable
  if (BACKEND_PREFIXES.some(p => raw.startsWith(p))) return raw;

  // Network errors
  if (raw.includes('Failed to fetch') || raw.includes('NetworkError') || raw.includes('ERR_CONNECTION')) {
    return 'Impossible de contacter le serveur. Vérifiez votre connexion.';
  }
  if (raw.includes('timeout') || raw.includes('Timeout')) {
    return "La requête a pris trop de temps. Réessayez.";
  }
  if (raw.includes('413') || raw.includes('too large')) {
    return 'Le fichier est trop volumineux.';
  }
  if (raw.includes('401') || raw.includes('Unauthorized')) {
    return "Session expirée. Rechargez la page.";
  }

  // If it looks like a backend message (short, no stack trace), keep it
  if (raw.length < 200 && !raw.includes('\n') && !raw.includes('at ')) {
    return raw;
  }

  // Gibberish — generic fallback
  return fallback ?? genericError();
}

function genericError(): string {
  return 'Une erreur est survenue. Réessayez.';
}

function extractMessage(e: unknown): string {
  if (!e) return '';
  if (typeof e === 'string') return e.trim();
  if (e instanceof Error) return e.message.trim();
  if (typeof e === 'object' && e !== null) {
    const obj = e as Record<string, unknown>;
    if (typeof obj.message === 'string') return obj.message.trim();
    if (typeof obj.error === 'string') return obj.error.trim();
  }
  try { return String(e).trim(); } catch { return ''; }
}
