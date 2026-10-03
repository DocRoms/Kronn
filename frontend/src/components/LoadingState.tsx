import { useEffect, useLayoutEffect, useState } from 'react';
import { KronnMark } from './KronnMark';
import {
  bootMessage,
  bootScreenActive,
  holdBootScreen,
  onBootScreenFinished,
  setBootPhase,
  type BootPhase,
} from '../lib/bootScreen';

export interface LoadingStateProps {
  phase?: BootPhase;
  /** Sentence under the mark once the app is open; defaults to the phase's. */
  message?: string;
  onRetry?: () => void;
  /** Full window (start-up) rather than the page area. */
  fullscreen?: boolean;
}

/**
 * Kronn's one loading visual: the mark with a plain sentence under it.
 * During a cold start it draws nothing and keeps the screen index.html
 * painted, only changing its sentence, so loaders never hand over to one
 * another on screen.
 */
export function LoadingState({ phase = 'opening', message, onRetry, fullscreen = false }: LoadingStateProps) {
  const [deferToBoot, setDeferToBoot] = useState(bootScreenActive);

  useLayoutEffect(() => {
    if (!deferToBoot) return;
    const release = holdBootScreen();
    const stop = onBootScreenFinished(() => setDeferToBoot(false));
    return () => {
      stop();
      release();
    };
  }, [deferToBoot]);

  useEffect(() => {
    if (deferToBoot) setBootPhase(phase, onRetry);
  }, [deferToBoot, phase, onRetry]);

  if (deferToBoot) return null;
  return (
    <div className={fullscreen ? 'app-fullscreen' : 'kronn-loading-state'} role="status" aria-live="polite">
      <KronnMark size={fullscreen ? 100 : 48} className="app-loading-mark" />
      <span className="app-loading-text">{message ?? bootMessage(phase)}</span>
      {onRetry && (
        <button type="button" onClick={onRetry} className="app-retry-btn">
          {bootMessage('retry')}
        </button>
      )}
    </div>
  );
}

/** Keeps the cold-start screen up while `active`, without drawing anything. */
export function BootHold({ active, phase = 'opening' }: { active: boolean; phase?: BootPhase }) {
  useLayoutEffect(() => {
    if (!active || !bootScreenActive()) return;
    return holdBootScreen();
  }, [active]);
  useEffect(() => {
    if (active) setBootPhase(phase);
  }, [active, phase]);
  return null;
}
