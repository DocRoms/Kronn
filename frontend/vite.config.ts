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
      ],
    },
  },
});
