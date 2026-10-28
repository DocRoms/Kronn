import { FileCode2 } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { formatBytes, pageHtmlWeight, type PageHtmlWeight } from '../lib/live-page-weight';

const TITLE_KEYS: Record<PageHtmlWeight, string> = {
  ok: 'pages.htmlWeight.ok',
  heavy: 'pages.htmlWeight.heavy',
  'very-heavy': 'pages.htmlWeight.veryHeavy',
};

/** A Page's HTML weight: always shown in the header, only when heavy in the list. */
export function HtmlWeightBadge({ bytes, variant }: { bytes: number; variant: 'header' | 'list' }) {
  const { t } = useT();
  const weight = pageHtmlWeight(bytes);
  if (variant === 'list' && weight === 'ok') return null;
  const size = formatBytes(bytes);
  const label = t(TITLE_KEYS[weight], size);
  return (
    <span
      className={variant === 'header' ? 'live-pages-html-weight' : 'live-pages-weight-badge'}
      data-weight={weight}
      title={label}
      aria-label={variant === 'header' ? label : undefined}
    >
      {variant === 'header' && <FileCode2 size={11} />} {size}
    </span>
  );
}
