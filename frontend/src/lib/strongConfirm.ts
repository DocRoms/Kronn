// KT-1496 — a route that returns a secret or destroys data asks for the strong
// confirmation when one is enrolled. The server answers 403 with the stable
// code below; the screen asks for the code inline, exchanges it for a
// short-lived single-use grant bound to that one call (`POST /api/trust/grants`),
// then repeats the very same request with the grant in `X-Kronn-Grant`. A
// second protected action asks again.
import { ApiRequestError } from './apiRequestError';

export const STRONG_CONFIRMATION_REQUIRED = 'strong_confirmation_required';
export const CONFIRM_HEADER = 'X-Kronn-Confirm';
export const GRANT_HEADER = 'X-Kronn-Grant';
/** Wrong codes before the caller gives up (the server locks at five anyway). */
const MAX_ASKS = 5;

export interface StrongPrompt {
  method: string;
  path: string;
  /** Why the previous code was refused, when this is a retry. */
  message: string | null;
  answer: (code: string | null) => void;
}

type Listener = () => void;
type Minter = (method: string, path: string, code: string) => Promise<string>;

const listeners = new Set<Listener>();
const waiting: StrongPrompt[] = [];
let current: StrongPrompt | null = null;
let mint: Minter | null = null;

/** The request layer registers how a code becomes a grant (it owns the auth headers). */
export function registerGrantMinter(minter: Minter) {
  mint = minter;
}

function advance() {
  current = waiting.shift() ?? null;
  listeners.forEach(listener => listener());
}

/** What the dialog shows now: one prompt at a time. */
export function currentStrongPrompt(): StrongPrompt | null {
  return current;
}

export function subscribeStrongPrompt(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Ask the owner for the code of `method path`; `null` when they decline. */
export function askStrongCode(method: string, path: string, message: string | null): Promise<string | null> {
  return new Promise(resolve => {
    const prompt: StrongPrompt = {
      method,
      path,
      message,
      answer: code => {
        resolve(code);
        advance();
      },
    };
    if (current) {
      waiting.push(prompt);
    } else {
      current = prompt;
      listeners.forEach(listener => listener());
    }
  });
}

export const isStrongRequired = (code: string | undefined | null): boolean =>
  code === STRONG_CONFIRMATION_REQUIRED;

/** The path of a call without its query: what a grant is bound to. */
const callPath = (path: string) => path.split('?')[0];

/** Ask for the code until it yields a grant (or, with no minter, a code header).
 *  `null` when the owner declines or runs out of tries. */
async function proofHeaders(method: string, path: string, message: string | null): Promise<Record<string, string> | null> {
  let shown = message;
  for (let asked = 0; asked < MAX_ASKS; asked += 1) {
    const code = await askStrongCode(method, path, shown);
    if (code === null) return null;
    if (!mint) return { [CONFIRM_HEADER]: code };
    try {
      return { [GRANT_HEADER]: await mint(method, callPath(path), code) };
    } catch (error) {
      shown = error instanceof Error ? error.message : String(error);
    }
  }
  return null;
}

/** Run a request, and when the server asks for the strong confirmation, ask the
 *  owner for the code and run it again with the grant it buys. `path` is the
 *  `/api/…` path of the call; `attempt` receives the extra headers to add. */
export async function withStrongConfirm<T>(
  method: string,
  path: string,
  attempt: (extraHeaders: Record<string, string>) => Promise<T>,
): Promise<T> {
  let headers: Record<string, string> = {};
  for (let asked = 0; ; asked += 1) {
    try {
      return await attempt(headers);
    } catch (error) {
      if (!(error instanceof ApiRequestError) || !isStrongRequired(error.code) || asked >= MAX_ASKS) {
        throw error;
      }
      const granted = await proofHeaders(method, path, Object.keys(headers).length > 0 ? error.message : null);
      if (granted === null) throw error;
      headers = granted;
    }
  }
}

/** `fetch` for the calls that read the raw response (downloads, uploads). */
export async function strongFetch(url: string, init: RequestInit & { method?: string }, path: string): Promise<Response> {
  const method = init.method ?? 'GET';
  const callWith = (extra: Record<string, string>) =>
    fetch(url, { ...init, headers: { ...(init.headers as Record<string, string> | undefined), ...extra } });
  let response = await callWith({});
  for (let asked = 0; asked < MAX_ASKS; asked += 1) {
    if (response.status !== 403 && response.status !== 429) return response;
    const body = await response.clone().json().catch(() => null) as { error_code?: string; error?: string } | null;
    if (!isStrongRequired(body?.error_code)) return response;
    const granted = await proofHeaders(method, path, asked > 0 ? body?.error ?? null : null);
    if (granted === null) return response;
    response = await callWith(granted);
  }
  return response;
}
