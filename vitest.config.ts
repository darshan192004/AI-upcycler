import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    globals: true,
    include: ['packages/**/*.test.ts', 'apps/*/src/**/*.test.ts'],
    coverage: {
      provider: 'v8',
      include: ['packages/protocol/src/**', 'packages/layer-planner/src/**'],
      thresholds: { lines: 80, functions: 80, branches: 80, statements: 80 },
    },
  },
});
