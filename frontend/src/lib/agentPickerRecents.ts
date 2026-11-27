/**
 * KT-1994 — the agent picker's « Récents »: the last choices applied through
 * it (an agent, its connection, and a mode or an exact model), most recent
 * first. Kept in localStorage under a `kronn:` key that the UI preferences
 * mirror server-side (`SYNCED_KEYS` in uiPreferences.ts), like the collection
 * favourites: a fresh origin finds them again.
 */
import { safeGetItem, safeSetItem } from './safeStorage';
import { ALL_AGENT_TYPES } from './constants';
import type { AgentType, ModelTier } from '../types/generated';

export const AGENT_PICKER_RECENTS_KEY = 'kronn:agentPicker:recents';

/** How many distinct choices the section shows (owner decision of 2026-11-21). */
export const AGENT_PICKER_RECENTS_MAX = 3;

export interface AgentPickerRecent {
  agent: AgentType;
  connectionId?: string | null;
  /** A mode; `null` is the agent's own default. Absent when `model` is set. */
  tier?: ModelTier | null;
  /** An exact model id (« Personnalisé »). */
  model?: string | null;
}

const TIERS: readonly string[] = ['economy', 'default', 'reasoning'];
const AGENTS: readonly string[] = [...ALL_AGENT_TYPES, 'Custom'];

/** Two entries are the same choice: same agent, connection, and mode or model. */
export function recentKey(entry: AgentPickerRecent): string {
  return JSON.stringify([
    entry.agent,
    entry.connectionId ?? null,
    entry.model ? null : entry.tier ?? null,
    entry.model ?? null,
  ]);
}

function normalize(value: unknown): AgentPickerRecent | null {
  if (!value || typeof value !== 'object') return null;
  const raw = value as Record<string, unknown>;
  if (typeof raw.agent !== 'string' || !AGENTS.includes(raw.agent)) return null;
  const model = typeof raw.model === 'string' && raw.model.trim() ? raw.model.trim() : null;
  const tier = typeof raw.tier === 'string' && TIERS.includes(raw.tier) ? raw.tier as ModelTier : null;
  const connectionId = typeof raw.connectionId === 'string' && raw.connectionId ? raw.connectionId : null;
  return {
    agent: raw.agent as AgentType,
    ...(connectionId ? { connectionId } : {}),
    ...(model ? { model } : { tier }),
  };
}

/** The stored choices, most recent first; anything unreadable is dropped. */
export function readAgentPickerRecents(): AgentPickerRecent[] {
  const stored = safeGetItem(AGENT_PICKER_RECENTS_KEY);
  if (!stored) return [];
  try {
    const parsed: unknown = JSON.parse(stored);
    if (!Array.isArray(parsed)) return [];
    const seen = new Set<string>();
    const out: AgentPickerRecent[] = [];
    for (const item of parsed) {
      const entry = normalize(item);
      if (!entry || seen.has(recentKey(entry))) continue;
      seen.add(recentKey(entry));
      out.push(entry);
      if (out.length === AGENT_PICKER_RECENTS_MAX) break;
    }
    return out;
  } catch {
    return [];
  }
}

/** Put a choice that was just applied first; the same choice is never listed twice. */
export function recordAgentPickerRecent(entry: AgentPickerRecent): AgentPickerRecent[] {
  const next = normalize(entry);
  if (!next) return readAgentPickerRecents();
  const key = recentKey(next);
  const list = [next, ...readAgentPickerRecents().filter(item => recentKey(item) !== key)]
    .slice(0, AGENT_PICKER_RECENTS_MAX);
  safeSetItem(AGENT_PICKER_RECENTS_KEY, JSON.stringify(list));
  return list;
}
