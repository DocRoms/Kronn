const RE_SEED = /<!--KRONN_SEED_START-->\s*([\s\S]*?)\s*<!--KRONN_SEED_END-->/;
const RE_AGENT_HANDOFF = /^<!-- KRONN_AGENT_HANDOFF:[\s\S]*?-->\s*/;
const INJECTED_CONTEXT_RE = /<!-- kronn:context title="([^"]*)" -->\n?([\s\S]*?)\n?<!-- \/kronn:context -->/g;
export const DELETED_MESSAGE_MARKER = '[kronn:message-deleted]';

export type MsgSegment =
  | { kind: 'text'; body: string }
  | { kind: 'context'; title: string; body: string };

export function splitMessageSeed(content: string): { visible: string; seed: string | null } {
  const match = content.match(RE_SEED);
  if (!match) return { visible: content, seed: null };
  const index = match.index ?? 0;
  return { visible: content.slice(0, index).trim(), seed: match[1].trim() };
}

export function stripAgentHandoff(content: string): string {
  return content.replace(RE_AGENT_HANDOFF, '');
}

export function isDeletedMessage(content: string): boolean {
  return content.startsWith(DELETED_MESSAGE_MARKER);
}

export function splitInjectedContext(content: string): MsgSegment[] {
  const segments: MsgSegment[] = [];
  let last = 0;
  for (const match of content.matchAll(INJECTED_CONTEXT_RE)) {
    const index = match.index ?? 0;
    if (index > last) segments.push({ kind: 'text', body: content.slice(last, index) });
    segments.push({ kind: 'context', title: match[1], body: match[2] });
    last = index + match[0].length;
  }
  if (last < content.length) segments.push({ kind: 'text', body: content.slice(last) });
  return segments.length ? segments : [{ kind: 'text', body: content }];
}
