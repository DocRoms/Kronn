// Shared plumbing for the kept configuration-assistant conversations (KT-1111).
import { assistantConversations } from '../lib/api';
import { parseKronnApply } from '../lib/kronnApply';
import type {
  AssistantConversation,
  Workflow,
  DiscussionMessage,
  UpdateAssistantConversationRequest,
} from '../types/generated';

export const ASSISTANT_CONVERSATIONS_CHANGED = 'kronn:assistant-conversations-changed';

export function notifyAssistantConversationsChanged(): void {
  window.dispatchEvent(new Event(ASSISTANT_CONVERSATIONS_CHANGED));
}

export interface AssistantChatMessage {
  role: 'user' | 'assistant';
  text: string;
}

/** Whether the last proposal of a kept conversation was applied. */
export function proposalStatus(conversation: AssistantConversation): 'none' | 'applied' | 'pending' {
  if (!conversation.last_proposal_signature) return conversation.last_applied_signature ? 'applied' : 'none';
  return conversation.last_applied_signature === conversation.last_proposal_signature ? 'applied' : 'pending';
}

/** Signature of the last proposal in an assistant reply, if it made one. */
export function lastProposalSignature(text: string): string | null {
  const { blocks } = parseKronnApply(text);
  return blocks.length > 0 ? blocks[blocks.length - 1].signature : null;
}

/** Rebuilds the helper's chat from a stored transcript: the system prompt
 *  (first message) and tool breadcrumbs are dropped, and each user turn loses
 *  the context block the helper prepended before `questionLabels`. */
export function transcriptToChat(
  messages: DiscussionMessage[],
  questionLabels: string[],
): AssistantChatMessage[] {
  const chat: AssistantChatMessage[] = [];
  messages.forEach((message, index) => {
    if (message.channel === 'note' || message.role === 'System') return;
    if (message.role === 'User') {
      if (index === 0) return;
      chat.push({ role: 'user', text: stripContext(message.content, questionLabels) });
    } else {
      chat.push({ role: 'assistant', text: message.content });
    }
  });
  return chat;
}

function stripContext(content: string, labels: string[]): string {
  for (const label of labels) {
    if (!label) continue;
    const at = content.lastIndexOf(`\n${label}\n`);
    if (at >= 0) return content.slice(at + label.length + 2).trim();
  }
  return content;
}

function patchConversation(discussionId: string, patch: UpdateAssistantConversationRequest): void {
  assistantConversations.update(discussionId, patch)
    .then(() => notifyAssistantConversationsChanged())
    .catch(e => console.warn('[assistantConversation] update failed:', e));
}

/** Remembers the last proposal of a finished reply, if it made one. */
export function recordProposal(discussionId: string, replyText: string): void {
  const signature = lastProposalSignature(replyText);
  if (signature) patchConversation(discussionId, { last_proposal_signature: signature });
}

export function recordApplied(discussionId: string, signature: string): void {
  patchConversation(discussionId, { last_applied_signature: signature });
}

// Conversations owed to a saved object whose attachment failed, by target id.
// Kept in this browser so a later visit of that plugin or step can still
// offer and attach them; a label or a name would not prove they belong to it.
const PENDING_KEY = 'kronn:assistantPendingAttachments';

interface PendingEntry {
  id: string;
  /** The step's draft name, for an ApiCall step; null for a plugin. */
  step: string | null;
}

function readPending(): Record<string, PendingEntry[]> {
  try {
    const raw = localStorage.getItem(PENDING_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return parsed && typeof parsed === 'object' ? parsed as Record<string, PendingEntry[]> : {};
  } catch {
    return {};
  }
}

function writePending(all: Record<string, PendingEntry[]>): void {
  try {
    const kept = Object.fromEntries(Object.entries(all).filter(([, list]) => list.length > 0));
    if (Object.keys(kept).length === 0) localStorage.removeItem(PENDING_KEY);
    else localStorage.setItem(PENDING_KEY, JSON.stringify(kept));
  } catch {
    // No storage: the conversation stays in the Assistants section.
  }
}

function addPending(targetId: string, entry: PendingEntry): void {
  const all = readPending();
  const list = (all[targetId] ?? []).filter(item => item.id !== entry.id);
  all[targetId] = [...list, entry];
  writePending(all);
}

/** The conversations owed to `targetId`; for a step, only those started on
 *  one of `steps` (its draft name or its durable id). */
export function pendingFor(targetId: string, steps: (string | null)[] = [null]): string[] {
  return (readPending()[targetId] ?? [])
    .filter(entry => steps.includes(entry.step))
    .map(entry => entry.id);
}

/** Forgets a conversation once it is attached. */
export function settlePending(targetId: string, discussionId: string): void {
  const all = readPending();
  if (!all[targetId]) return;
  all[targetId] = all[targetId].filter(entry => entry.id !== discussionId);
  writePending(all);
}

/** Step name → durable id of a saved workflow; null when the workflow could
 *  not be read or the step has no id, so nothing is filed under a name. */
export function savedStepKey(saved: Pick<Workflow, 'steps' | 'on_failure'> | null): (step: string) => string | null {
  const ids = new Map([...(saved?.steps ?? []), ...(saved?.on_failure ?? [])]
    .filter(step => !!step.id)
    .map(step => [step.name, step.id as string] as const));
  return step => ids.get(step) ?? null;
}

/** Conversations started on an object that is not saved yet (a new plugin,
 *  workflow or Quick API), attached to it once it exists. A conversation stays
 *  tracked until its attachment succeeds; one that cannot be attached now is
 *  also recorded as owed to the saved object, for a later visit of it. */
export class AssistantDraftStore {
  private readonly entries = new Map<string, string | null>();

  track(discussionId: string, step: string | null = null): void {
    this.entries.set(discussionId, step);
  }

  /** Follows a step renamed after its conversation started. */
  renameStep(from: string, to: string): void {
    if (from === to) return;
    for (const [id, step] of this.entries) if (step === from) this.entries.set(id, to);
  }

  get size(): number {
    return this.entries.size;
  }

  ids(): string[] {
    return Array.from(this.entries.keys());
  }

  clear(): void {
    this.entries.clear();
  }

  /** Attaches every tracked conversation to `targetId`; throws when one
   *  fails. `stepKey` turns a step name into the saved step's durable id, or
   *  null when it is unknown: that conversation then stays owed, never filed
   *  under a key that is not the step's. */
  async attach(
    targetId: string,
    label?: string,
    stepKey: (step: string) => string | null = step => step,
  ): Promise<void> {
    const failures: unknown[] = [];
    let attached = 0;
    for (const [id, step] of Array.from(this.entries)) {
      const key = step ? stepKey(step) : null;
      if (step && !key) {
        addPending(targetId, { id, step });
        failures.push(new Error(`step "${step}" has no saved id yet`));
        continue;
      }
      try {
        await assistantConversations.update(id, {
          target_id: targetId,
          ...(key ? { target_step: key } : {}),
          ...(label ? { target_label: label } : {}),
        });
        this.entries.delete(id);
        settlePending(targetId, id);
        attached += 1;
      } catch (e) {
        // The saved step's id when known: it survives a later rename.
        addPending(targetId, { id, step: key ?? step });
        failures.push(e);
      }
    }
    if (attached > 0) notifyAssistantConversationsChanged();
    if (failures.length > 0) {
      throw new Error(`${failures.length} assistant conversation(s) not attached: ${String(failures[0])}`);
    }
  }
}
