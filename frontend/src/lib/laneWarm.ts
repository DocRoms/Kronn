// KT-1742 — the lane warm-up settings, as a form draft and back.
//
// One shape serves Settings > Workers (the global entry) and a project's own
// override. A field left on its default is simply absent from the stored entry:
// « detected » for the global one, « inherit » for a project.
import type { LaneWarmConfig } from '../types/generated';

/** Default bound of a lane's first build, in minutes (server-side default). */
export const DEFAULT_FIRST_BUILD_TIMEOUT_MIN = 60;
/** Largest bound the server accepts (a day). */
export const MAX_FIRST_BUILD_TIMEOUT_MIN = 1440;

export type SeedAlignmentChoice = '' | 'on' | 'off';
/** `inherit`: detected (global) or the global entry (project); `none`: no variable at all. */
export type EnvMode = 'inherit' | 'custom' | 'none';

export interface LaneWarmDraft {
  seedAlignment: SeedAlignmentChoice;
  envMode: EnvMode;
  /** One `NAME=value` per line, used while `envMode` is `custom`. */
  envText: string;
  command: string;
  /** Minutes; blank = the default. */
  timeout: string;
  /** KT-1742 — Rust roots to warm, one directory per line; blank = every detected root (a project's own). */
  rootsText: string;
}

export const EMPTY_LANE_WARM_DRAFT: LaneWarmDraft = {
  seedAlignment: '',
  envMode: 'inherit',
  envText: '',
  command: '',
  timeout: '',
  rootsText: '',
};

export function draftOf(config: LaneWarmConfig | null | undefined): LaneWarmDraft {
  if (!config) return EMPTY_LANE_WARM_DRAFT;
  const env = config.compiler_env;
  return {
    seedAlignment: config.seed_alignment === undefined || config.seed_alignment === null
      ? ''
      : config.seed_alignment ? 'on' : 'off',
    envMode: env === undefined || env === null ? 'inherit' : env.length === 0 ? 'none' : 'custom',
    envText: (env ?? []).join('\n'),
    command: config.first_build_command ?? '',
    timeout: config.first_build_timeout_min === undefined || config.first_build_timeout_min === null
      ? ''
      : String(config.first_build_timeout_min),
    rootsText: (config.rust_roots ?? []).join('\n'),
  };
}

export function envLines(text: string): string[] {
  return text.split('\n').map(line => line.trim()).filter(line => line.length > 0);
}

/** The Rust roots of a draft: one directory per line, `./` and a trailing `/` dropped. */
export function rootLines(text: string): string[] {
  const roots: string[] = [];
  for (const line of envLines(text)) {
    const root = line.replace(/^\.\//, '').replace(/\/+$/, '');
    if (root !== '' && !roots.includes(root)) roots.push(root);
  }
  return roots;
}

/** The first problem of a draft, as the part of the form it is in. */
export function draftProblem(draft: LaneWarmDraft): 'timeout' | 'env' | 'roots' | null {
  const raw = draft.timeout.trim();
  if (raw !== '') {
    const minutes = Number(raw);
    if (!Number.isInteger(minutes) || minutes < 1 || minutes > MAX_FIRST_BUILD_TIMEOUT_MIN) return 'timeout';
  }
  if (draft.envMode === 'custom') {
    const entries = envLines(draft.envText);
    if (entries.length === 0) return 'env';
    if (!entries.every(entry => /^[A-Za-z_][A-Za-z0-9_]*=/.test(entry))) return 'env';
  }
  // A root is a directory inside the repository: relative, no `..`.
  if (rootLines(draft.rootsText).some(root => root.startsWith('/') || root.split('/').includes('..'))) return 'roots';
  return null;
}

/** The entry to store; an untouched field is left out, so it keeps inheriting. */
export function configOf(draft: LaneWarmDraft): LaneWarmConfig {
  const config: LaneWarmConfig = {};
  if (draft.seedAlignment !== '') config.seed_alignment = draft.seedAlignment === 'on';
  if (draft.envMode === 'none') config.compiler_env = [];
  if (draft.envMode === 'custom') config.compiler_env = envLines(draft.envText);
  const command = draft.command.trim();
  if (command !== '') config.first_build_command = command;
  const timeout = draft.timeout.trim();
  if (timeout !== '') config.first_build_timeout_min = Number(timeout);
  const roots = rootLines(draft.rootsText);
  if (roots.length > 0) config.rust_roots = roots;
  return config;
}

export function sameDraft(a: LaneWarmDraft, b: LaneWarmDraft): boolean {
  return a.seedAlignment === b.seedAlignment
    && a.envMode === b.envMode
    && (a.envMode !== 'custom' || envLines(a.envText).join('\n') === envLines(b.envText).join('\n'))
    && a.command.trim() === b.command.trim()
    && a.timeout.trim() === b.timeout.trim()
    && rootLines(a.rootsText).join('\n') === rootLines(b.rootsText).join('\n');
}
