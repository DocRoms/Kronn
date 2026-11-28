// KT-2004 — the line model and variants of LogViewer, apart from the
// component so a file of components exports components only (fast refresh).

export type LogStream = 'stdout' | 'stderr';
export type LogLevel = 'debug' | 'info' | 'warn' | 'error';
export const LOG_LEVELS: readonly LogLevel[] = ['debug', 'info', 'warn', 'error'];

export interface LogLineObject {
  text: string;
  /** stderr is tinted danger and marked "stderr". */
  stream?: LogStream;
  /** error is tinted danger, warn warning; both are marked in words. */
  level?: LogLevel;
  /** When it was written: a `<time>` in a column of its own. A string is
   *  shown as it is (and is its `datetime`); a Date or an epoch in ms is
   *  written as a time of day in the interface's locale. */
  time?: string | number | Date;
}

/** A line: its text alone, or with its stream, level and time. */
export type LogLine = string | LogLineObject;

/** The bounded heights of the scroll area, or `fill`: as tall as its parent
 *  (a panel, a dialog body), which must have a height. */
export type LogMaxHeight = 'sm' | 'md' | 'lg' | 'fill';
export const LOG_MAX_HEIGHTS: readonly LogMaxHeight[] = ['sm', 'md', 'lg', 'fill'];

/** The tint of a line, from the status tokens; never its only cue. */
export type LogTone = 'danger' | 'warning';

/** ESC [ … final (CSI: colours, cursor moves), ESC ] … BEL or ESC \ (OSC:
 *  titles, hyperlinks), and the two-character escapes (ESC 7, ESC c…). */
// eslint-disable-next-line no-control-regex
const ANSI = /\u001b\[[0-?]*[ -/]*[@-~]|\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)|\u001b[0-9=>@-Z\\-_a-z]|\u009b[0-?]*[ -/]*[@-~]/g;

/** The text of `s` without its ANSI escape sequences: none is rendered. */
export function stripAnsi(s: string): string {
  return s.replace(ANSI, '');
}

/** The object form of a line. */
export function toLogLine(line: LogLine): LogLineObject {
  return typeof line === 'string' ? { text: line } : line;
}

/** The tint of a line: danger for stderr and errors, warning for warnings. */
export function logTone(line: LogLineObject): LogTone | undefined {
  if (line.level === 'error') return 'danger';
  if (line.level === 'warn') return 'warning';
  if (line.stream === 'stderr') return 'danger';
  return undefined;
}

/** The whole text of a log, ANSI removed, one line per line (what its
 *  CopyButton copies). */
export function logText(lines: readonly LogLine[]): string {
  return lines.map(l => stripAnsi(toLogLine(l).text)).join('\n');
}
