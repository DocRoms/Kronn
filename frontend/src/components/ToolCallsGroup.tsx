// 0.8.6 phase 4 — Collapsible tool-call banner(s).
//
// Groups every consecutive `[kronn-internal: ...]` / `[agent-native: ...]`
// System message between two non-tool messages and renders 1-2 sub-banners
// above the next agent reply. Each banner collapses independently ; both
// collapsed by default.
//
// Why TWO sub-banners (kronn vs native) :
//   - Kronn-MCP tools (qa_run, api_call, ...) carry the user's intent —
//     the Kronn primitives the agent invoked.
//   - Native tools (Read, Bash, Edit, ...) describe what the agent did
//     LOCALLY (file system, shell).
//   The user (2026-05-22 feedback) wants to scan "what Kronn did" without
//   noise from "what the agent did under the hood", but ALSO needs the
//   native trace for post-stream debug since the live tool log
//   disappears when the stream ends.
//
// Before (every tool call = its own bubble) :
//   🔧 api_call({...long json...})        15:37
//   🔧 qa_list                             15:37
//   🔧 Read                                15:37
//   🔧 Bash                                15:37
//   Agent: <reply>
//
// After (this component) :
//   ┌──────────────────────────────────────────────────────────┐
//   │ 🔧 Outils Kronn (3) — qa_run ×2, api_call ×1     ~3s ▾  │
//   └──────────────────────────────────────────────────────────┘
//   ┌──────────────────────────────────────────────────────────┐
//   │ ⚙ Outils agent-natifs (8) — Read ×3, Bash ×3, Edit ×2  │
//   └──────────────────────────────────────────────────────────┘
//   Agent: <reply>

import { useState } from 'react';
import { ChevronRight } from 'lucide-react';
import type { DiscussionMessage } from '../types/generated';
import {
  parseKronnToolMessage,
  groupToolCallsByName,
  formatDurationCompact,
  type KronnToolCall,
  type ToolCallSource,
} from '../lib/kronnToolParser';

interface ToolCallsGroupProps {
  /** Consecutive tool System messages, in chronological order. Caller is
   *  responsible for the grouping — we just render. */
  messages: DiscussionMessage[];
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ToolCallsGroup({ messages, t }: ToolCallsGroupProps) {
  // Parse + filter — defensive against messages that look like tool traces
  // but don't match the canonical regex (incomplete writes, future format
  // changes).
  const allCalls: KronnToolCall[] = messages
    .map(m => parseKronnToolMessage(m.content))
    .filter((c): c is KronnToolCall => c !== null);

  if (allCalls.length === 0) return null;

  // Group calls by source so each sub-banner only renders its own.
  // Timestamps are paired with calls so per-source duration stays accurate.
  const kronnPairs: Array<{ call: KronnToolCall; ts: string }> = [];
  const nativePairs: Array<{ call: KronnToolCall; ts: string }> = [];
  messages.forEach((m) => {
    const call = parseKronnToolMessage(m.content);
    if (!call) return;
    const target = call.source === 'kronn-internal' ? kronnPairs : nativePairs;
    target.push({ call, ts: m.timestamp });
  });

  return (
    <div className="disc-tool-calls-group" data-testid="tool-calls-group">
      {kronnPairs.length > 0 && (
        <ToolCallsSubBanner
          source="kronn-internal"
          pairs={kronnPairs}
          t={t}
          testIdSuffix="kronn"
        />
      )}
      {nativePairs.length > 0 && (
        <ToolCallsSubBanner
          source="agent-native"
          pairs={nativePairs}
          t={t}
          testIdSuffix="native"
        />
      )}
    </div>
  );
}

// ─── Sub-banner ────────────────────────────────────────────────────

interface ToolCallsSubBannerProps {
  source: ToolCallSource;
  pairs: Array<{ call: KronnToolCall; ts: string }>;
  t: (key: string, ...args: (string | number)[]) => string;
  testIdSuffix: string;
}

function ToolCallsSubBanner({ source, pairs, t, testIdSuffix }: ToolCallsSubBannerProps) {
  // Default collapsed (Q3 answer 2026-05-22). Each sub-banner gets its
  // own state so the user can expand Kronn calls while keeping the
  // native trace collapsed (or vice versa).
  const [expanded, setExpanded] = useState(false);

  const calls = pairs.map(p => p.call);
  const grouped = groupToolCallsByName(calls);
  const total = calls.length;

  // Per-source time span — first to last timestamp WITHIN that source's
  // calls. Native and Kronn spans can differ when the agent interleaves
  // (Read → qa_run → Edit → api_call) ; each banner reports its own.
  let durationLabel: string | null = null;
  if (pairs.length >= 2) {
    const firstTs = new Date(pairs[0].ts).getTime();
    const lastTs = new Date(pairs[pairs.length - 1].ts).getTime();
    const span = lastTs - firstTs;
    if (span > 0) durationLabel = formatDurationCompact(span);
  }

  const summary = grouped
    .map(g => g.count > 1 ? `${g.name} ×${g.count}` : g.name)
    .join(', ');

  // Source-specific label + icon. Token-economy framing kept ONLY on
  // the Kronn banner — agent-native tools DO cost tokens (Bash output
  // gets fed back to the LLM), so it would be a lie there.
  const isKronn = source === 'kronn-internal';
  const titleKey = isKronn ? 'disc.toolCallsTitle' : 'disc.toolCallsNativeTitle';
  const icon = isKronn ? '🔧' : '⚙';

  return (
    <div
      className="disc-tool-calls-subbanner"
      data-testid={`tool-calls-subbanner-${testIdSuffix}`}
      data-source={source}
      data-expanded={expanded}
      style={{
        margin: '4px 0',
        padding: '8px 12px',
        borderRadius: 'var(--kr-radius-md, 6px)',
        background: 'var(--kr-bg-card-subtle, transparent)',
        border: '1px solid var(--kr-border-subtle, transparent)',
        fontSize: 12,
        // Native banner ghosted slightly so the Kronn one stands out
        // when both are shown side-by-side.
        opacity: isKronn ? 1 : 0.86,
      }}
    >
      <button
        type="button"
        className="disc-tool-calls-header"
        data-testid={`tool-calls-toggle-${testIdSuffix}`}
        onClick={() => setExpanded(v => !v)}
        aria-expanded={expanded}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 6,
          width: '100%',
          background: 'transparent',
          border: 'none',
          padding: 0,
          color: 'var(--kr-text-secondary)',
          cursor: 'pointer',
          fontSize: 12,
          textAlign: 'left',
        }}
      >
        <ChevronRight
          size={10}
          style={{
            transition: 'transform 120ms',
            transform: expanded ? 'rotate(90deg)' : 'rotate(0deg)',
            flexShrink: 0,
          }}
          aria-hidden="true"
        />
        <span aria-hidden="true">{icon}</span>
        <span className="font-semibold">{t(titleKey, total)}</span>
        <span style={{ color: 'var(--kr-text-ghost)' }}>—</span>
        <span style={{ color: 'var(--kr-text-ghost)', flexGrow: 1 }}>{summary}</span>
        {durationLabel && (
          <span style={{ color: 'var(--kr-text-ghost)', flexShrink: 0 }}>
            ~{durationLabel}
          </span>
        )}
      </button>
      {expanded && (
        <ul
          className="disc-tool-calls-list"
          data-testid={`tool-calls-list-${testIdSuffix}`}
          style={{
            listStyle: 'none',
            margin: '6px 0 0 16px',
            padding: 0,
            display: 'flex',
            flexDirection: 'column',
            gap: 4,
          }}
        >
          {calls.map((call, idx) => (
            <li
              key={idx}
              className="disc-tool-calls-item"
              style={{
                fontSize: 11,
                color: 'var(--kr-text-secondary)',
                fontFamily: 'var(--kr-font-mono, monospace)',
                wordBreak: 'break-word',
              }}
            >
              <span style={{ color: 'var(--kr-text-ghost)' }}>•</span>{' '}
              <span className="font-semibold">{call.name}</span>
              {call.args && (
                <span style={{ color: 'var(--kr-text-ghost)' }}>({call.args})</span>
              )}
              {call.result && (
                <details
                  style={{
                    marginTop: 2,
                    marginLeft: 12,
                    fontSize: 10,
                  }}
                >
                  <summary style={{ color: 'var(--kr-text-ghost)', cursor: 'pointer' }}>
                    {t('disc.kronnToolResult')}
                  </summary>
                  <pre
                    style={{
                      margin: '4px 0 0 0',
                      padding: 4,
                      background: 'var(--kr-bg-code-panel)',
                      // Pin against the always-dark code-panel bg
                      // (feedback_bg_code_panel_pin_text memory) — without
                      // this, light theme renders dark text on dark bg.
                      color: '#e8eaed',
                      borderRadius: 4,
                      overflow: 'auto',
                      maxHeight: 200,
                    }}
                  >
                    {call.result}
                  </pre>
                </details>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
