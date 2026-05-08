import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';

export default tseslint.config(
  { ignores: ['dist', 'node_modules', 'node_modules_old'] },

  // Base JS recommended
  js.configs.recommended,

  // TypeScript strict
  ...tseslint.configs.strict,

  // React hooks rules
  {
    plugins: {
      'react-hooks': reactHooks,
      'react-refresh': reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // React Fast Refresh wants component-only files. Hook + Provider in
      // one file (useTour + TourProvider, useToast + ToastContainer, etc.)
      // is the documented Context pattern across the codebase — splitting
      // would just create churn. Allow the pattern via `allowExportNames`
      // for the hooks we ship.
      'react-refresh/only-export-components': ['warn', {
        allowConstantExport: true,
        allowExportNames: [
          'useTour', 'useToast', 'useT', 'useTheme',
          'I18nProvider', 'ThemeProvider', 'TourProvider', 'ToastProvider',
          // App.tsx exports `setRetryDelay` for tests + `RETRY_DELAY` const
          'setRetryDelay', 'RETRY_DELAY',
        ],
      }],
      // The React 19/20 strict-rules are advisory while we migrate. They
      // catch patterns that work today but won't match future React's
      // expectations (setState in effect, deriving state in effect, mutating
      // values during render, refs for non-DOM values). Keep them as
      // warnings so they're visible without breaking CI — track refactors
      // in `docs/tech-debt/`.
      'react-hooks/purity': 'warn',
      'react-hooks/immutability': 'warn',
      'react-hooks/refs': 'warn',
      'react-hooks/set-state-in-effect': 'warn',
      'react-hooks/preserve-manual-memoization': 'warn',
    },
  },

  // Project-specific rules
  {
    files: ['**/*.{ts,tsx}'],
    rules: {
      // ── Strictness ──
      'no-console': ['warn', { allow: ['warn', 'error'] }],
      'no-debugger': 'error',
      // `confirm()` is the project's documented destructive-op pattern
      // (13+ call sites for delete/archive/reset confirmations); `alert()`
      // and `prompt()` are not used. `no-alert` lumps the three together
      // with no granular toggle, so we override it via a custom selector
      // that only flags `alert(` / `prompt(` calls.
      'no-alert': 'off',
      'no-restricted-syntax': ['warn',
        { selector: "CallExpression[callee.name='alert']", message: 'alert() blocks the UI thread; use the toast hook instead.' },
        { selector: "CallExpression[callee.name='prompt']", message: 'prompt() is not used; build a proper modal/form instead.' },
      ],
      'prefer-const': 'error',
      'no-var': 'error',
      // `== null` and `!= null` are the idiomatic dual-check for null-or-
      // undefined used throughout the codebase (API response shapes, React
      // optional props, JSON pointer lookups). Strict `===` for everything
      // else.
      'eqeqeq': ['error', 'always', { null: 'ignore' }],
      'no-implicit-coercion': ['error', { allow: ['!!'] }],
      'no-throw-literal': 'error',

      // ── TypeScript strict ──
      '@typescript-eslint/no-explicit-any': 'warn',
      '@typescript-eslint/no-unused-vars': ['error', {
        argsIgnorePattern: '^_',
        varsIgnorePattern: '^_',
        destructuredArrayIgnorePattern: '^_',
        caughtErrorsIgnorePattern: '^_',
      }],
      '@typescript-eslint/no-non-null-assertion': 'warn',
      '@typescript-eslint/consistent-type-imports': ['error', { prefer: 'type-imports' }],

      // ── Relaxations for existing code patterns ──
      '@typescript-eslint/no-empty-function': 'off',
      '@typescript-eslint/no-dynamic-delete': 'off',
      // api.ts uses void as generic arg (api<void>), which is idiomatic
      '@typescript-eslint/no-invalid-void-type': ['error', { allowAsThisParameter: false, allowInGenericTypeArguments: true }],
      '@typescript-eslint/no-unused-expressions': 'error',
    },
  },

  // Dashboard uses IIFE render blocks — allow unused expressions
  {
    files: ['**/pages/Dashboard.tsx'],
    rules: {
      '@typescript-eslint/no-unused-expressions': 'off',
    },
  },

  // api.ts — void used as generic arg in api<void>() calls, which is idiomatic
  {
    files: ['**/lib/api.ts'],
    rules: {
      '@typescript-eslint/no-invalid-void-type': 'off',
    },
  },

  // Generated types — lenient. ts-rs writes one file per Rust struct under
  // `src/types/`; they all start with the same banner. We can't lint them
  // because `serde_json::Value` round-trips as `any`, and `RetryConfig`'s
  // exponential backoff fields are typed as bigint with `any` fallbacks.
  {
    files: ['**/types/*.ts', '**/types/*.d.ts'],
    rules: {
      '@typescript-eslint/no-explicit-any': 'off',
      '@typescript-eslint/no-unused-vars': 'off',
    },
  },

  // Test files — more lenient
  {
    files: ['**/*.test.{ts,tsx}', '**/test/**'],
    rules: {
      'no-console': 'off',
      '@typescript-eslint/no-explicit-any': 'off',
      '@typescript-eslint/no-non-null-assertion': 'off',
    },
  },
);
