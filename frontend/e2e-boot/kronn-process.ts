import { spawn, type ChildProcess } from 'node:child_process';
import { closeSync, mkdirSync, openSync, readFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { join } from 'node:path';

/**
 * A real `kronn` binary on its own data directory and port, started and
 * stopped by the spec. Nothing is stubbed: this is the process a user runs, so
 * a boot that stops on a startup error (KT-1625: os error 5 on Windows) fails
 * here before any page opens.
 */

/** Cold start on a CI runner includes the first-run key and config writes. */
const BOOT_TIMEOUT_MS = 120_000;

export function kronnBinary(): string {
  const bin = process.env.KRONN_BIN;
  if (!bin) throw new Error('KRONN_BIN must name the kronn executable to boot');
  return bin;
}

export function frontendDist(): string {
  const dist = process.env.KRONN_FRONTEND_DIST;
  if (!dist) throw new Error('KRONN_FRONTEND_DIST must name the built frontend (frontend/dist)');
  return dist;
}

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as { port: number };
      server.close(() => resolve(port));
    });
  });
}

export class KronnInstance {
  private child: ChildProcess | null = null;
  private exit: { code: number | null; signal: NodeJS.Signals | null } | null = null;
  private spawnError: Error | null = null;
  port = 0;

  /** `logFile` is appended to by every start, so a restart keeps the first run's log. */
  constructor(
    readonly dataDir: string,
    readonly logFile: string,
  ) {}

  get url(): string {
    return `http://127.0.0.1:${this.port}`;
  }

  /** Boots Kronn and resolves once `/api/health` answers 200. */
  async start(): Promise<void> {
    mkdirSync(this.dataDir, { recursive: true });
    this.port = await freePort();
    this.exit = null;
    this.spawnError = null;
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      KRONN_DATA_DIR: this.dataDir,
      KRONN_PORT: String(this.port),
      KRONN_HOST: '127.0.0.1',
      KRONN_FRONTEND_DIST: frontendDist(),
    };
    // A first-run scan walks the home directory: point it at an empty one when asked.
    if (process.env.KRONN_E2E_HOME) {
      env.HOME = process.env.KRONN_E2E_HOME;
      env.USERPROFILE = process.env.KRONN_E2E_HOME;
    }
    const log = openSync(this.logFile, 'a');
    try {
      this.child = spawn(kronnBinary(), [], { env, stdio: ['ignore', log, log], windowsHide: true });
    } finally {
      closeSync(log);
    }
    const child = this.child;
    child.once('exit', (code, signal) => {
      this.exit = { code, signal };
    });
    child.once('error', (error) => {
      this.spawnError = error;
    });
    await this.waitForHealth();
  }

  private async waitForHealth(): Promise<void> {
    const deadline = Date.now() + BOOT_TIMEOUT_MS;
    while (Date.now() < deadline) {
      if (this.spawnError) throw new Error(`kronn could not be started: ${this.spawnError.message}`);
      if (this.exit) {
        throw new Error(
          `kronn exited during boot (code ${this.exit.code}, signal ${this.exit.signal}):\n${this.logTail()}`,
        );
      }
      try {
        const response = await fetch(`${this.url}/api/health`);
        if (response.status === 200) return;
      } catch {
        // Not listening yet.
      }
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    throw new Error(`kronn did not answer /api/health within ${BOOT_TIMEOUT_MS} ms:\n${this.logTail()}`);
  }

  logTail(lines = 40): string {
    try {
      return readFileSync(this.logFile, 'utf8').split('\n').slice(-lines).join('\n');
    } catch {
      return '(no log)';
    }
  }

  /** Ends the process and waits until it has released the database. */
  async stop(): Promise<void> {
    const child = this.child;
    this.child = null;
    if (!child || this.exit) return;
    const gone = new Promise<void>((resolve) => child.once('exit', () => resolve()));
    child.kill();
    await gone;
  }
}

export function dataFile(instance: KronnInstance, ...parts: string[]): string {
  return join(instance.dataDir, ...parts);
}
