import { useCallback, useEffect, useRef, useState } from 'react';
import { modelCatalogApi } from '../lib/api';
import { modelRuntimeTargetId } from '../lib/modelCatalogSelection';
import type { ModelCatalogSnapshot, ModelCatalogView } from '../types/generated';

const inFlight = new Map<string, Promise<ModelCatalogView>>();
const queue: Array<() => void> = [];
let running = 0;

function refreshTarget(target: ModelCatalogView): Promise<ModelCatalogView> {
  const id = target.runtime_target_id;
  const existing = inFlight.get(id);
  if (existing) return existing;
  const pending = new Promise<void>(resolve => {
    const start = () => { running++; resolve(); };
    if (running < 2) start(); else queue.push(start);
  }).then(() => modelCatalogApi.refresh({ runtime_target_id: id, agent_type: target.agent_type, force: false }))
    .then(view => {
      if (view.runtime_target_id !== id || view.agent_type !== target.agent_type) {
        throw new Error('Model catalogue target mismatch');
      }
      return view;
    }).finally(() => {
      inFlight.delete(id);
      running--;
      queue.shift()?.();
    });
  inFlight.set(id, pending);
  return pending;
}

/** Cached options stay visible while stale CLI catalogues refresh, never per keystroke. */
export function useModelCatalogSnapshot(open: boolean, runtimeTargets?: string[]) {
  const [catalog, setCatalog] = useState<ModelCatalogSnapshot | null>(null);
  const [catalogError, setCatalogError] = useState(false);
  const [loading, setLoading] = useState(open);
  const generation = useRef(0);
  const targetKey = runtimeTargets ? JSON.stringify([...new Set(runtimeTargets)].sort()) : '*';

  const invalidateSnapshot = useCallback((targetId?: string) => {
    setCatalog(previous => previous ? {
      ...previous,
      targets: previous.targets.map(target => !targetId || target.runtime_target_id === targetId
        ? { ...target, stale: true, live_refresh_ok: false } : target),
    } : null);
  }, []);

  const reload = useCallback(async () => {
    const current = ++generation.current;
    const active = () => generation.current === current;
    try {
      const snapshot = await modelCatalogApi.list();
      if (!active()) return snapshot;
      setCatalog(snapshot);
      setCatalogError(false);
      setLoading(false);
      const selected: string[] | null = targetKey === '*' ? null : JSON.parse(targetKey);
      for (const target of snapshot.targets) {
        if ((!target.stale && target.live_refresh_ok)
          || !target.runtime_target_id.startsWith('agent:')
          || ['Ollama', 'LiteLlm', 'Nvidia', 'Custom'].includes(target.agent_type)
          || modelRuntimeTargetId(target.agent_type) !== target.runtime_target_id
          || (selected && !selected.includes(target.runtime_target_id))) continue;
        void refreshTarget(target).then(view => {
          if (!active()) return;
          setCatalog(previous => previous ? { ...previous, targets: previous.targets.map(item =>
            item.runtime_target_id === view.runtime_target_id
              ? { ...view, target_label: view.target_label ?? item.target_label } : item) } : null);
        }).catch(() => {
          if (!active()) return;
          setCatalogError(true);
          invalidateSnapshot(target.runtime_target_id);
        });
      }
      return snapshot;
    } catch (error) {
      if (active()) {
        setCatalogError(true);
        setLoading(false);
        invalidateSnapshot();
      }
      throw error;
    }
  }, [targetKey, invalidateSnapshot]);
  const refetch = useCallback(() => reload().catch(() => null), [reload]);
  const cancelPendingResults = useCallback(() => { generation.current++; }, []);

  useEffect(() => {
    if (open) void refetch();
    return cancelPendingResults;
  }, [open, refetch, cancelPendingResults]);
  return { catalog, catalogError, data: catalog, error: catalogError, loading, refetch, reload, invalidateSnapshot };
}
