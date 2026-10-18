import js from '@eslint/js';
import globals from 'globals';

const strict = {
  eqeqeq: 'error',
  'no-var': 'error',
  'prefer-const': 'error',
  'no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
  'no-implicit-coercion': 'error',
  'no-eval': 'error',
  'no-implied-eval': 'error',
  'no-new-func': 'error',
  // The page shows what Claude and the microphone produced: text only, never markup.
  'no-restricted-properties': [
    'error',
    { property: 'innerHTML', message: 'Use textContent.' },
    { property: 'outerHTML', message: 'Use textContent.' },
    { property: 'insertAdjacentHTML', message: 'Use textContent.' },
  ],
  'no-restricted-globals': ['error', { name: 'eval', message: 'No eval.' }],
};

export default [
  js.configs.recommended,
  {
    files: ['ui/**/*.js'],
    languageOptions: { ecmaVersion: 2024, sourceType: 'module', globals: globals.browser },
    rules: strict,
  },
  {
    files: ['tests/ui/**/*.js', 'eslint.config.js'],
    languageOptions: { ecmaVersion: 2024, sourceType: 'module', globals: globals.node },
    rules: strict,
  },
];
