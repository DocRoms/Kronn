import { useCallback, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Check, ChevronDown, Search, X } from 'lucide-react';

export interface SearchableSelectOption {
  value: string;
  label: string;
  keywords?: string;
  description?: string;
  disabled?: boolean;
  /** Shown before the label: a thumbnail of what this entry IS. A filename
   *  alone makes the reader open every entry to find the one they meant. */
  visual?: ReactNode;
  /** Synthetic "use what I typed" entry added by `allowCustomValue`, not a
   *  catalogue result. Callers can target it (styling, tests) via this flag. */
  custom?: boolean;
}

interface SearchableSelectProps {
  value: string;
  options: SearchableSelectOption[];
  onChange: (value: string) => void;
  label: string;
  placeholder: string;
  emptyLabel: string;
  clearLabel?: string;
  clearable?: boolean;
  disabled?: boolean;
  testId?: string;
  className?: string;
  dataModelTierAgent?: string;
  dataModelTier?: string;
  /** Offer the typed query itself as a selectable entry when it matches no
   *  option. For a catalogue that cannot prove or disprove compatibility
   *  (KT-531), the operator must still be able to name an exact model id the
   *  list does not carry — never just a fixed set of detected values. */
  allowCustomValue?: boolean;
  customValueHint?: string;
}

// Mirrors the CSS cap: min(420px, 50vh). The gap is the menu's offset from
// the control plus a margin from the viewport edge.
const MENU_MAX_HEIGHT = 420;
const MENU_VIEWPORT_SHARE = 0.5;
const MENU_EDGE_GAP = 13;

export function SearchableSelect({
  value,
  options,
  onChange,
  label,
  placeholder,
  emptyLabel,
  clearLabel,
  clearable = true,
  disabled = false,
  testId,
  className,
  dataModelTierAgent,
  dataModelTier,
  allowCustomValue = false,
  customValueHint,
}: SearchableSelectProps) {
  const listId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [placement, setPlacement] = useState<'bottom' | 'top'>('bottom');
  const [menuMaxHeight, setMenuMaxHeight] = useState<number>();
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const selected = options.find(option => option.value === value);
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const filtered = useMemo(() => {
    if (!normalizedQuery) return options;
    return options.filter(option => (
      option.label.toLocaleLowerCase().includes(normalizedQuery)
      || option.keywords?.toLocaleLowerCase().includes(normalizedQuery)
    ));
  }, [normalizedQuery, options]);
  const trimmedQuery = query.trim();
  const displayedOptions = useMemo(() => {
    const base = clearLabel && !normalizedQuery
      ? [{ value: '', label: clearLabel } satisfies SearchableSelectOption, ...filtered]
      : filtered;
    const hasExactMatch = options.some(option => (
      option.value.toLocaleLowerCase() === normalizedQuery
      || option.label.toLocaleLowerCase() === normalizedQuery
    ));
    if (!allowCustomValue || !trimmedQuery || hasExactMatch) return base;
    const customOption: SearchableSelectOption = {
      value: trimmedQuery,
      label: trimmedQuery,
      description: customValueHint,
      custom: true,
    };
    return [...base, customOption];
  }, [allowCustomValue, clearLabel, customValueHint, filtered, normalizedQuery, options, trimmedQuery]);

  const firstEnabledIndex = displayedOptions.findIndex(option => !option.disabled);
  const resolvedActiveIndex = displayedOptions[activeIndex] && !displayedOptions[activeIndex].disabled
    ? activeIndex
    : Math.max(firstEnabledIndex, 0);

  // Open on the side the list fits, else the roomier one, and never taller
  // than that side's room, so the whole menu stays in the viewport.
  const placeMenu = useCallback(() => {
    if (!rootRef.current) return;
    const bounds = rootRef.current.getBoundingClientRect();
    const cap = Math.min(MENU_MAX_HEIGHT, window.innerHeight * MENU_VIEWPORT_SHARE);
    const roomBelow = Math.max(0, window.innerHeight - bounds.bottom - MENU_EDGE_GAP);
    const roomAbove = Math.max(0, bounds.top - MENU_EDGE_GAP);
    const menu = menuRef.current;
    // Without layout (jsdom) scrollHeight is 0: assume the list fills the cap.
    const natural = menu?.scrollHeight ? menu.scrollHeight + menu.offsetHeight - menu.clientHeight : cap;
    const needed = Math.min(cap, natural);
    const next = needed <= roomBelow || roomBelow >= roomAbove ? 'bottom' : 'top';
    setPlacement(next);
    setMenuMaxHeight(Math.floor(Math.min(cap, next === 'bottom' ? roomBelow : roomAbove)));
  }, []);

  useLayoutEffect(() => {
    if (!open) return;
    placeMenu();
    const onScroll = (event: Event) => {
      if (event.target !== menuRef.current) placeMenu();
    };
    window.addEventListener('resize', placeMenu);
    window.addEventListener('scroll', onScroll, true);
    return () => {
      window.removeEventListener('resize', placeMenu);
      window.removeEventListener('scroll', onScroll, true);
    };
  }, [open, displayedOptions.length, placeMenu]);

  const choose = (option: SearchableSelectOption) => {
    if (option.disabled) return;
    onChange(option.value);
    setQuery('');
    setOpen(false);
  };

  const moveActive = (direction: 1 | -1) => {
    if (displayedOptions.length === 0) return;
    let next = resolvedActiveIndex;
    for (let step = 0; step < displayedOptions.length; step += 1) {
      next = Math.max(0, Math.min(next + direction, displayedOptions.length - 1));
      if (!displayedOptions[next]?.disabled) {
        setActiveIndex(next);
        return;
      }
      if (next === 0 || next === displayedOptions.length - 1) return;
    }
  };

  return (
    <div
      ref={rootRef}
      className={['searchable-select', className].filter(Boolean).join(' ')}
      data-open={open}
      data-placement={placement}
      onBlur={event => {
        if (!rootRef.current?.contains(event.relatedTarget as Node | null)) {
          setOpen(false);
          setQuery('');
        }
      }}
    >
      <div className="searchable-select-control">
        <Search size={14} aria-hidden="true" />
        <input
          type="search"
          role="combobox"
          aria-label={label}
          aria-expanded={open}
          aria-controls={listId}
          aria-activedescendant={open && displayedOptions[resolvedActiveIndex]
            ? `${listId}-${resolvedActiveIndex}`
            : undefined}
          autoComplete="off"
          disabled={disabled}
          data-testid={testId}
          data-model-tier-agent={dataModelTierAgent}
          data-model-tier={dataModelTier}
          placeholder={placeholder}
          value={open ? query : (selected?.label ?? '')}
          onFocus={() => {
            setOpen(true);
            setQuery('');
            const selectedIndex = displayedOptions.findIndex(option => option.value === value && !option.disabled);
            setActiveIndex(selectedIndex >= 0 ? selectedIndex : Math.max(firstEnabledIndex, 0));
          }}
          onChange={event => {
            setQuery(event.target.value);
            setOpen(true);
            setActiveIndex(0);
          }}
          onKeyDown={event => {
            if (event.key === 'ArrowDown') {
              event.preventDefault();
              setOpen(true);
              moveActive(1);
            } else if (event.key === 'ArrowUp') {
              event.preventDefault();
              setOpen(true);
              moveActive(-1);
            } else if (event.key === 'Enter' && open && displayedOptions[resolvedActiveIndex]) {
              event.preventDefault();
              choose(displayedOptions[resolvedActiveIndex]);
            } else if (event.key === 'Escape') {
              event.preventDefault();
              setOpen(false);
              setQuery('');
            }
          }}
        />
        {value && !disabled && clearable ? (
          <button
            type="button"
            className="searchable-select-clear"
            aria-label={clearLabel ?? emptyLabel}
            onMouseDown={event => event.preventDefault()}
            onClick={() => choose({ value: '', label: clearLabel ?? emptyLabel })}
          >
            <X size={13} />
          </button>
        ) : (
          <ChevronDown size={14} className="searchable-select-chevron" aria-hidden="true" />
        )}
      </div>

      {open && !disabled && (
        <div
          ref={menuRef}
          id={listId}
          className="searchable-select-menu"
          role="listbox"
          aria-label={label}
          style={menuMaxHeight === undefined ? undefined : { maxHeight: menuMaxHeight }}
        >
          {displayedOptions.length === 0 ? (
            <p className="searchable-select-empty" role="status">{emptyLabel}</p>
          ) : displayedOptions.map((option, index) => (
            <button
              key={option.value || '__clear'}
              id={`${listId}-${index}`}
              type="button"
              role="option"
              aria-label={option.custom && option.description ? `${option.label} — ${option.description}` : option.label}
              aria-selected={option.value === value}
              aria-disabled={option.disabled || undefined}
              disabled={option.disabled}
              className="searchable-select-option"
              data-active={index === resolvedActiveIndex}
              data-disabled={option.disabled || undefined}
              data-value={option.value}
              data-custom={option.custom || undefined}
              onMouseEnter={() => { if (!option.disabled) setActiveIndex(index); }}
              onMouseDown={event => event.preventDefault()}
              onClick={() => choose(option)}
            >
              {option.visual && (
                <span className="searchable-select-visual" aria-hidden="true">{option.visual}</span>
              )}
              <span>
                <strong>{option.label}</strong>
                {option.description && <small>{option.description}</small>}
              </span>
              {option.value === value && <Check size={14} aria-hidden="true" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
