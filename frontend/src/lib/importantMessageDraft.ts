import type { ImportantCategory } from '../types/generated';

export type FormCategory = 'information' | 'decision' | 'blocking';
type FormState = 'editing' | 'preparing' | 'sending' | 'refused' | 'text-only' | 'uncertain' | 'cancelled';
export interface Draft {
  open: boolean;
  content: string;
  category: FormCategory;
  taskRef: string;
  factId: string;
  clientMessageId: string;
  submitted?: string;
  state: FormState;
}

// Text survives room/settings navigation, not browser reload. Authority is
// intentionally absent from this store and from every persisted browser key.
const drafts = new Map<string, Draft>();
const listeners = new Map<string, Set<() => void>>();
export const pending = new Map<string, { controller: AbortController; sending: boolean }>();

export function newId(): string {
  if (typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export function emptyDraft(): Draft {
  return { open: false, content: '', category: 'information', taskRef: '', factId: newId(), clientMessageId: newId(), submitted: undefined, state: 'editing' };
}

export function readDraft(room: string): Draft {
  let draft = drafts.get(room);
  if (!draft) { draft = emptyDraft(); drafts.set(room, draft); }
  return draft;
}

export function updateDraft(room: string, patch: Partial<Draft>) {
  drafts.set(room, { ...readDraft(room), ...patch });
  listeners.get(room)?.forEach(listener => listener());
}

/** Explicit deletion/reset; never called just because a component unmounts. */
export function discardImportantDraft(room: string) {
  pending.get(room)?.controller.abort();
  pending.delete(room);
  drafts.delete(room);
  listeners.get(room)?.forEach(listener => listener());
}

export function takeScalars(value: string, limit: number): string {
  return Array.from(value.replace(/[\uD800-\uDFFF]/gu, '\uFFFD')).slice(0, limit).join('');
}

export function importantMessageFence(content: string, category: FormCategory, taskRef: string, dedupKey: string, impact: string): string {
  const message = takeScalars(content.trim(), 2500);
  const scalars = Array.from(message);
  const cardCategory: ImportantCategory = category === 'blocking' ? 'blocking_alert' : category;
  const spec = {
    version: 1, category: cardCategory, dedup_key: `human:${dedupKey}`,
    title: takeScalars(message.split(/\r?\n/, 1)[0], 200),
    highlight: takeScalars(message, 500),
    context: scalars.length > 500 ? scalars.slice(500).join('') : undefined,
    impact, action_required: { required: false },
    references: taskRef ? { task_ref: taskRef } : {},
  };
  return `\`\`\`kronn-important\n${JSON.stringify(spec)}\n\`\`\``;
}

export function subscribeImportantDraft(room: string, listener: () => void) {
  const set = listeners.get(room) ?? new Set<() => void>();
  set.add(listener);
  listeners.set(room, set);
  return () => {
    set.delete(listener);
    if (!set.size) listeners.delete(room);
  };
}
