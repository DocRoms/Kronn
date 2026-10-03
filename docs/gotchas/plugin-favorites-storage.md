# Plugin favorite persistence

Plugin favorites remain browser-local through `usePersistentIdSet` and the `kronn:collection-favorites:plugins` key. [src: file: frontend/src/components/plugins/usePluginListState.ts:61]

The MCP client contract has overview, registry, rescan, test, bundle, config, probe, secret, and context operations, but no plugin-favorite persistence operation. [src: file: frontend/src/lib/api.ts:1380]

Moving these favorites to server storage therefore needs a backend persistence/API contract; KT-832 preserves the existing local behavior instead of adding that cross-layer scope implicitly.
