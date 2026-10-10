import { useCallback, useLayoutEffect, useRef } from 'react';
import { useParams } from 'react-router';
import { HomeRedirect } from './HomeRedirect';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { useDashboardContext } from '../lib/dashboardContext';
import { PagesPage } from '../pages/PagesPage';

/**
 * Progressive disclosure: the library has no address until the first Artifact
 * exists. Nothing renders while the capability is still unknown, so a reload
 * on this route neither flashes the page nor bounces an activated user away.
 */
export function PagesRoute() {
  const ctx = useDashboardContext();
  const nav = useKronnNavigate();
  // The address owns the open Page: `/pages/<id>`. Picking one is a step
  // Back can undo; the one the page opens on its own at the bare address
  // (the last visit, or the first of the library) replaces it instead.
  const { pageId } = useParams<{ pageId?: string }>();
  // Read through a ref: the page lists this callback in an effect's dependencies.
  const addressed = useRef(pageId);
  useLayoutEffect(() => { addressed.current = pageId; }, [pageId]);
  const selectPage = useCallback((id: string | null) => {
    const current = addressed.current;
    if (id === (current ?? null)) return;
    if (id) nav.toLivePage(id, { replace: current === undefined });
    else nav.toPage('pages');
  }, [nav]);
  if (!ctx.pagesCapability) return null;
  if (!ctx.pagesCapability.activated) return <HomeRedirect />;
  return (
    <PagesPage
      projects={ctx.projects}
      selectedPageId={pageId ?? null}
      onSelectedPageChange={selectPage}
      onNavigateWorkflow={nav.toWorkflow}
      onNavigateDiscussion={nav.toDiscussion}
    />
  );
}
