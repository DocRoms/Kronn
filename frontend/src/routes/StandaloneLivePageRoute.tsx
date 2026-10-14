import { useParams, useSearchParams } from 'react-router';
import { livePageViewParams } from '../lib/live-page-navigation';
import { StandaloneLivePage } from '../pages/StandaloneLivePage';
import { HomeRedirect } from './HomeRedirect';

export function StandaloneLivePageRoute() {
  const { pageId } = useParams<{ pageId: string }>();
  // The query carries the Page's view parameters (`?tv=1` for a wall screen).
  const [search] = useSearchParams();
  const id = pageId?.trim();
  if (!id) return <HomeRedirect />;
  return <StandaloneLivePage pageId={id} params={livePageViewParams(search.toString())} />;
}
