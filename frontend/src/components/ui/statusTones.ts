/** KT-1880 — the tones of a StatusBadge: one per --kr-status-* colour. */
export type StatusTone = 'success' | 'warning' | 'danger' | 'info' | 'neutral';

export const STATUS_TONES: readonly StatusTone[] = ['success', 'warning', 'danger', 'info', 'neutral'];
