import { useTheme } from '../lib/ThemeContext';
import { useMatrixDecode } from '../hooks/useMatrixDecode';

interface MatrixTextProps {
  /** The target text to display. Scrambles on mount / when it changes,
   *  settles to this value after ~500ms. */
  text: string;
}

/** Wraps a string with the one-shot matrix decode animation when (and
 *  only when) the active theme is `matrix`. Otherwise renders the text
 *  verbatim — zero cost, no mounted interval. */
export function MatrixText({ text }: MatrixTextProps) {
  const { theme } = useTheme();
  const active = theme === 'matrix';
  const display = useMatrixDecode(text, active);
  return (
    <span
      className="matrix-text"
      data-decoding={active && display !== text ? 'true' : undefined}
    >
      {display}
    </span>
  );
}
