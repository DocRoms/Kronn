import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';

/** Validate destinations before any credential enrolment/rotation request. */
export function validatePublicationSandbox({ backendUrl, devPort, config, pid, listeners }) {
  const backend = new URL(backendUrl);
  const backendPort = Number(backend.port);
  const frontendPort = Number(devPort);
  const allowedPort = value => Number.isInteger(value) && value >= 1024 && value <= 65535
    && ![3140, 5173].includes(value);
  if (backend.protocol !== 'http:' || !['127.0.0.1', 'localhost'].includes(backend.hostname)
    || backend.username || backend.password || backend.pathname !== '/' || backend.search || backend.hash
    || !allowedPort(backendPort) || !allowedPort(frontendPort) || backendPort === frontendPort) {
    throw new Error('publication E2E requires distinct, non-developer loopback ports');
  }
  const server = config.match(/(?:^|\n)\[server\]\s*\n([\s\S]*?)(?=\n\[|$)/)?.[1] ?? '';
  if (!config.startsWith('# Written by scripts/e2e-sandbox-backend.sh.')
    || !/^host = "127\.0\.0\.1"$/m.test(server)
    || Number(server.match(/^port = (\d+)$/m)?.[1]) !== backendPort) {
    throw new Error('publication E2E backend URL does not match its owned configuration');
  }
  const owner = pid.trim();
  const actual = [...new Set(listeners.trim().split(/\s+/))];
  if (!/^[1-9]\d*$/.test(owner) || actual.length !== 1 || actual[0] !== owner) {
    throw new Error('publication E2E backend listener is not the process recorded by its launcher');
  }
}

export function assertPublicationSandbox(environment = process.env) {
  const directory = environment.KRONN_SANDBOX_DIR ?? '';
  if (!directory || !isAbsolute(directory)) throw new Error('an absolute KRONN_SANDBOX_DIR is required');
  const backendUrl = environment.KRONN_BACKEND_URL ?? '';
  const port = new URL(backendUrl).port;
  const config = readFileSync(join(directory, 'config.toml'), 'utf8');
  const pid = readFileSync(join(directory, 'backend.pid'), 'utf8');
  const listeners = execFileSync('lsof', ['-nP', `-iTCP:${port}`, '-sTCP:LISTEN', '-t'], {
    encoding: 'utf8', timeout: 5000, stdio: ['ignore', 'pipe', 'pipe'],
  });
  validatePublicationSandbox({ backendUrl, devPort: environment.VITE_DEV_PORT, config, pid, listeners });
}
