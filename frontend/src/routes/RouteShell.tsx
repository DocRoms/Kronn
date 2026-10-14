import { Suspense, type ReactNode } from 'react';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { LoadingState } from '../components/LoadingState';
import { useT } from '../lib/I18nContext';

// The shared loading state lives in the main chunk, so it can cover the fetch
// of a page chunk.
function PageFallback() {
  const { t } = useT();
  return <LoadingState message={t('common.loading')} />;
}

/** What every dashboard page is rendered in: its own error zone and loader. */
export function RouteShell({ label, children }: { label: string; children: ReactNode }) {
  return (
    <ErrorBoundary mode="zone" label={label}>
      <Suspense fallback={<PageFallback />}>{children}</Suspense>
    </ErrorBoundary>
  );
}
