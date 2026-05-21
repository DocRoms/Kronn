import { useEffect, useId, useRef, useState, type KeyboardEvent } from 'react';
import { ChevronDown, Check } from 'lucide-react';
import './Dropdown.css';

export interface DropdownOption<V extends string = string> {
  value: V;
  label: string;
  description?: string;
  disabled?: boolean;
}

interface DropdownProps<V extends string = string> {
  value: V;
  options: DropdownOption<V>[];
  onChange: (value: V) => void;
  ariaLabel?: string;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  /** When provided, used as the visible label for the current selection
   *  even if `value` is not in `options` (e.g. unsupported variants). */
  fallbackLabel?: string;
  /** Test hook for the trigger button. */
  testId?: string;
}

// Reusable popover dropdown that replaces native <select> so dark-theme
// option colors are honored across Firefox/Safari (they ignore CSS on
// native <option> chrome). Keyboard a11y mirrors a real listbox:
// ArrowUp/Down move highlight, Enter/Space select, Esc closes,
// Home/End jump to bounds, focus returns to trigger on close.
export function Dropdown<V extends string = string>({
  value,
  options,
  onChange,
  ariaLabel,
  placeholder,
  disabled,
  className,
  fallbackLabel,
  testId,
}: DropdownProps<V>) {
  const [open, setOpen] = useState(false);
  const [highlight, setHighlight] = useState(() =>
    Math.max(0, options.findIndex(o => o.value === value)),
  );
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const listId = useId();

  const current = options.find(o => o.value === value);
  const displayLabel = current?.label ?? fallbackLabel ?? placeholder ?? value ?? '';

  // Sync highlight when external value changes while closed.
  useEffect(() => {
    if (!open) {
      const idx = options.findIndex(o => o.value === value);
      if (idx >= 0) setHighlight(idx);
    }
  }, [value, options, open]);

  // Click outside → close.
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      const root = triggerRef.current?.parentElement;
      if (root && !root.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);

  // Scroll highlighted item into view.
  useEffect(() => {
    if (!open || !listRef.current) return;
    const el = listRef.current.querySelector<HTMLElement>(
      `[data-dropdown-index="${highlight}"]`,
    );
    el?.scrollIntoView({ block: 'nearest' });
  }, [highlight, open]);

  const move = (delta: number) => {
    const enabled = options
      .map((o, i) => ({ o, i }))
      .filter(({ o }) => !o.disabled);
    if (enabled.length === 0) return;
    const currentEnabledIdx = enabled.findIndex(({ i }) => i === highlight);
    const nextEnabledIdx = currentEnabledIdx < 0
      ? 0
      : (currentEnabledIdx + delta + enabled.length) % enabled.length;
    setHighlight(enabled[nextEnabledIdx].i);
  };

  const selectAt = (idx: number) => {
    const opt = options[idx];
    if (!opt || opt.disabled) return;
    onChange(opt.value);
    setOpen(false);
    triggerRef.current?.focus();
  };

  const onKey = (e: KeyboardEvent<HTMLElement>) => {
    if (disabled) return;
    if (!open) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        setOpen(true);
      }
      return;
    }
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        move(1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        move(-1);
        break;
      case 'Home':
        e.preventDefault();
        setHighlight(options.findIndex(o => !o.disabled));
        break;
      case 'End': {
        e.preventDefault();
        const lastEnabled = [...options].reverse().findIndex(o => !o.disabled);
        if (lastEnabled >= 0) setHighlight(options.length - 1 - lastEnabled);
        break;
      }
      case 'Enter':
      case ' ':
        e.preventDefault();
        selectAt(highlight);
        break;
      case 'Escape':
        e.preventDefault();
        setOpen(false);
        triggerRef.current?.focus();
        break;
    }
  };

  return (
    <div className={`kr-dropdown ${className ?? ''}`.trim()} onKeyDown={onKey}>
      <button
        ref={triggerRef}
        type="button"
        className={`kr-dropdown-trigger${disabled ? ' kr-dropdown-trigger-disabled' : ''}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-label={ariaLabel}
        disabled={disabled}
        onClick={() => !disabled && setOpen(o => !o)}
        data-testid={testId}
      >
        <span className="kr-dropdown-trigger-label">{displayLabel}</span>
        <ChevronDown size={14} className="kr-dropdown-trigger-chev" aria-hidden />
      </button>
      {open && (
        <div
          ref={listRef}
          id={listId}
          role="listbox"
          aria-label={ariaLabel}
          className="kr-dropdown-list"
        >
          {options.map((opt, i) => {
            const isSelected = opt.value === value;
            const isHighlighted = i === highlight;
            return (
              <div
                key={opt.value}
                role="option"
                aria-selected={isSelected}
                aria-disabled={opt.disabled}
                data-dropdown-index={i}
                data-testid={testId ? `${testId}-option-${opt.value}` : undefined}
                className={[
                  'kr-dropdown-option',
                  isHighlighted ? 'kr-dropdown-option-highlight' : '',
                  isSelected ? 'kr-dropdown-option-selected' : '',
                  opt.disabled ? 'kr-dropdown-option-disabled' : '',
                ].filter(Boolean).join(' ')}
                onMouseEnter={() => setHighlight(i)}
                onMouseDown={(e) => {
                  // Prevent trigger blur before we get the click.
                  e.preventDefault();
                }}
                onClick={() => selectAt(i)}
              >
                <div className="kr-dropdown-option-main">
                  {isSelected && <Check size={12} className="kr-dropdown-option-check" />}
                  <span className="kr-dropdown-option-label">{opt.label}</span>
                </div>
                {opt.description && (
                  <span className="kr-dropdown-option-desc">{opt.description}</span>
                )}
              </div>
            );
          })}
          {options.length === 0 && (
            <div className="kr-dropdown-option kr-dropdown-option-disabled">
              <span className="kr-dropdown-option-label">{placeholder ?? '—'}</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
