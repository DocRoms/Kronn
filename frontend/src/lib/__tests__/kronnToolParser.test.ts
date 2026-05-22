// 0.8.6 phase 4 — pure unit tests for the `[kronn-internal: ...]`
// parser shared between MessageBubble and ToolCallsGroup. A regex break
// here would silently fall back to "raw System message" rendering for
// every tool call, regressing the whole 0.8.6 phase 4 UX win.

import { describe, it, expect } from 'vitest';
import {
  parseKronnToolMessage,
  isKronnToolMessage,
  groupToolCallsByName,
  formatDurationCompact,
} from '../kronnToolParser';

describe('parseKronnToolMessage — kronn-internal source', () => {
  it('parses a no-arg call', () => {
    const out = parseKronnToolMessage('[kronn-internal: qa_list()]');
    expect(out).toEqual({ source: 'kronn-internal', name: 'qa_list', args: '', result: null });
  });

  it('parses a no-paren call (legacy backend format)', () => {
    const out = parseKronnToolMessage('[kronn-internal: disc_meta]');
    expect(out).toEqual({ source: 'kronn-internal', name: 'disc_meta', args: null, result: null });
  });

  it('parses a JSON-args call', () => {
    const out = parseKronnToolMessage(
      '[kronn-internal: api_call({"endpoint_path": "/v1/sites"})]',
    );
    expect(out?.source).toBe('kronn-internal');
    expect(out?.name).toBe('api_call');
    expect(out?.args).toBe('{"endpoint_path": "/v1/sites"}');
    expect(out?.result).toBeNull();
  });

  it('parses a call with a result payload', () => {
    const out = parseKronnToolMessage(
      '[kronn-internal: disc_get_message(4) → {"role": "Agent", "content": "hi"}]',
    );
    expect(out?.source).toBe('kronn-internal');
    expect(out?.name).toBe('disc_get_message');
    expect(out?.args).toBe('4');
    expect(out?.result).toBe('{"role": "Agent", "content": "hi"}');
  });

  it('handles a multi-line args block (e.g. structured JSON)', () => {
    // The backend may pretty-print large JSON args across multiple
    // lines. The regex must tolerate this so the parser doesn't drop
    // calls with verbose bodies.
    const content = '[kronn-internal: qa_run({"qa_id": "abc",\n  "vars": {"key": "value"}\n})]';
    const out = parseKronnToolMessage(content);
    expect(out?.name).toBe('qa_run');
    expect(out?.args).toContain('"qa_id": "abc"');
    expect(out?.args).toContain('"key": "value"');
  });

  it('returns null for non-tool content (defensive — every System message hits this)', () => {
    expect(parseKronnToolMessage('summary cached at 14:32')).toBeNull();
    expect(parseKronnToolMessage('Error: connection refused')).toBeNull();
    expect(parseKronnToolMessage('')).toBeNull();
    expect(parseKronnToolMessage('[unrelated-bracket-thing]')).toBeNull();
  });

  it('returns null for malformed tool messages (defensive)', () => {
    // Missing closing bracket — parser refuses rather than emitting
    // a half-parsed tool that would render with garbage text.
    expect(parseKronnToolMessage('[kronn-internal: qa_list(')).toBeNull();
    // Unknown tool name with capital letters / dashes — strict regex
    // accepts only lowercase + underscores (matches the actual
    // backend tool catalog).
    expect(parseKronnToolMessage('[kronn-internal: BadName()]')).toBeNull();
    expect(parseKronnToolMessage('[kronn-internal: foo-bar()]')).toBeNull();
  });
});

describe('isKronnToolMessage', () => {
  it('returns true for any kronn-internal-prefixed content', () => {
    // No regex match required — this is the cheap predicate the
    // group-fold uses to decide whether to buffer a message. Even
    // malformed tool traces still belong to the tool group, not the
    // regular System-message stream.
    expect(isKronnToolMessage('[kronn-internal: qa_list()]')).toBe(true);
    expect(isKronnToolMessage('[kronn-internal: malformed')).toBe(true);
  });

  it('returns true for agent-native-prefixed content (0.8.6 phase 4)', () => {
    // Native tools (Claude Code's Read/Bash/Edit, third-party MCP)
    // share the same buffering path as kronn-internal — the grouping
    // logic doesn't care about the source, only the parser does.
    expect(isKronnToolMessage('[agent-native: Read({})]')).toBe(true);
    expect(isKronnToolMessage('[agent-native: mcp__github__create_issue({})]')).toBe(true);
  });

  it('returns false for non-tool System content', () => {
    expect(isKronnToolMessage('summary cached')).toBe(false);
    expect(isKronnToolMessage('Error: foo')).toBe(false);
    expect(isKronnToolMessage('')).toBe(false);
  });
});

describe('parseKronnToolMessage — agent-native source (0.8.6 phase 4)', () => {
  it('parses Claude Code native PascalCase tool', () => {
    const out = parseKronnToolMessage('[agent-native: Read({"file_path":"/tmp/a.rs"})]');
    expect(out?.source).toBe('agent-native');
    expect(out?.name).toBe('Read');
    expect(out?.args).toBe('{"file_path":"/tmp/a.rs"}');
  });

  it('parses third-party MCP tool with namespace prefix', () => {
    // mcp__<server>__<tool> shape comes from wired MCP servers like
    // mcp-github. The kronn-internal regex would reject this (caps +
    // underscores) ; the agent-native one accepts it.
    const out = parseKronnToolMessage(
      '[agent-native: mcp__github__create_issue({"title":"foo"})]',
    );
    expect(out?.source).toBe('agent-native');
    expect(out?.name).toBe('mcp__github__create_issue');
  });

  it('parses a Bash command with shell args', () => {
    // Real-world : the args contain a complex command string. We
    // preserve it verbatim since the backend already truncates at
    // 120 chars (truncate_tool_args).
    const out = parseKronnToolMessage(
      '[agent-native: Bash({"command":"ls -la /tmp"})]',
    );
    expect(out?.name).toBe('Bash');
    expect(out?.args).toBe('{"command":"ls -la /tmp"}');
  });

  it('parses with truncated args (ending in ellipsis from backend)', () => {
    // Edit / Write tools carry large file contents ; the backend
    // truncates at 120 chars + `…` before persisting. Parser must
    // accept this gracefully.
    const out = parseKronnToolMessage(
      '[agent-native: Edit({"file_path":"/a/b.rs","old":"xxxxxx…)]',
    );
    expect(out?.name).toBe('Edit');
    expect(out?.args).toContain('…');
  });

  it('returns null for malformed agent-native messages', () => {
    expect(parseKronnToolMessage('[agent-native: Read(')).toBeNull();
    // Empty name → not parseable.
    expect(parseKronnToolMessage('[agent-native: ()]')).toBeNull();
  });
});

describe('groupToolCallsByName', () => {
  it('groups + counts identical tool names', () => {
    const calls = [
      { source: 'kronn-internal' as const, name: 'qa_run', args: null, result: null },
      { source: 'kronn-internal' as const, name: 'qa_run', args: null, result: null },
      { source: 'kronn-internal' as const, name: 'api_call', args: null, result: null },
      { source: 'kronn-internal' as const, name: 'qa_run', args: null, result: null },
      { source: 'kronn-internal' as const, name: 'mcp_list', args: null, result: null },
    ];
    expect(groupToolCallsByName(calls)).toEqual([
      { name: 'qa_run', count: 3 },
      { name: 'api_call', count: 1 },
      { name: 'mcp_list', count: 1 },
    ]);
  });

  it('sorts alphabetically when counts tie (stable order across renders)', () => {
    const calls = [
      { source: 'kronn-internal' as const, name: 'zoo_tool', args: null, result: null },
      { source: 'kronn-internal' as const, name: 'apple_tool', args: null, result: null },
    ];
    expect(groupToolCallsByName(calls).map(g => g.name)).toEqual(['apple_tool', 'zoo_tool']);
  });

  it('returns empty list for empty input', () => {
    expect(groupToolCallsByName([])).toEqual([]);
  });
});

describe('formatDurationCompact', () => {
  it('renders sub-second as "<1s"', () => {
    expect(formatDurationCompact(0)).toBe('<1s');
    expect(formatDurationCompact(999)).toBe('<1s');
  });

  it('renders seconds without minute prefix', () => {
    expect(formatDurationCompact(1_000)).toBe('1s');
    expect(formatDurationCompact(45_000)).toBe('45s');
    expect(formatDurationCompact(59_000)).toBe('59s');
  });

  it('renders minutes + seconds compactly above 1 min', () => {
    expect(formatDurationCompact(60_000)).toBe('1m');
    expect(formatDurationCompact(75_000)).toBe('1m15s');
    expect(formatDurationCompact(125_000)).toBe('2m5s');
  });

  it('handles very long spans (defensive — long QP runs)', () => {
    // 10 minutes — a deep workflow may take this long.
    expect(formatDurationCompact(600_000)).toBe('10m');
  });
});
