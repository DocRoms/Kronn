/** A refused API request, with the backend's stable error category when it sent one. */
export class ApiRequestError extends Error {
  constructor(message: string, readonly code?: string) {
    super(message);
    this.name = 'ApiRequestError';
  }
}
