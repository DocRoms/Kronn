import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const desktopDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repoDir = resolve(desktopDir, '..');
const venvDir = join(repoDir, 'target', 'docs-sidecar-build-venv');
const isWindows = process.platform === 'win32';
const venvPython = isWindows
  ? join(venvDir, 'Scripts', 'python.exe')
  : join(venvDir, 'bin', 'python');

function run(program, args, options = {}) {
  const result = spawnSync(program, args, {
    cwd: repoDir,
    env: process.env,
    stdio: 'inherit',
    ...options,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

function findPython() {
  const candidates = isWindows
    ? [['python', []], ['py', ['-3']]]
    : [['python3', []], ['python', []]];
  for (const [program, prefix] of candidates) {
    const result = spawnSync(program, [...prefix, '--version'], { stdio: 'ignore' });
    if (result.status === 0) return { program, prefix };
  }
  throw new Error('Python 3.10+ is required to build the bundled document exporter.');
}

if (!existsSync(venvPython)) {
  const python = findPython();
  run(python.program, [...python.prefix, '-m', 'venv', venvDir]);
}

run(venvPython, [
  '-m',
  'pip',
  'install',
  '--disable-pip-version-check',
  '--quiet',
  '--upgrade',
  join(repoDir, 'backend', 'sidecars', 'docs') + '[bundle]',
]);
run(venvPython, [
  join(repoDir, 'backend', 'sidecars', 'docs', 'build_bundle.py'),
  '--output',
  join(repoDir, 'desktop', 'src-tauri', 'resources', 'docs-sidecar'),
]);
