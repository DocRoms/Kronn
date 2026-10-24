// KT-1448 — named workflow refs: the rules the backend applies (`workflows::refs`).
import type { ResourceRefKind } from '../types/generated';

export const REF_KINDS: ResourceRefKind[] = ['page', 'workflow', 'quick_prompt', 'quick_api', 'quick_exec', 'skill'];

export const isValidRefName = (name: string) => /^[a-z][a-z0-9_]{0,40}$/.test(name);
export const isSafeRefId = (id: string) => /^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/.test(id);

export type RefOptions = Record<ResourceRefKind, { id: string; label: string }[]>;
