import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import uiGuards from './scripts/eslint-ui-guards.mjs';

/** KT-1881 — the message of the rules that refuse a new native confirm(). */
export const NATIVE_CONFIRM = 'window.confirm() is replaced by ConfirmDialog: use useConfirm() from components/ui/useConfirm (await confirm({ title, message, confirmLabel })). See docs/conventions/ui-components.md.';

/** KT-1887 — the design-system rules (scripts/eslint-ui-guards.mjs). */
export const UI_GUARD_RULES = Object.keys(uiGuards.rules).map(rule => `ui-guards/${rule}`);

/** KT-1887 — the folder of the base components accepts no warning at all.
 *  CI's `--max-warnings` ceiling counts the whole frontend, so a new warning
 *  there would pass as long as the total stays under it. */
export const UI_FOLDER = 'src/components/ui/**';

/** Every rule the configs set to "warn" for any TS/TSX file, as an error
 *  (with its options). The per-file relaxations (Dashboard, api.ts, types)
 *  never match components/ui, so only the configs that reach every file
 *  count, the last one winning as in the cascade. */
export function warningsAsErrors(list) {
  const rules = {};
  for (const config of list) {
    const generic = !config.files || config.files.flat().some(f => /^\*\*\/\*\.[{a-z]/.test(f));
    if (!generic) continue;
    for (const [rule, entry] of Object.entries(config.rules ?? {})) {
      const level = Array.isArray(entry) ? entry[0] : entry;
      if (level === 'warn' || level === 1) rules[rule] = Array.isArray(entry) ? ['error', ...entry.slice(1)] : 'error';
      else delete rules[rule];
    }
  }
  return rules;
}

const configs = tseslint.config(
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
      // `no-alert` lumps alert, confirm and prompt together with no granular
      // toggle, so each gets its own rule below.
      'no-alert': 'off',
      'no-restricted-syntax': ['warn',
        { selector: "CallExpression[callee.name='alert']", message: 'alert() blocks the UI thread; use the toast hook instead.' },
        { selector: "CallExpression[callee.name='prompt']", message: 'prompt() is not used; build a proper modal/form instead.' },
      ],
      // KT-1881 — the native confirm() is replaced by ConfirmDialog
      // (components/ui/ConfirmDialog.tsx, or useConfirm for a promise).
      // The calls that existed when the rule came in are recorded in
      // eslint-suppressions.json (ESLint bulk suppressions): a new call
      // fails, and a removed one must leave the file too (`pnpm lint
      // --prune-suppressions`). no-restricted-globals is scope-aware: a
      // local function named `confirm` is not the global one.
      'no-restricted-globals': ['error', { name: 'confirm', message: NATIVE_CONFIRM }],
      'no-restricted-properties': ['error',
        { object: 'window', property: 'confirm', message: NATIVE_CONFIRM },
        { object: 'globalThis', property: 'confirm', message: NATIVE_CONFIRM },
        { object: 'self', property: 'confirm', message: NATIVE_CONFIRM },
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

  // KT-1887 — the design-system guards (scripts/eslint-ui-guards.mjs). The
  // violations that existed when they came in are recorded in
  // eslint-suppressions.json; a new one fails. See
  // docs/conventions/ui-lint-guards.md.
  {
    files: ['src/**/*.{ts,tsx}'],
    plugins: { 'ui-guards': uiGuards },
    rules: Object.fromEntries(UI_GUARD_RULES.map(rule => [rule, 'error'])),
  },
  // components/ui is where Dialog, Drawer and ConfirmDialog are built.
  {
    files: [UI_FOLDER],
    rules: { 'ui-guards/no-adhoc-dialog': 'off' },
  },
);

export default tseslint.config(
  ...configs,

  {
    files: [UI_FOLDER],
    linterOptions: { reportUnusedDisableDirectives: 'error', reportUnusedInlineConfigs: 'error' },
    rules: warningsAsErrors(configs),
  },

  // Test files — more lenient
  {
    files: ['**/*.test.{ts,tsx}', '**/test/**'],
    rules: {
      'no-console': 'off',
      '@typescript-eslint/no-explicit-any': 'off',
      '@typescript-eslint/no-non-null-assertion': 'off',
      // A test stubs or spies on the native confirm of the code it covers.
      'no-restricted-globals': 'off',
      'no-restricted-properties': 'off',
    },
  },

  // A test and its helpers render fixtures: literal strings, bare controls,
  // raw styles. The design-system guards apply to the app only.
  {
    files: ['**/*.test.{ts,tsx}', '**/test/**', '**/__tests__/**'],
    rules: Object.fromEntries(UI_GUARD_RULES.map(rule => [rule, 'off'])),
  },
);
