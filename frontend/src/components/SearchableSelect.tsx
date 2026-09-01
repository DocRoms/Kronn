import { useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Check, ChevronDown, Search, X } from 'lucide-react';

export interface SearchableSelectOption {
  value: string;
  label: string;
  keywords?: string;
  description?: string;
  disabled?: boolean;
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
}

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
}: SearchableSelectProps) {
  const listId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [placement, setPlacement] = useState<'bottom' | 'top'>('bottom');
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
  const displayedOptions = useMemo(() => (
    clearLabel && !normalizedQuery
      ? [{ value: '', label: clearLabel } satisfies SearchableSelectOption, ...filtered]
      : filtered
  ), [clearLabel, filtered, normalizedQuery]);

  const firstEnabledIndex = displayedOptions.findIndex(option => !option.disabled);
  const resolvedActiveIndex = displayedOptions[activeIndex] && !displayedOptions[activeIndex].disabled
    ? activeIndex
    : Math.max(firstEnabledIndex, 0);

  useLayoutEffect(() => {
    if (!open || !rootRef.current) return;
    const bounds = rootRef.current.getBoundingClientRect();
    const roomBelow = window.innerHeight - bounds.bottom;
    const roomAbove = bounds.top;
    setPlacement(roomBelow < 220 && roomAbove > roomBelow ? 'top' : 'bottom');
  }, [open, displayedOptions.length]);

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
        <div id={listId} className="searchable-select-menu" role="listbox" aria-label={label}>
          {displayedOptions.length === 0 ? (
            <p className="searchable-select-empty" role="status">{emptyLabel}</p>
          ) : displayedOptions.map((option, index) => (
            <button
              key={option.value || '__clear'}
              id={`${listId}-${index}`}
              type="button"
              role="option"
              aria-label={option.label}
              aria-selected={option.value === value}
              aria-disabled={option.disabled || undefined}
              disabled={option.disabled}
              className="searchable-select-option"
              data-active={index === resolvedActiveIndex}
              data-disabled={option.disabled || undefined}
              data-value={option.value}
              onMouseEnter={() => { if (!option.disabled) setActiveIndex(index); }}
              onMouseDown={event => event.preventDefault()}
              onClick={() => choose(option)}
            >
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
