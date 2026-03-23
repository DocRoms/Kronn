import { useState, useCallback, useRef } from 'react';
import React from 'react';

type ToastType = 'success' | 'error' | 'info';

interface Toast {
  id: number;
  message: string;
  type: ToastType;
}

export type ToastFn = (message: string, type?: ToastType) => void;

let styleInjected = false;

export function useToast() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const idRef = useRef(0);

  const toast: ToastFn = useCallback((message: string, type: ToastType = 'info') => {
    const id = ++idRef.current;
    setToasts(prev => [...prev.slice(-2), { id, message, type }]); // max 3
    setTimeout(() => setToasts(prev => prev.filter(t => t.id !== id)), 4000);
  }, []);

  const ToastContainer = useCallback(() => {
    const children: React.ReactElement[] = [];

    // Inject keyframes style once
    if (!styleInjected) {
      styleInjected = true;
      children.push(
        React.createElement('style', { key: '__toast_style' },
          '@keyframes toastSlideIn { from { transform: translateX(100%); opacity: 0; } to { transform: translateX(0); opacity: 1; } }'
        )
      );
    }

    for (const t of toasts) {
      children.push(
        React.createElement('div', {
          key: t.id,
          role: 'alert',
          'aria-live': 'assertive',
          style: {
            padding: '10px 16px',
            borderRadius: 8,
            fontSize: 13,
            color: '#fff',
            background: t.type === 'error'
              ? 'rgba(220,50,50,0.95)'
              : t.type === 'success'
                ? 'rgba(50,180,50,0.95)'
                : 'rgba(0,180,200,0.95)',
            border: `1px solid ${
              t.type === 'error'
                ? 'rgba(255,80,80,0.3)'
                : t.type === 'success'
                  ? 'rgba(80,255,80,0.3)'
                  : 'rgba(80,200,255,0.3)'
            }`,
            backdropFilter: 'blur(10px)',
            maxWidth: 350,
            animation: 'toastSlideIn 0.3s ease-out',
          },
        }, t.message)
      );
    }

    return React.createElement('div', {
      style: {
        position: 'fixed',
        top: 16,
        right: 16,
        zIndex: 9999,
        display: 'flex',
        flexDirection: 'column' as const,
        gap: 8,
        pointerEvents: 'none',
      },
    }, ...children);
  }, [toasts]);

  return { toast, ToastContainer };
}
