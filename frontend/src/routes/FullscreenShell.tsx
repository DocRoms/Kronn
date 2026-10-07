import { Suspense, type ReactNode } from 'react';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { LoadingState } from '../components/LoadingState';

/** What a whole-window view is rendered in: the fullscreen error screen and loader. */
export function FullscreenShell({ children }: { children: ReactNode }) {
  return (
    <ErrorBoundary>
      <Suspense fallback={<LoadingState fullscreen />}>{children}</Suspense>
    </ErrorBoundary>
  );
}
