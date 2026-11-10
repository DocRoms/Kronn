import { useEffect, useState } from 'react';
import { config as configApi } from '../lib/api';
import { DEFAULT_HEAVY_OUTPUT_KB } from '../lib/heavyOutput';

/** The configured « heavy output » threshold in KB (Settings > Workers); the
 *  default until the server answers, and when it cannot. */
export function useHeavyOutputThreshold(): number {
  const [kb, setKb] = useState(DEFAULT_HEAVY_OUTPUT_KB);
  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => configApi.getWorkers())
      .then(value => {
        if (live && Number.isFinite(value?.heavy_output_kb) && value.heavy_output_kb > 0) setKb(value.heavy_output_kb);
      })
      .catch(() => {});
    return () => { live = false; };
  }, []);
  return kb;
}
