import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'jsdom',
    coverage: {
      provider: 'v8', // or 'istanbul'
      reporter: ['text', 'json', 'html'],
      reportsDirectory: './coverage',
      exclude: [
        'coverage/**',
        'dist/**',
        '**/node_modules/**',
        '**/[.]**',
        'packages/*/test?(s)/**',
        '**/*.d.ts',
        '**/virtual:*',
        '**/__x00__*',
        '**/\x00*',
        'cypress/**',
        'test?(s)/**',
        'test?(-*).?(c|m)[jt]s?(x)',
        '**/*{.,-}{test,spec}.?(c|m)[jt]s?(x)',
        '**/__tests__/**',
        '**/{karma,rollup,webpack,vite,vitest,jest,ava,babel,nyc,cypress,tsup,build}.config.*',
        '**/vitest.{workspace,projects}.[jt]s?(on)',
        '**/.{eslint,mocha,prettier}rc.{?(c|m)js,yml}',
        // Project-specific excludes
        'src/main.ts',
        'src/vite-env.d.ts',
        'src/app.css',
        'stryker.conf.json',
        'reports/**'
      ],
      include: [
        'src/**/*.{js,ts,svelte}',
        '!src/**/*.spec.{js,ts}',
        '!src/**/*.test.{js,ts}',
        '!src/**/__tests__/**'
      ],
      all: true,
      thresholds: {
        lines: 7,
        functions: 25,
        branches: 65,
        statements: 7
      }
    }
  },
});
