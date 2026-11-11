import js from '@eslint/js';
import jsxA11y from 'eslint-plugin-jsx-a11y-x';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import globals from 'globals';
import ts from 'typescript-eslint';

export default ts.config(
  {
    ignores: [
      'dist/',
      'coverage/',
      'node_modules/',
      'src-tauri/target/',
      'src-tauri/gen/',
      'dev/servers/',
      'src/lib/ipc/bindings/',
    ],
  },
  js.configs.recommended,
  ...ts.configs.recommended,
  reactHooks.configs.flat['recommended-latest'],
  jsxA11y.configs.recommended,
  {
    files: ['src/**/*.{ts,tsx}'],
    languageOptions: { globals: globals.browser },
    plugins: { 'react-refresh': reactRefresh },
    rules: {
      'react-refresh/only-export-components': ['warn', { allowConstantExport: true }],
    },
  },
  {
    // Library modules (hooks, editor kit) are not hot-reloaded pages.
    files: ['src/lib/**/*.{ts,tsx}'],
    rules: { 'react-refresh/only-export-components': 'off' },
  },
  {
    files: ['*.{js,ts}', 'scripts/**/*.{js,mjs}'],
    languageOptions: { globals: globals.node },
  },
  {
    rules: {
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_' },
      ],
      '@typescript-eslint/consistent-type-imports': 'error',
      'no-console': ['warn', { allow: ['warn', 'error'] }],
      eqeqeq: ['error', 'smart'],
    },
  },
  // Tests and CLI scripts may print (must come after the general rules).
  {
    files: ['src/**/*.test.{ts,tsx}', 'src/test/**', 'scripts/**/*.mjs'],
    rules: { 'no-console': 'off' },
  },
);
