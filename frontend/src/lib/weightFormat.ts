// Byte formatting and load-state type for the storage-weight indicator.
// Kept out of the component file so importing them does not disable React
// fast refresh for the component.

/** Per-discussion load state.
 *
 * `ready` may only be used for an id that was actually REQUESTED: for such an
 * id an absent weight really means empty. `unmeasured` is the id that was
 * never asked for — bounding the batch does not make those rows weigh zero,
 * and rendering a 0 for them would be an invented measurement. */
export type WeightLoadState = 'loading' | 'unavailable' | 'ready' | 'unmeasured';

export function formatBytes(bytes: number): string {
  if (bytes <= 0) return '0 o';
  const units = ['o', 'Ko', 'Mo', 'Go', 'To'];
  const exp = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  const value = bytes / 1024 ** exp;
  // Sub-unit precision only where it carries information, and no trailing
  // ".0": "2 Ko" reads better than "2.0 Ko" and means the same thing.
  const digits = exp === 0 ? 0 : value < 10 ? 1 : 0;
  const text = value.toFixed(digits).replace(/\.0$/, '');
  return `${text} ${units[exp]}`;
}
