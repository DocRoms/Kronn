import { Database } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { formatBytes, pageDatasetWeight, type PageHtmlWeight } from '../lib/live-page-weight';

const TITLE_KEYS: Record<Exclude<PageHtmlWeight, 'ok'>, string> = {
  heavy: 'pages.datasetWeight.heavy',
  'very-heavy': 'pages.datasetWeight.veryHeavy',
};

/** KT-1525 — a Page dataset's weight, like the HTML's: shown only when heavy,
 *  with the fix (select fields, limit, paginate in the publishing workflow).
 *  `name` is the dataset; omitted for the Page's total. */
export function DatasetWeightBadge({ bytes, name, variant }: {
  bytes: number;
  name?: string;
  variant: 'header' | 'list';
}) {
  const { t } = useT();
  const weight = pageDatasetWeight(bytes);
  if (weight === 'ok') return null;
  const size = formatBytes(bytes);
  const label = t(TITLE_KEYS[weight], name ?? t('pages.datasetWeight.total'), size);
  return (
    <span
      className={variant === 'header' ? 'live-pages-html-weight' : 'live-pages-weight-badge'}
      data-testid="dataset-weight-badge"
      data-weight={weight}
      title={label}
      aria-label={label}
    >
      {variant === 'header' && <Database size={11} />} {variant === 'header' ? size : t('pages.datasetWeight.badge')}
    </span>
  );
}
