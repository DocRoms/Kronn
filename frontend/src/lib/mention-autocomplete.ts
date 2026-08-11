export interface AgentMentionQuery {
  query: string;
  start: number;
  end: number;
}

/** Find the unfinished @alias immediately before the caret. */
export function findAgentMentionQuery(
  text: string,
  cursorPos: number,
): AgentMentionQuery | null {
  const prefix = text.slice(0, Math.max(0, Math.min(cursorPos, text.length)));
  const match = prefix.match(/(?:^|[\s([{])@([\w-]*)$/);
  if (!match) return null;
  const query = match[1].toLowerCase();
  return {
    query,
    start: prefix.length - query.length - 1,
    end: prefix.length,
  };
}
