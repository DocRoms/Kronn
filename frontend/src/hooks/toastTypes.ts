export type ToastType = 'success' | 'error' | 'warning' | 'info';

export interface ToastOptions {
  persistent?: boolean;
  copyable?: string;
  dedup?: boolean;
}

export interface Toast {
  id: number;
  message: string;
  type: ToastType;
  persistent: boolean;
  copyable: string | null;
}

export type ToastFn = (
  message: string,
  type?: ToastType,
  options?: ToastOptions,
) => void;
