import {
  useCallback, useContext, useEffect, useId, useLayoutEffect, useRef, useState,
  type KeyboardEvent, type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { measureChipsOutOfView, measureStripOverflow } from '../lib/stripOverflow';
import { HeaderScrollLineContext } from './headerScrollLineContext';
import { useAnchoredLayer } from './ui/useAnchoredLayer';
import './ParticipantsStrip.css';

export interface ParticipantsStripEntry {
  key: string | number;
  /** The chip in the strip: a button that opens the participant's details. */
  chip: ReactNode;
  /** Its row in the full list the « +N » counter opens. */
  row: ReactNode;
  onSelect: () => void;
}

export interface ParticipantsStripProps {
  entries: readonly ParticipantsStripEntry[];
  /** The accessible name of the « +N » counter, given the number of
   *  participants: a plural form (`tp`), right for one and for several. */
  moreLabel: (total: number) => string;
  /** The name of the full list. */
  allLabel: string;
  /** Shown when there is no participant. */
  empty?: ReactNode;
}

/**
 * KT-1936 — the participants on ONE line that scrolls sideways (owner
 * decision 2026-11-20, option A): the right edge fades while chips are past
 * it, and a « +N » counter (the chips not in view) opens the full list. The
 * strip never wraps and is never replaced by a summary button; the same fade
 * applies on a wide screen when the chips do not all fit.
 *
 * The full list is a floating layer (portal and `useAnchoredStyle`, as Menu):
 * a header that clips its overflow cannot cut it, and it stays inside the
 * window. Focus moves into it on open; Escape closes it and gives focus back
 * to the counter; a press outside closes it.
 *
 * KT-1936 review round 2 — inside a HeaderScrollLine (the room header) the
 * whole line scrolls, not the strip: the chips are counted against the line's
 * visible box, the fade is the line's, and the counter goes to the line's
 * pinned end slot, outside what scrolls, so it is always in view.
 */
export function ParticipantsStrip({ entries, moreLabel, allLabel, empty }: ParticipantsStripProps) {
  const listId = useId();
  const listRef = useRef<HTMLDivElement>(null);
  const moreRef = useRef<HTMLButtonElement>(null);
  const [overflow, setOverflow] = useState({ hidden: 0, fadeEnd: false });
  const [open, setOpen] = useState(false);
  const line = useContext(HeaderScrollLineContext);
  const lineScroller = line?.scroller ?? null;

  const measure = useCallback(() => {
    const list = listRef.current;
    if (!list) return;
    // The empty state is not a chip: it never makes a counter.
    const next = entries.length === 0
      ? { hidden: 0, fadeEnd: false }
      : lineScroller
        ? { hidden: measureChipsOutOfView(lineScroller, list), fadeEnd: false }
        : measureStripOverflow(list);
    setOverflow(current => (current.hidden === next.hidden && current.fadeEnd === next.fadeEnd ? current : next));
    // Every chip in view again (a wider window): the counter goes, its list too.
    if (next.hidden === 0) setOpen(false);
  }, [lineScroller, entries.length]);

  useLayoutEffect(measure, [measure, entries]);

  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    lineScroller?.addEventListener('scroll', measure, { passive: true });
    let observer: ResizeObserver | undefined;
    if (typeof ResizeObserver !== 'undefined') {
      observer = new ResizeObserver(measure);
      observer.observe(list);
      if (lineScroller) observer.observe(lineScroller);
    }
    return () => {
      lineScroller?.removeEventListener('scroll', measure);
      observer?.disconnect();
    };
  }, [measure, lineScroller]);

  const close = useCallback(() => setOpen(false), []);
  // Focus into the list once placed, Tab kept inside, Escape back to the
  // counter, a press outside closes: shared with the participant details.
  const { ref: layerRef, style } = useAnchoredLayer<HTMLDivElement>({
    anchorRef: moreRef, open, placement: 'bottom', align: 'end', onClose: close,
  });

  const showMore = overflow.hidden > 0;

  const onListKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('button'));
    if (!buttons.length) return;
    event.preventDefault();
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === 'ArrowDown'
      ? (at + 1) % buttons.length
      : (at - 1 + buttons.length) % buttons.length;
    buttons[next].focus();
  };

  // A participant chosen in the full list: its chip scrolls into view, so
  // the details it opens sit under it.
  const revealChip = (index: number) => {
    const item = listRef.current?.children[index] as HTMLElement | undefined;
    item?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
  };

  const more = showMore ? (
    <button
      ref={moreRef}
      type="button"
      className="kr-participants-strip-more"
      data-testid="participants-more"
      aria-label={moreLabel(entries.length)}
      aria-expanded={open}
      aria-haspopup="dialog"
      aria-controls={open ? listId : undefined}
      onClick={() => (open ? close() : setOpen(true))}
    >
      +{overflow.hidden}
    </button>
  ) : null;

  return (
    <div className="kr-participants-strip" data-fade-end={overflow.fadeEnd || undefined}>
      <div ref={listRef} className="disc-participants-list kr-participants-strip-list" onScroll={measure}>
        {entries.length === 0 ? empty : entries.map(entry => (
          <span key={entry.key} className="kr-participants-strip-item">{entry.chip}</span>
        ))}
      </div>
      {more && (line?.end ? createPortal(more, line.end) : more)}
      {open && createPortal(
        <div
          ref={layerRef}
          id={listId}
          className="kr-participants-strip-all"
          role="dialog"
          aria-label={allLabel}
          data-testid="participants-all"
          style={style}
          onKeyDown={onListKeyDown}
        >
          <ul>
            {entries.map((entry, index) => (
              <li key={entry.key}>
                <button type="button" onClick={() => { close(); revealChip(index); entry.onSelect(); }}>{entry.row}</button>
              </li>
            ))}
          </ul>
        </div>,
        document.body,
      )}
    </div>
  );
}
