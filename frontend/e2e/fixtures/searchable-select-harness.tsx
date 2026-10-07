import { createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { SearchableSelect } from '../../src/components/SearchableSelect';
import '../../src/styles/index.css';

/** Mounts the real SearchableSelect alone, its control `top` px from the top. */
export function mount(top: number, count: number): void {
  const host = document.createElement('div');
  host.style.cssText = `position:fixed;top:${top}px;left:20px;width:500px;z-index:2147483647`;
  document.body.appendChild(host);
  createRoot(host).render(createElement(SearchableSelect, {
    value: '', onChange: () => {}, label: 'Model', placeholder: 'Search', emptyLabel: 'Empty',
    className: 'searchable-select--compact',
    options: Array.from({ length: count }, (_, i) => ({ value: `provider/model-${i}`, label: `provider/model-${i}` })),
  }));
}
