import { useSearchParams } from 'react-router';
import { discussionMosaicRoute, discussionMosaicSearch } from '../lib/discussion-mosaic-navigation';
import { StandaloneDiscussionMosaic } from '../pages/StandaloneDiscussionMosaic';
import { HomeRedirect } from './HomeRedirect';

export function StandaloneDiscussionsMosaicRoute() {
  const [params, setParams] = useSearchParams();
  const mosaic = discussionMosaicRoute(params);
  if (!mosaic) return <HomeRedirect />;
  return (
    <StandaloneDiscussionMosaic
      discussionIds={mosaic.discussionIds}
      layout={mosaic.layout}
      // A layout change rewrites the address in place: it is not a step
      // Back should undo.
      onLayoutChange={layout => setParams(discussionMosaicSearch(mosaic.discussionIds, layout), { replace: true })}
    />
  );
}
