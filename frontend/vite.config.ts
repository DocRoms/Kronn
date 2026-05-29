/// <reference types="vitest" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  worker: {
    format: 'es',
  },
  build: {
    rollupOptions: {
      output: {
        // Vite 8 uses rolldown under the hood — the object form
        // `{name: [pkg, ...]}` that rollup accepted is no longer
        // supported. Function form: receives the imported module id and
        // returns the chunk name.
        manualChunks(id) {
          if (id.includes('node_modules/react/')
              || id.includes('node_modules/react-dom/')) {
            return 'vendor-react';
          }
          if (id.includes('node_modules/react-markdown/')
              || id.includes('node_modules/remark-gfm/')) {
            return 'vendor-markdown';
          }
          return undefined;
        },
      },
    },
  },
  server: {
    port: Number(process.env.VITE_DEV_PORT ?? 5173),
    proxy: {
      // KRONN_BACKEND_URL lets perf-test scripts point at a sandbox backend
      // (e.g. http://localhost:3141 with KRONN_DATA_DIR=/tmp/kronn-perf-sandbox)
      // without touching the user's real backend on :3140.
      //
      // `ws: true` MUST be on this entry — the frontend connects to
      // `/api/ws` (not `/ws`), so the WS upgrade lives under the `/api`
      // namespace. Without this, `useWebSocket` sees the upgrade time out
      // ("WebSocket opening handshake timed out") and the multi-user /
      // streaming-progress / partial-recovery channels stay silent under
      // `pnpm dev`. (Production goes through nginx which handles upgrades
      // transparently — the bug only bit the dev server.)
      '/api': {
        target: process.env.KRONN_BACKEND_URL ?? 'http://localhost:3140',
        changeOrigin: true,
        ws: true,
      },
    },
  },
  test: {
    globals: true,
    environment: 'happy-dom',
    setupFiles: ['./src/test/setup.ts'],
    css: false,
    testTimeout: 30000,
    hookTimeout: 30000,
    // E2E specs live under e2e/ and use Playwright's runner; vitest must
    // skip them or `import { test } from '@playwright/test'` blows up
    // with "test.describe() called outside Playwright".
    //
    // `node_modules_old/**` covers the legacy pnpm cache some local devs
    // keep around (renamed manually after a `pnpm install` that broke
    // something). Matching it here keeps a `pnpm test` invocation green
    // even when that 100+ MB sibling directory is present — without
    // this, vitest picks up the embedded test files in third-party
    // packages and fails on imports that aren't part of our project.
    exclude: ['node_modules/**', 'node_modules_old/**', 'dist/**', 'e2e/**'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.{ts,tsx}'],
      exclude: [
        'src/test/**',
        'src/**/__tests__/**',
        'src/types/generated.ts',
        'src/main.tsx',
        'src/vite-env.d.ts',
        // Web Workers — run off the main thread via `self.onmessage` /
        // `self.postMessage`, loading ML models (Transformers STT / VITS
        // TTS). happy-dom can't instantiate a Worker context, so V8 never
        // records their lines and they drag the headline down for code
        // that is, by construction, un-unit-testable here (covered by the
        // e2e voice flow instead).
        'src/lib/stt-worker.ts',
        'src/lib/tts-worker.ts',
        // TTS playback engine — drives a Web Worker + `new Audio()` /
        // HTMLAudioElement playback. happy-dom has no audio element or
        // Worker runtime, so the synth/play loop can't execute here
        // (covered by the e2e voice flow, like the workers above).
        'src/lib/tts-engine.ts',
      ],
      // Regression floors — pinned just below the actual coverage so a
      // single test removal tanks CI. Bump these up when we land another
      // coverage push. Don't lower them — that's the whole point.
      //
      // 2026-05-29 coverage sprint raised the bar in two waves:
      //  (1) api.ts (the entire UI↔backend boundary) 11%→99% Functions /
      //      25%→92% Lines — every method (verb/URL/encoding/body) + SSE
      //      streamers exercised. Workers (stt/tts) excluded above.
      //  (2) leaf + stateful components: QuickApiForm 0→96%, ProjectSkills
      //      5→100%, SwipeableDiscItem 44→95%, DiscussionSidebar 69→97%,
      //      MessageBubble 67→85% Lines.
      //  (3) mid-tier components: GitPanel 31→93%, CustomApiAiHelper
      //      51→94%, ApiCallAiHelper 54→94%, AiDocViewer 38→67%,
      //      AgentsSection 52→70%. tts-engine.ts excluded (Web Audio).
      //  (4) WorkflowWizard 35→80%, WorkflowDetail 50→62%,
      //      ProjectLinkedRepos 39→97%. Crossed the 70% Lines milestone.
      // Actuals: Statements 67.6 / Branches 63.1 / Functions 63.0 / Lines 71.0.
      // Floors kept DELIBERATELY TIGHT (~0.5pt under actual) until we reach
      // 75% Lines front / 80% back — a >1pt cushion is too loose at this
      // stage. Ratchet up after every coverage push; never down.
      thresholds: {
        statements: 67,
        branches: 62.5,
        functions: 62.5,
        lines: 70.5,
      },
    },
  },
});
