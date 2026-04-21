import { useEffect, useRef } from 'react';

/** Classic Konami code — the NES up-up-down-down-left-right-left-right-B-A
 *  sequence. Kept as `KeyboardEvent.key` strings so we don't have to worry
 *  about layout (QWERTY/AZERTY) for letters. */
const KONAMI_SEQUENCE = [
  'ArrowUp', 'ArrowUp',
  'ArrowDown', 'ArrowDown',
  'ArrowLeft', 'ArrowRight',
  'ArrowLeft', 'ArrowRight',
  'b', 'a',
] as const;

/** Listen for the Konami code anywhere in the document and fire `onUnlock`
 *  when the full sequence lands. Keys pressed inside inputs/textareas
 *  are ignored so the user can navigate text fields with arrow keys
 *  without accidentally advancing the sequence (or aborting one in
 *  progress).
 *
 *  Letter keys are matched case-insensitively (`b`/`B`/`a`/`A` all work).
 *  A non-matching key resets the sequence, but the new key is treated
 *  as a potential sequence start — so pressing ↑↑↓↓←→←→AAB↑↑↓↓←→←→BA
 *  still succeeds on the second attempt without needing to pause. */
export function useKonamiCode(onUnlock: () => void): void {
  const onUnlockRef = useRef(onUnlock);
  onUnlockRef.current = onUnlock;

  useEffect(() => {
    let pos = 0;
    const handler = (e: KeyboardEvent) => {
      // Ignore keypresses while editing text — otherwise arrow keys
      // moving the cursor would eat the sequence mid-type.
      const target = e.target as HTMLElement | null;
      if (target) {
        const tag = target.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA' || target.isContentEditable) {
          return;
        }
      }

      const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
      const expected = KONAMI_SEQUENCE[pos];

      if (key === expected) {
        pos++;
        if (pos === KONAMI_SEQUENCE.length) {
          pos = 0;
          onUnlockRef.current();
        }
      } else {
        // Non-match — reset. If the wrong key happens to be the first
        // of the sequence (↑), treat it as a fresh start so a user who
        // fumbles doesn't have to pause before retrying.
        pos = key === KONAMI_SEQUENCE[0] ? 1 : 0;
      }
    };

    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, []);
}
