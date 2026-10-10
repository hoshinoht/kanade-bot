import js from '@eslint/js';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import ts from 'typescript-eslint';

export default ts.config(
  { ignores: ['**/dist/**', '**/node_modules/**', 'e2e/.results/**', 'e2e/.captures/**'] },
  js.configs.recommended,
  ...ts.configs.recommended,
  ...svelte.configs.recommended,
  {
    languageOptions: { globals: { ...globals.browser, ...globals.node } },
  },
  {
    files: ['**/*.svelte', '**/*.svelte.ts'],
    languageOptions: { parserOptions: { parser: ts.parser, extraFileExtensions: ['.svelte'] } },
  },
  {
    files: ['**/*.svelte'],
    rules: {
      // style-src 'self' (no 'unsafe-inline'): style attributes, style: directives
      // and Svelte transitions (which write inline styles) are all banned.
      'svelte/no-inline-styles': ['error', { allowTransitions: false }],
      'svelte/no-at-html-tags': 'error',
    },
  },
  {
    rules: {
      'no-restricted-properties': [
        'error',
        { property: 'innerHTML', message: 'Trusted Types sink; build DOM nodes instead.' },
        { property: 'outerHTML', message: 'Trusted Types sink.' },
        { property: 'insertAdjacentHTML', message: 'Trusted Types sink.' },
      ],
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
    },
  },
);
