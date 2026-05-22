// 0.8.6 phase 4 — UI tests for the collapsible tool-calls banner.
//
// Pins the user-visible contract decided 2026-05-22 :
//   - default state is collapsed (no per-call details visible)
//   - banner summary shows count + per-tool breakdown + time-span
//   - clicking expands ; clicking again collapses
//   - long-result `<details>` block opens independently per call
//   - 2 sub-banners (kronn vs native) when both sources are present,
//     each with independent expand state
//
// The pure parsing logic is covered by `kronnToolParser.test.ts`. This
// file focuses on the React rendering + interaction surface only.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { ToolCallsGroup } from '../ToolCallsGroup';
import type { DiscussionMessage } from '../../types/generated';

function mkMsg(content: string, timestamp: string, idSuffix: string): DiscussionMessage {
  return {
    id: `msg-${idSuffix}`,
    role: 'System',
    content,
    agent_type: null,
    timestamp,
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    author_pseudo: null,
    author_avatar_email: null,
  };
}

// Passthrough t : tests assert keys, not localized strings.
const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

describe('ToolCallsGroup — Kronn-MCP sub-banner', () => {
  it('renders nothing for an empty list (defensive — guards against bad inputs)', () => {
    const { container } = render(<ToolCallsGroup messages={[]} t={t} />);
    expect(container.firstChild).toBeNull();
  });

  it('renders the kronn sub-banner with count + per-tool breakdown when collapsed', () => {
    const msgs = [
      mkMsg('[kronn-internal: qa_run({"qa_id": "a"})]', '2026-05-22T15:00:00Z', '1'),
      mkMsg('[kronn-internal: qa_run({"qa_id": "b"})]', '2026-05-22T15:00:01Z', '2'),
      mkMsg('[kronn-internal: qa_run({"qa_id": "c"})]', '2026-05-22T15:00:02Z', '3'),
      mkMsg('[kronn-internal: api_call({"endpoint": "/x"})]', '2026-05-22T15:00:03Z', '4'),
      mkMsg('[kronn-internal: mcp_list()]', '2026-05-22T15:00:04Z', '5'),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    const sub = screen.getByTestId('tool-calls-subbanner-kronn');
    expect(sub.getAttribute('data-expanded')).toBe('false');
    expect(sub.getAttribute('data-source')).toBe('kronn-internal');
    // Title with count interpolated.
    expect(sub.textContent).toContain('disc.toolCallsTitle:5');
    expect(sub.textContent).toContain('qa_run ×3');
    expect(sub.textContent).toContain('api_call');
    expect(sub.textContent).toContain('mcp_list');
    // The single-occurrence tools must NOT carry the ×1 suffix.
    expect(sub.textContent).not.toContain('api_call ×1');
    expect(sub.textContent).not.toContain('mcp_list ×1');
    // The native sub-banner must NOT appear (no agent-native messages).
    expect(screen.queryByTestId('tool-calls-subbanner-native')).toBeNull();
  });

  it('renders a time-span hint when the group spans >= 1s', () => {
    const msgs = [
      mkMsg('[kronn-internal: qa_run()]', '2026-05-22T15:00:00Z', '1'),
      mkMsg('[kronn-internal: qa_run()]', '2026-05-22T15:00:12Z', '2'),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    const sub = screen.getByTestId('tool-calls-subbanner-kronn');
    expect(sub.textContent).toContain('~12s');
  });

  it('omits the time-span when the group has a single message', () => {
    const msgs = [
      mkMsg('[kronn-internal: qa_list()]', '2026-05-22T15:00:00Z', '1'),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    const sub = screen.getByTestId('tool-calls-subbanner-kronn');
    expect(sub.textContent).not.toMatch(/~\d+[sm]/);
  });

  it('keeps the per-call list hidden when collapsed (default state)', () => {
    const msgs = [mkMsg('[kronn-internal: qa_list()]', '2026-05-22T15:00:00Z', '1')];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    expect(screen.queryByTestId('tool-calls-list-kronn')).toBeNull();
  });

  it('reveals the per-call list when the user clicks the header', () => {
    const msgs = [
      mkMsg('[kronn-internal: qa_run({"qa_id": "abc"})]', '2026-05-22T15:00:00Z', '1'),
      mkMsg('[kronn-internal: api_call({"endpoint": "/x"})]', '2026-05-22T15:00:01Z', '2'),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    fireEvent.click(screen.getByTestId('tool-calls-toggle-kronn'));
    const list = screen.getByTestId('tool-calls-list-kronn');
    expect(list).toBeInTheDocument();
    expect(list.textContent).toContain('qa_run');
    expect(list.textContent).toContain('{"qa_id": "abc"}');
    expect(list.textContent).toContain('api_call');
    expect(list.textContent).toContain('{"endpoint": "/x"}');
  });

  it('collapses again on second click (toggle behaviour)', () => {
    const msgs = [mkMsg('[kronn-internal: qa_list()]', '2026-05-22T15:00:00Z', '1')];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    fireEvent.click(screen.getByTestId('tool-calls-toggle-kronn'));
    expect(screen.getByTestId('tool-calls-list-kronn')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('tool-calls-toggle-kronn'));
    expect(screen.queryByTestId('tool-calls-list-kronn')).toBeNull();
  });

  it('skips malformed tool messages without crashing the banner', () => {
    const msgs = [
      mkMsg('[kronn-internal: qa_list()]', '2026-05-22T15:00:00Z', '1'),
      mkMsg('[kronn-internal: BAD_FORMAT_no_lowercase', '2026-05-22T15:00:01Z', '2'),
      mkMsg('[kronn-internal: api_call({})]', '2026-05-22T15:00:02Z', '3'),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    const sub = screen.getByTestId('tool-calls-subbanner-kronn');
    // Count reflects ONLY the parseable entries (2 valid, 1 dropped).
    expect(sub.textContent).toContain('disc.toolCallsTitle:2');
    expect(sub.textContent).toContain('qa_list');
    expect(sub.textContent).toContain('api_call');
  });

  it('exposes a result <details> block per call when results are present', () => {
    const msgs = [
      mkMsg(
        '[kronn-internal: qa_run({"qa_id": "a"}) → {"status": "ok", "data": [1, 2, 3]}]',
        '2026-05-22T15:00:00Z',
        '1',
      ),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    fireEvent.click(screen.getByTestId('tool-calls-toggle-kronn'));
    const list = screen.getByTestId('tool-calls-list-kronn');
    expect(list.textContent).toContain('disc.kronnToolResult');
    expect(list.textContent).toContain('"status": "ok"');
  });

  it('hides the result block when no payload was captured (read-only tools)', () => {
    const msgs = [mkMsg('[kronn-internal: qa_list()]', '2026-05-22T15:00:00Z', '1')];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    fireEvent.click(screen.getByTestId('tool-calls-toggle-kronn'));
    const list = screen.getByTestId('tool-calls-list-kronn');
    expect(list.textContent).not.toContain('disc.kronnToolResult');
  });

  it('aria-expanded mirrors the visual state for screen readers', () => {
    const msgs = [mkMsg('[kronn-internal: qa_list()]', '2026-05-22T15:00:00Z', '1')];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    const toggle = screen.getByTestId('tool-calls-toggle-kronn');
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
  });
});

// ─── 0.8.6 phase 4 — agent-native sub-banner (NEW) ──────────────────

describe('ToolCallsGroup — agent-native sub-banner', () => {
  it('renders ONLY the native sub-banner when no kronn-internal calls', () => {
    const msgs = [
      mkMsg('[agent-native: Read({"file_path":"/tmp/a.rs"})]', '2026-05-22T15:00:00Z', '1'),
      mkMsg('[agent-native: Read({"file_path":"/tmp/b.rs"})]', '2026-05-22T15:00:01Z', '2'),
      mkMsg('[agent-native: Bash({"command":"ls"})]', '2026-05-22T15:00:02Z', '3'),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    const sub = screen.getByTestId('tool-calls-subbanner-native');
    expect(sub.getAttribute('data-source')).toBe('agent-native');
    expect(sub.textContent).toContain('disc.toolCallsNativeTitle:3');
    expect(sub.textContent).toContain('Read ×2');
    expect(sub.textContent).toContain('Bash');
    // No kronn sub-banner.
    expect(screen.queryByTestId('tool-calls-subbanner-kronn')).toBeNull();
  });

  it('renders BOTH sub-banners when both sources are present', () => {
    // The everyday case : agent uses qa_run AND Read in the same reply.
    const msgs = [
      mkMsg('[kronn-internal: qa_run({"qa_id":"a"})]', '2026-05-22T15:00:00Z', '1'),
      mkMsg('[agent-native: Read({"file_path":"/x"})]', '2026-05-22T15:00:01Z', '2'),
      mkMsg('[kronn-internal: api_call({"endpoint":"/y"})]', '2026-05-22T15:00:02Z', '3'),
      mkMsg('[agent-native: Bash({"command":"ls"})]', '2026-05-22T15:00:03Z', '4'),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    // Both sub-banners present.
    expect(screen.getByTestId('tool-calls-subbanner-kronn')).toBeInTheDocument();
    expect(screen.getByTestId('tool-calls-subbanner-native')).toBeInTheDocument();
    // Each shows the right count for its source.
    expect(screen.getByTestId('tool-calls-subbanner-kronn').textContent).toContain('disc.toolCallsTitle:2');
    expect(screen.getByTestId('tool-calls-subbanner-native').textContent).toContain('disc.toolCallsNativeTitle:2');
  });

  it('expand state is independent per sub-banner', () => {
    const msgs = [
      mkMsg('[kronn-internal: qa_list()]', '2026-05-22T15:00:00Z', '1'),
      mkMsg('[agent-native: Read({"file_path":"/x"})]', '2026-05-22T15:00:01Z', '2'),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    // Expand ONLY the kronn one.
    fireEvent.click(screen.getByTestId('tool-calls-toggle-kronn'));
    expect(screen.getByTestId('tool-calls-list-kronn')).toBeInTheDocument();
    // Native list MUST stay collapsed — independent state.
    expect(screen.queryByTestId('tool-calls-list-native')).toBeNull();
    // Now flip the native one.
    fireEvent.click(screen.getByTestId('tool-calls-toggle-native'));
    expect(screen.getByTestId('tool-calls-list-native')).toBeInTheDocument();
    // Kronn stays expanded.
    expect(screen.getByTestId('tool-calls-list-kronn')).toBeInTheDocument();
  });

  it('accepts PascalCase native tool names (Claude Code convention)', () => {
    // Claude Code's natives are Read / Bash / Edit / Write / Grep / Glob.
    // The kronn-internal regex is lowercase-only ; the agent-native one
    // accepts any non-paren sequence so PascalCase survives.
    const msgs = [
      mkMsg('[agent-native: Read({})]', '2026-05-22T15:00:00Z', '1'),
      mkMsg('[agent-native: Edit({})]', '2026-05-22T15:00:01Z', '2'),
      mkMsg('[agent-native: Write({})]', '2026-05-22T15:00:02Z', '3'),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    const sub = screen.getByTestId('tool-calls-subbanner-native');
    expect(sub.textContent).toContain('Read');
    expect(sub.textContent).toContain('Edit');
    expect(sub.textContent).toContain('Write');
  });

  it('accepts third-party MCP tool names with mcp__server__tool prefix', () => {
    // Wired MCP servers (mcp-github, mcp-atlassian) emit tools with this
    // prefix. They aren't kronn-internal (which strips the prefix at
    // capture time) so they land in the native bucket.
    const msgs = [
      mkMsg('[agent-native: mcp__github__create_issue({"title":"foo"})]', '2026-05-22T15:00:00Z', '1'),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    const sub = screen.getByTestId('tool-calls-subbanner-native');
    expect(sub.textContent).toContain('mcp__github__create_issue');
  });

  it('per-source time-span computed from same-source timestamps only', () => {
    // Native calls span 10s, kronn calls span 2s. Each sub-banner must
    // report ITS OWN span, not the global one. Prevents the "Kronn took
    // 30s" lie when most of the 30s was a Bash call between them.
    const msgs = [
      mkMsg('[kronn-internal: qa_run()]', '2026-05-22T15:00:00Z', '1'),
      mkMsg('[agent-native: Bash({})]', '2026-05-22T15:00:05Z', '2'),
      mkMsg('[kronn-internal: qa_run()]', '2026-05-22T15:00:02Z', '3'),
      mkMsg('[agent-native: Bash({})]', '2026-05-22T15:00:10Z', '4'),
    ];
    render(<ToolCallsGroup messages={msgs} t={t} />);
    // Kronn span = 0→2s = 2s
    expect(screen.getByTestId('tool-calls-subbanner-kronn').textContent).toContain('~2s');
    // Native span = 5→10s = 5s
    expect(screen.getByTestId('tool-calls-subbanner-native').textContent).toContain('~5s');
  });
});

describe('ToolCallsGroup — defensive scenarios', () => {
  it('handles ten or more calls without performance fanfare', () => {
    const msgs: DiscussionMessage[] = [];
    for (let i = 0; i < 15; i++) {
      msgs.push(mkMsg(
        `[kronn-internal: qa_run({"qa_id": "${i}"})]`,
        `2026-05-22T15:00:${i.toString().padStart(2, '0')}Z`,
        `${i}`,
      ));
    }
    render(<ToolCallsGroup messages={msgs} t={t} />);
    const sub = screen.getByTestId('tool-calls-subbanner-kronn');
    expect(sub.textContent).toContain('disc.toolCallsTitle:15');
    expect(sub.textContent).toContain('qa_run ×15');
  });

  it('does not change content when re-rendered with the same messages (idempotent)', () => {
    const msgs = [mkMsg('[kronn-internal: qa_list()]', '2026-05-22T15:00:00Z', '1')];
    const { rerender, container } = render(<ToolCallsGroup messages={msgs} t={t} />);
    const html1 = container.innerHTML;
    rerender(<ToolCallsGroup messages={msgs} t={t} />);
    expect(container.innerHTML).toBe(html1);
  });

  it('exposes stable test hooks per sub-banner', () => {
    const msgs = [
      mkMsg('[kronn-internal: qa_list()]', '2026-05-22T15:00:00Z', '1'),
      mkMsg('[agent-native: Read({})]', '2026-05-22T15:00:01Z', '2'),
    ];
    render(<ToolCallsGroup messages={msgs} t={(k) => k} />);
    expect(screen.getByTestId('tool-calls-group')).toBeInTheDocument();
    expect(screen.getByTestId('tool-calls-subbanner-kronn')).toBeInTheDocument();
    expect(screen.getByTestId('tool-calls-subbanner-native')).toBeInTheDocument();
    expect(screen.getByTestId('tool-calls-toggle-kronn')).toBeInTheDocument();
    expect(screen.getByTestId('tool-calls-toggle-native')).toBeInTheDocument();
  });
});

void vi;
