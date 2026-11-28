module.exports = {
  root: true,
  parser: '@typescript-eslint/parser',
  parserOptions: {
    ecmaVersion: 2022,
    sourceType: 'module',
  },
  plugins: ['@typescript-eslint'],
  extends: ['eslint:recommended', 'plugin:@typescript-eslint/recommended'],
  env: {
    es2022: true,
    node: true,
  },
  ignorePatterns: [
    'node_modules/',
    'dist/',
    '**/build/',
    'apps/*/android/',
    'apps/*/windows/',
    'apps/*/babel.config.js',
    'apps/*/jest.config.js',
    'apps/*/jest.config.windows.js',
    'apps/*/metro.config.js',
  ],
  rules: {
    '@typescript-eslint/no-explicit-any': 'error',
    '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
  },
  overrides: [
    {
      files: ['**/*.test.ts', '**/*.test.tsx'],
      env: { node: true },
      globals: {
        describe: 'readonly',
        it: 'readonly',
        expect: 'readonly',
        beforeEach: 'readonly',
        afterEach: 'readonly',
      },
    },
  ],
};
