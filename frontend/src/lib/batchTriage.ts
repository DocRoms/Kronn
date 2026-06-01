import type { Discussion, DiscussionMessage } from '../types/generated';

/**
 * Parsed view of a batch-triage agent reply. The triage QP emits a **Markdown**
 * report (not JSON) — verdict-first, then "what's wrong", open questions, and a
 * one-line summary. This module extracts the fields the batch-review cockpit
 * needs. Parsing is tolerant: a missing section just leaves its field empty.
 */
export interface BatchTriageResult {
  ticket_id?: string;
  verdict?: string;
  confidence?: string;
  human_summary?: string;
  /** "⚠️ Ce qui cloche" bullets — incohérences / manques. */
  issues?: string[];
  /** "❓ À trancher" numbered questions. */
  open_questions?: string[];
  /** The "👉 Et maintenant ?" action line, jargon-free. */
  next_action?: string;
}

export interface BatchTriageRow {
  discussion: Discussion;
  result: BatchTriageResult | null;
  parseError: string | null;
}

/** Strip leading list/heading markers + surrounding emphasis from a line. */
function cleanItem(line: string): string {
  return line
    .replace(/^\s*(?:[-*]|\d+[.)])\s+/, '') // bullet / "1." / "1)"
    .replace(/^\s*\*\*(.*?)\*\*\s*$/, '$1') // **bold** wrapper
    .trim();
}

/** Collect the list items under a section heading until the next heading / hr / blank-after-content. */
function sectionItems(lines: string[], headingMatcher: RegExp): string[] {
  const start = lines.findIndex(l => headingMatcher.test(l));
  if (start < 0) return [];
  const out: string[] = [];
  for (let i = start + 1; i < lines.length; i += 1) {
    const l = lines[i];
    if (/^\s*#{1,6}\s/.test(l) || /^\s*---\s*$/.test(l)) break; // next section / hr
    if (/^\s*(?:[-*]|\d+[.)])\s+/.test(l)) {
      const item = cleanItem(l);
      // Skip the "nothing here" sentinels the prompt allows.
      if (item && !/^(aucune|✅\s|rien\b|none\b|nothing\b)/i.test(item)) out.push(item);
    }
  }
  return out;
}

const VERDICT_WORDS = [
  'Prêt à framer', 'Décision humaine requise', 'Décision requise', 'À fermer',
  'Bloqué', 'Obsolète', 'Analyse incomplète', 'Risque',
  'Ready to frame', 'Needs human input', 'To close', 'Blocked', 'Obsolete', 'Risk',
];

export function parseBatchTriageResult(content: string): { result: BatchTriageResult | null; error: string | null } {
  const text = content.trim();
  if (!text) return { result: null, error: 'empty' };
  const lines = text.split('\n');
  const res: BatchTriageResult = {};

  // ticket_id: from "## EW-1234 — Triage" (or any heading first token)
  const heading = lines.find(l => /^\s*#{1,3}\s+\S/.test(l));
  if (heading) {
    const m = heading.replace(/^\s*#{1,3}\s+/, '').match(/\b([A-Z]+-\d+)\b/);
    if (m) res.ticket_id = m[1];
  }

  // verdict: explicit "**Verdict :** X" OR a known verdict phrase anywhere near the top.
  const explicit = text.match(/\*\*\s*Verdict\s*:?\s*\*\*\s*([^·*\n|]+)/i)
    || text.match(/Verdict\s*:\s*([^·*\n|]+)/i);
  if (explicit) {
    res.verdict = explicit[1].replace(/[*`]/g, '').trim();
  } else {
    // Bold-first style: "**🟢 Prêt à framer** · confiance haute"
    const found = VERDICT_WORDS.find(w => text.includes(w));
    if (found) res.verdict = found;
  }

  // confidence — `conf\w*` covers confiance / confidence / confianza, then the value.
  const conf = text.match(/conf\w*\s*[:.]?\s*\**\s*(haute|moyenne|basse|high|medium|low|alta|media|baja)\b/i);
  if (conf) res.confidence = conf[1].toLowerCase();

  // action line "👉 ..." (strip the trailing "· Jira : ..." part for readability)
  const action = lines.find(l => l.includes('👉'));
  if (action) res.next_action = action.replace(/^.*👉\s*/, '').replace(/\s*·\s*\*\*Jira.*$/i, '').trim();

  // "En bref : ..." one-liner → human_summary
  const enBref = text.match(/_?\s*(?:En bref|In short|En resumen)\s*:\s*([^\n_]+)/i);
  if (enBref) res.human_summary = enBref[1].replace(/[_*`]+$/g, '').trim();

  res.issues = sectionItems(lines, /Ce qui cloche|Incohérences|What'?s wrong|Problèmes|Problems/i);
  res.open_questions = sectionItems(lines, /À trancher|Questions ouvertes|Open questions|Questions/i);

  // If we extracted nothing meaningful, treat as unparseable so the UI can flag it.
  const gotSomething = res.verdict || res.human_summary || res.issues.length || res.open_questions.length;
  if (!gotSomething) return { result: null, error: 'unrecognized_format' };
  return { result: res, error: null };
}

export function lastAgentMessage(messages: DiscussionMessage[]): DiscussionMessage | null {
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    if (messages[i].role === 'Agent') return messages[i];
  }
  return null;
}

export function buildBatchTriageRows(discussions: Discussion[]): BatchTriageRow[] {
  return discussions.map((discussion) => {
    const msg = lastAgentMessage(discussion.messages);
    if (!msg) return { discussion, result: null, parseError: 'no_agent_message' };
    const parsed = parseBatchTriageResult(msg.content);
    return { discussion, result: parsed.result, parseError: parsed.error };
  });
}

export function buildContinuationDraft(row: BatchTriageRow): string {
  const ticketId = row.result?.ticket_id || row.discussion.title.split(/\s|—/)[0] || row.discussion.title;
  const summary = row.result?.human_summary ? `\n\nRésumé triage:\n${row.result.human_summary}` : '';
  const questions = row.result?.open_questions?.length
    ? `\n\nQuestions ouvertes à traiter:\n${row.result.open_questions.map(q => `- ${q}`).join('\n')}`
    : '';
  return `CONTINUER LE FRAMING INTERACTIF\n\nTicket: ${ticketId}\nSource discussion: ${row.discussion.id}\nMode: interactive_framing\nwrite_intent: preview_only_until_explicit_yes\nrequires_human_confirmation_before_write: true\n\nReprends le triage déjà produit dans cette discussion, clarifie les points nécessaires avec moi, puis prépare une preview Jira. N'écris rien dans Jira sans mon oui explicite.${summary}${questions}`;
}
