import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import './Tabs.css';

export interface TabItem {
  /** Stable id, the value `onChange` receives. */
  id: string;
  label: ReactNode;
  /** An icon before the label, hidden from assistive technology. */
  icon?: ReactNode;
  /** A count or a state after the label (a StatusBadge, a number). */
  badge?: ReactNode;
  disabled?: boolean;
  /** The panel the tab shows. Without one, the caller renders the content and
   *  the tab only reports the selection (it then has no aria-controls). */
  panel?: ReactNode;
}

export interface TabsProps {
  /** Accessible name of the tab list, required: what the tabs switch between. */
  label: string;
  tabs: readonly TabItem[];
  /** Selected tab, when the caller holds the state. */
  value?: string;
  /** First selected tab when the component holds the state (default: the first enabled tab). */
  defaultValue?: string;
  onChange?: (id: string) => void;
  size?: 'sm' | 'md';
  className?: string;
}

/** KT-1880 — tabs with the WAI-ARIA roles (tablist, tab, tabpanel): one tab
 *  in the Tab order, the arrows move between tabs and select them, Home and
 *  End go to the first and last, a disabled tab is skipped. */
export function Tabs({ label, tabs, value, defaultValue, onChange, size = 'md', className }: TabsProps) {
  const base = useId();
  const firstEnabled = tabs.find(tab => !tab.disabled)?.id;
  const [own, setOwn] = useState(defaultValue ?? firstEnabled);
  const selected = value ?? own;
  const refs = useRef(new Map<string, HTMLButtonElement>());
  const tabId = (id: string) => `${base}-tab-${id}`;
  const panelId = (id: string) => `${base}-panel-${id}`;

  const select = (id: string) => {
    if (value === undefined) setOwn(id);
    if (id !== selected) onChange?.(id);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const enabled = tabs.filter(tab => !tab.disabled);
    if (enabled.length === 0) return;
    const at = enabled.findIndex(tab => tab.id === selected);
    let next: number | null = null;
    if (e.key === 'ArrowRight') next = (at + 1) % enabled.length;
    else if (e.key === 'ArrowLeft') next = (at - 1 + enabled.length) % enabled.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = enabled.length - 1;
    if (next === null) return;
    e.preventDefault();
    const id = enabled[next].id;
    select(id);
    refs.current.get(id)?.focus();
  };

  const active = tabs.find(tab => tab.id === selected);
  return (
    <div className={className ? `ui-tabs ${className}` : 'ui-tabs'} data-size={size}>
      <div role="tablist" aria-label={label} className="ui-tabs-list" onKeyDown={onKeyDown}>
        {tabs.map(tab => {
          const isSelected = tab.id === selected;
          return (
            <button
              key={tab.id}
              ref={el => { if (el) refs.current.set(tab.id, el); else refs.current.delete(tab.id); }}
              type="button"
              role="tab"
              id={tabId(tab.id)}
              className="ui-tab"
              aria-selected={isSelected}
              aria-controls={tab.panel !== undefined ? panelId(tab.id) : undefined}
              tabIndex={isSelected ? 0 : -1}
              disabled={tab.disabled}
              onClick={() => select(tab.id)}
            >
              {tab.icon && <span className="ui-tab-icon" aria-hidden="true">{tab.icon}</span>}
              <span>{tab.label}</span>
              {tab.badge !== undefined && <span className="ui-tab-badge">{tab.badge}</span>}
            </button>
          );
        })}
      </div>
      {active?.panel !== undefined && (
        <div
          role="tabpanel"
          id={panelId(active.id)}
          aria-labelledby={tabId(active.id)}
          tabIndex={0}
          className="ui-tab-panel"
        >
          {active.panel}
        </div>
      )}
    </div>
  );
}
