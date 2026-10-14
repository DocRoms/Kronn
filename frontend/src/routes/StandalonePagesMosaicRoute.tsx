import { useSearchParams } from 'react-router';
import { livePageMosaicRoute } from '../lib/live-page-navigation';
import { StandaloneLivePageMosaic } from '../pages/StandaloneLivePageMosaic';
import { HomeRedirect } from './HomeRedirect';

export function StandalonePagesMosaicRoute() {
  const [params] = useSearchParams();
  const mosaic = livePageMosaicRoute(params);
  if (!mosaic) return <HomeRedirect />;
  return <StandaloneLivePageMosaic pageIds={mosaic.pageIds} layout={mosaic.layout} />;
}
