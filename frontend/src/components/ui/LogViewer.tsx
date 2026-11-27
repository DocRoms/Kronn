import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { ArrowDownToLine, Eraser, RefreshCw, WrapText } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { Button } from './Button';
import { CopyButton } from './CopyButton';
import { IconButton } from './IconButton';
import {
  logText, logTone, stripAnsi, toLogLine,
  type LogLine, type LogLineObject, type LogMaxHeight,
} from './logKinds';
import './LogViewer.css';

export interface LogViewerProps {
  /** What the log is ("Output of the run"). Required: it names the scroll
   *  area, a focusable region with role="log". */
  label: string;
  lines: readonly LogLine[];
  /** A bounded height (default md), or `fill`: as tall as its parent. */
  maxHeight?: LogMaxHeight;
  /** Start wrapped (default) or on one line per line, scrolling sideways;
   *  the toolbar's toggle switches it. */
  wrap?: boolean;
  /** Keep the end in view as lines arrive (default on). Scrolling up by hand
   *  turns it off and shows a "jump to the end" control that resumes it. */
  follow?: boolean;
  /** The source is still writing: a cursor on the last line. */
  streaming?: boolean;
  /** A number before each line (hidden from assistive technology). */
  lineNumbers?: boolean;
  /** The number of the first line shown (default 1): a tail of a longer
   *  output starts where the tail starts. */
  startNumber?: number;
  /** A CopyButton in the toolbar: copies the whole text. */
  copyable?: boolean;
  /** A refresh button in the toolbar. */
  onRefresh?: () => void;
  /** A clear button in the toolbar. */
  onClear?: () => void;
  /** Above the log: "last 200 lines", "truncated at 50 000 characters". */
  note?: ReactNode;
  /** When there is no line. Default "No output". */
  empty?: ReactNode;
  className?: string;
}

/** How far from the end (px) still counts as at the end. */
const END_SLACK = 4;

const atEnd = (el: HTMLElement) => el.scrollHeight - el.scrollTop - el.clientHeight <= END_SLACK;

/** A time as `<time>` reads it, and as the reader does. */
function timeParts(time: LogLineObject['time'], format: Intl.DateTimeFormat): { dateTime?: string; text: string } | null {
  if (time === undefined || time === '') return null;
  if (typeof time === 'string') {
    const ms = Date.parse(time);
    return { dateTime: time, text: Number.isNaN(ms) ? time : format.format(ms) };
  }
  const date = typeof time === 'number' ? new Date(time) : time;
  if (Number.isNaN(date.getTime())) return null;
  return { dateTime: date.toISOString(), text: format.format(date) };
}

/** KT-2004 — a log of lines: a focusable named region with role="log";
 *  stderr, errors and warnings tinted and marked in words; times in a column
 *  of their own; follow, wrap, a toolbar of what is wired, a note, an empty
 *  text. ANSI escape sequences are removed, never rendered. */
export function LogViewer({
  label, lines, maxHeight = 'md', wrap: initialWrap = true, follow = true, streaming = false, lineNumbers = false,
  startNumber = 1, copyable = false, onRefresh, onClear, note, empty, className,
}: LogViewerProps) {
  const { t, locale } = useT();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [wrap, setWrap] = useState(initialWrap);
  const [following, setFollowing] = useState(follow);

  // A new `follow` or `wrap` prop resets the state during render (react.dev,
  // "Adjusting some state when a prop changes"), not in an effect.
  const [prevFollow, setPrevFollow] = useState(follow);
  if (prevFollow !== follow) {
    setPrevFollow(follow);
    setFollowing(follow);
  }
  const [prevWrap, setPrevWrap] = useState(initialWrap);
  if (prevWrap !== initialWrap) {
    setPrevWrap(initialWrap);
    setWrap(initialWrap);
  }

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && follow && following) el.scrollTop = el.scrollHeight;
  }, [lines, follow, following, wrap]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el || !follow) return;
    const end = atEnd(el);
    if (!end && following) setFollowing(false);
    else if (end && !following) setFollowing(true);
  };

  const jumpToEnd = () => {
    const el = scrollRef.current;
    setFollowing(true);
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    // The control goes away: the focus goes to the log it moved.
    el.focus();
  };

  const format = new Intl.DateTimeFormat(locale, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  const items = lines.map(toLogLine);
  const withTimes = items.some(l => l.time !== undefined && l.time !== '');
  // The number column is as wide as the last number shown.
  const digits = String(startNumber + Math.max(items.length, 1) - 1).length;
  const marker = (line: LogLineObject) => {
    if (line.level === 'error' || line.level === 'warn') return t(`ui.log.level.${line.level}`);
    if (line.stream === 'stderr') return t('ui.log.stderr');
    return null;
  };

  const classes = ['ui-log', className].filter(Boolean).join(' ');
  return (
    <div
      className={classes}
      data-max-height={maxHeight}
      data-wrap={wrap ? 'true' : 'false'}
      data-streaming={streaming ? 'true' : undefined}
      style={{ '--ui-log-digits': digits } as CSSProperties}
    >
      {/* The wrap toggle is always wired, so the header always shows. */}
      <div className="ui-log-header">
        {note && <div className="ui-log-note">{note}</div>}
        <div className="ui-log-toolbar">
          {copyable && <CopyButton size="sm" label={t('ui.log.copy')} getText={() => logText(lines)} />}
          {onRefresh && <IconButton size="sm" label={t('ui.log.refresh')} icon={<RefreshCw />} onClick={onRefresh} data-action="refresh" />}
          {onClear && <IconButton size="sm" label={t('ui.log.clear')} icon={<Eraser />} onClick={onClear} data-action="clear" />}
          <IconButton
            size="sm"
            label={t('ui.log.wrap')}
            icon={<WrapText />}
            aria-pressed={wrap}
            onClick={() => setWrap(w => !w)}
            data-action="wrap"
          />
        </div>
      </div>
      <div className="ui-log-frame">
        <div
          ref={scrollRef}
          className="ui-log-scroll"
          role="log"
          aria-label={label}
          tabIndex={0}
          onScroll={onScroll}
        >
          {items.length === 0 ? (
            <p className="ui-log-empty">{empty ?? t('ui.log.empty')}</p>
          ) : items.map((line, i) => {
            const time = withTimes ? timeParts(line.time, format) : null;
            const mark = marker(line);
            const last = i === items.length - 1;
            return (
              <div
                // Lines only arrive at the end: their position is their identity.
                key={i}
                className="ui-log-line"
                data-stream={line.stream}
                data-level={line.level}
                data-tone={logTone(line)}
              >
                {lineNumbers && <span className="ui-log-number" aria-hidden="true">{startNumber + i}</span>}
                {withTimes && (time
                  ? <time className="ui-log-time" dateTime={time.dateTime}>{time.text}</time>
                  : <span className="ui-log-time" aria-hidden="true" />)}
                {mark && <span className="ui-log-marker">{mark}</span>}
                <span className="ui-log-text">
                  {stripAnsi(line.text)}
                  {streaming && last && <span className="ui-log-cursor" aria-hidden="true" />}
                </span>
              </div>
            );
          })}
        </div>
        {follow && !following && (
          <Button size="sm" className="ui-log-jump" icon={<ArrowDownToLine />} onClick={jumpToEnd}>
            {t('ui.log.jumpToEnd')}
          </Button>
        )}
      </div>
    </div>
  );
}
