export interface AgentMentionQuery {
  query: string;
  start: number;
  end: number;
}

/** Keep familiar alias prefixes ahead of additional catalogue/substring matches. */
export function mentionMatchRank(trigger: string, query: string): number {
  return trigger.slice(1).toLowerCase().startsWith(query.toLowerCase()) ? 0 : 1;
}

/** Find an unfinished alias/catalogue search token; selection inserts a canonical alias. */
export function findAgentMentionQuery(
  text: string,
  cursorPos: number,
): AgentMentionQuery | null {
  const prefix = text.slice(0, Math.max(0, Math.min(cursorPos, text.length)));
  const match = prefix.match(/(?:^|[\s([{])@([\p{L}\p{N}\p{M}_./:-]*)$/u);
  if (!match) return null;
  const query = match[1].toLowerCase();
  return {
    query,
    start: prefix.length - match[1].length - 1,
    end: prefix.length,
  };
}
