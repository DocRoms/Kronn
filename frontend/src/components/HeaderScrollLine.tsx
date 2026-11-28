import {
  useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState,
  type ReactNode,
} from 'react';
import { lineFadesEnd, wheelPixels } from '../lib/stripOverflow';
import { HeaderScrollLineContext } from './headerScrollLineContext';
import './HeaderScrollLine.css';

export interface HeaderScrollLineProps {
  children: ReactNode;
  className?: string;
}

/**
 * KT-1936 review round 2 — the room header's agent line (owner decision
 * 2026-11-20, option A): ONE line that scrolls sideways when its content does
 * not fit. The main agent's trigger comes first, so it is the first thing read;
 * the participant chips, the scope and the trust chips follow it in the
 * scroller. The end slot is pinned at the right, outside what scrolls: the
 * participants strip renders its « +N » counter there, so the way to the full
 * list stays in view. The right edge fades while something lies past it.
 *
 * Reached by pointer and touch (native scrolling), by a vertical wheel (turned
 * sideways here, its lines and pages counted in pixels) and by keyboard (the browser scrolls a focused control into
 * view). The line never wraps and keeps the header's height.
 */
export function HeaderScrollLine({ children, className }: HeaderScrollLineProps) {
  const [scroller, setScroller] = useState<HTMLDivElement | null>(null);
  const [end, setEnd] = useState<HTMLDivElement | null>(null);
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const [fadeEnd, setFadeEnd] = useState(false);
  const attachScroller = useCallback((element: HTMLDivElement | null) => {
    scrollerRef.current = element;
    setScroller(element);
  }, []);

  const measure = useCallback(() => {
    const element = scrollerRef.current;
    if (!element) return;
    const next = lineFadesEnd(element);
    setFadeEnd(current => (current === next ? current : next));
  }, []);

  useLayoutEffect(measure, [measure, children]);

  useEffect(() => {
    if (!scroller) return;
    scroller.addEventListener('scroll', measure, { passive: true });
    // A plain mouse wheel only scrolls up and down: on a line that can
    // scroll sideways, its turn moves the line instead.
    const onWheel = (event: WheelEvent) => {
      if (event.ctrlKey || Math.abs(event.deltaX) >= Math.abs(event.deltaY)) return;
      const room = scroller.scrollWidth - scroller.clientWidth;
      if (room <= 1) return;
      const before = scroller.scrollLeft;
      scroller.scrollLeft = Math.max(0, Math.min(room, before + wheelPixels(event, scroller)));
      if (scroller.scrollLeft !== before) event.preventDefault();
    };
    scroller.addEventListener('wheel', onWheel, { passive: false });
    let observer: ResizeObserver | undefined;
    if (typeof ResizeObserver !== 'undefined') {
      observer = new ResizeObserver(measure);
      observer.observe(scroller);
      if (trackRef.current) observer.observe(trackRef.current);
    }
    return () => {
      scroller.removeEventListener('scroll', measure);
      scroller.removeEventListener('wheel', onWheel);
      observer?.disconnect();
    };
  }, [scroller, measure]);

  const slots = useMemo(() => ({ scroller, end }), [scroller, end]);

  return (
    <div
      className={className ? `kr-header-line ${className}` : 'kr-header-line'}
      data-fade-end={fadeEnd || undefined}
      data-testid="header-scroll-line"
    >
      <div ref={attachScroller} className="kr-header-line-scroller" data-testid="header-scroll-line-scroller">
        <div ref={trackRef} className="kr-header-line-track">
          <HeaderScrollLineContext.Provider value={slots}>{children}</HeaderScrollLineContext.Provider>
        </div>
      </div>
      <div ref={setEnd} className="kr-header-line-end" />
    </div>
  );
}
