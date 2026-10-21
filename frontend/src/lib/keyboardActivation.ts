import type { KeyboardEvent } from 'react';

/**
 * Enter/Space handler for a non-button click target (role="button", a
 * focusable row). Keys from nested controls are ignored so their own
 * activation is not swallowed by the container.
 */
export function onActivationKey<T extends Element>(activate: () => void) {
  return (event: KeyboardEvent<T>) => {
    if (event.target !== event.currentTarget) return;
    if (event.key !== 'Enter' && event.key !== ' ') return;
    event.preventDefault();
    activate();
  };
}
