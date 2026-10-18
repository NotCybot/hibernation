import { defineConfig } from 'vitest/config';
import { builtinModules } from 'node:module';

export default defineConfig({
  build: {
    lib: {
      entry: 'src/index.ts',
      fileName: 'index',
      formats: ['cjs']
    },
    rollupOptions: {
      external: [...builtinModules, ...builtinModules.map((m) => `node:${m}`)]
    },
    sourcemap: true
  },
  plugins: [],
  test: {
    include: ['src/tests/**/*.test.ts'],
    coverage: {
      include: ['src/**/*.ts'],
      exclude: ['src/tests/**'],
      reporter: ['text', 'lcov']
    }
  }
});
