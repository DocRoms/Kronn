import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { ChevronDown, ChevronRight, Filter } from 'lucide-react';
import { useIsMobile } from '../hooks/useMediaQuery';
import './FilterFold.css';

/** Width under which a filter bar folds: the 640 px breakpoint of the
 *  repository panel and the phone layouts. `useIsMobile` is `max-width: n - 1`. */
export const FILTER_FOLD_BREAKPOINT = 641;

interface FilterFoldProps {
  /** Name of the fold ("Filters"); the active count is appended as `(n)`. */
  label: string;
  /** Filters currently set — shown on the button so a folded filter is never silent. */
  activeCount: number;
  /** The filter controls: chips, selects, a "clear" button. */
  children: ReactNode;
  className?: string;
}

/**
 * Filter controls that stay inline on a wide viewport and fold behind one
 * "Filters (n)" button on a narrow one, so they neither stretch the toolbar nor
 * push it past the viewport. The button opens the controls in the flow, under
 * itself: no floating panel to clip or overflow at 400 px.
 */
export function FilterFold({ label, activeCount, children, className }: FilterFoldProps) {
  const narrow = useIsMobile(FILTER_FOLD_BREAKPOINT);
  const [open, setOpen] = useState(false);
  const panelId = useId();
  const toggleRef = useRef<HTMLButtonElement>(null);

  if (!narrow) return <>{children}</>;

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Escape' || !open) return;
    // An open fold owns Escape, as the shell menus do: the page-level handler
    // that collapses a mobile sidebar must not also fire.
    event.preventDefault();
    event.stopPropagation();
    setOpen(false);
    toggleRef.current?.focus();
  };

  return (
    <div className={`kr-filter-fold${className ? ` ${className}` : ''}`} data-open={open} onKeyDown={onKeyDown}>
      <button
        ref={toggleRef}
        type="button"
        className="kr-filter-fold-toggle"
        data-active={activeCount > 0 || undefined}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => setOpen(value => !value)}
      >
        <Filter size={14} aria-hidden="true" />
        <span className="kr-filter-fold-label">{activeCount > 0 ? `${label} (${activeCount})` : label}</span>
        {open ? <ChevronDown size={14} aria-hidden="true" /> : <ChevronRight size={14} aria-hidden="true" />}
      </button>
      {open && <div id={panelId} className="kr-filter-fold-panel">{children}</div>}
    </div>
  );
}
