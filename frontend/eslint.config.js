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
      'react-refresh/only-export-components': ['warn', { allowConstantExport: true }],
    },
  },

  // Project-specific rules
  {
    files: ['**/*.{ts,tsx}'],
    rules: {
      // ── Strictness ──
      'no-console': ['warn', { allow: ['warn', 'error'] }],
      'no-debugger': 'error',
      'no-alert': 'warn',
      'prefer-const': 'error',
      'no-var': 'error',
      'eqeqeq': ['error', 'always'],
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

  // Generated types — lenient
  {
    files: ['**/types/generated.ts'],
    rules: {
      '@typescript-eslint/no-explicit-any': 'off',
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
