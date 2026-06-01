import '@testing-library/jest-dom/vitest';
import { configure } from '@testing-library/react';

// CI runs `vitest run --coverage` (v8 instrumentation) with heavy file
// parallelism on a shared runner. That slows React effect / microtask
// scheduling enough that the default 1000ms `waitFor` timeout can expire
// before a mount-effect assertion resolves — surfacing as rare, non-local
// flakes (e.g. DebugSection's "getLogs called on mount"). Raising the global
// async timeout gives slow CI runners headroom with ZERO cost on passing
// tests: `waitFor` returns as soon as its callback passes, so a higher ceiling
// only matters when the environment is genuinely slow.
configure({ asyncUtilTimeout: 5000 });
