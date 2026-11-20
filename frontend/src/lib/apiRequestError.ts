/**
 * A refused API request, with the backend's stable error category when it
 * sent one and the answer itself, for a refusal that carries more than text.
 */
export class ApiRequestError extends Error {
  constructor(message: string, readonly code?: string, readonly body?: unknown) {
    super(message);
    this.name = 'ApiRequestError';
  }
}
