// The backend the specs talk to directly. A task worker's lane names its own
// backend in KRONN_VITE_BACKEND_URL; KRONN_BACKEND_URL there is the host Kronn
// its MCP bridge uses, which a spec must never drive.
export const E2E_BACKEND_URL =
  process.env.KRONN_VITE_BACKEND_URL ?? process.env.KRONN_BACKEND_URL ?? 'http://localhost:3140';
